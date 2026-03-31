use crate::config::{ConfigWarning, LoadResult, has_config_changed, load_config};
use crate::device::{MouseDevice, NormalizedMouseEvent};
use crate::error::AppError;
use crate::router::{HoldBehavior, KeyStroke, RoutedAction, route};
use crate::virtual_keyboard::VirtualKeyboard;
use crate::virtual_mouse::VirtualMouse;
use clap::Parser;
use evdev::KeyCode;
use notify_rust::Notification;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{Duration, Instant, interval};

#[derive(Debug, Parser)]
#[command(name = "mousefold")]
#[command(about = "Mouse to keyboard remapper daemon", version)]
pub struct Cli {
    /// Path to the YAML configuration file.
    #[arg(short, long, value_name = "FILE")]
    pub config: PathBuf,

    /// Validate the configuration and exit.
    #[arg(short = 'v', long, default_value_t = false)]
    pub check_config: bool,
}

pub async fn run(cli: Cli) -> Result<(), AppError> {
    let load_result = load_config(&cli.config)?;

    if cli.check_config {
        report_warnings(&load_result.warnings);
        log_info(&format!(
            "config OK: {} ({})",
            cli.config.display(),
            load_result.config.device_selector.describe()
        ));
        return Ok(());
    }

    let mut runtime = Runtime::from_load_result(load_result)?;

    log_info(&format!(
        "started with config={} selector={} resolved_device={} ({})",
        runtime.config.source_path.display(),
        runtime.config.device_selector.describe(),
        runtime.mouse_device.resolved_path().display(),
        runtime.mouse_device.resolved_name()
    ));

    let mut sigint = signal(SignalKind::interrupt()).map_err(AppError::Signal)?;
    let mut sigterm = signal(SignalKind::terminate()).map_err(AppError::Signal)?;
    let mut reload_tick = interval(if runtime.config.reload.enabled {
        Duration::from_millis(runtime.config.reload.debounce_ms)
    } else {
        Duration::MAX
    });
    let mut hold_tick = interval(Duration::from_millis(5));

    loop {
        tokio::select! {
            _ = sigint.recv() => {
                log_info("received SIGINT, shutting down");
                break;
            }
            _ = sigterm.recv() => {
                log_info("received SIGTERM, shutting down");
                break;
            }
            event = runtime.mouse_device.next_event() => {
                runtime.handle_event(event?)?;
            }
            _ = hold_tick.tick() => {
                runtime.release_due_keys()?;
            }
            _ = reload_tick.tick(), if runtime.config.reload.enabled => {
                if has_config_changed(&runtime.config.source_path, runtime.config.source_modified)?.is_none() {
                    continue;
                }

                match runtime.apply_reload().await {
                    Ok(()) => {
                        log_info(&format!(
                            "reloaded config={} selector={} resolved_device={} ({})",
                            runtime.config.source_path.display(),
                            runtime.config.device_selector.describe(),
                            runtime.mouse_device.resolved_path().display(),
                            runtime.mouse_device.resolved_name()
                        ));
                    }
                    Err(err) => {
                        log_warn(&format!(
                            "reload failed for {}: {err}; keeping previous configuration",
                            runtime.config.source_path.display()
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

struct Runtime {
    config: crate::config::ActiveConfig,
    active_mode_index: usize,
    mouse_device: MouseDevice,
    virtual_mouse: VirtualMouse,
    virtual_keyboard: VirtualKeyboard,
    pending_mouse_events: Vec<NormalizedMouseEvent>,
    pending_keyboard_events: Vec<KeyStroke>,
    active_button_outputs: HashMap<KeyCode, Vec<ActiveButtonOutput>>,
    pressed_output_counts: HashMap<KeyCode, usize>,
    scheduled_releases: Vec<ScheduledRelease>,
}

#[derive(Clone, Copy, Debug)]
struct ActiveButtonOutput {
    key: KeyCode,
    hold: HoldBehavior,
}

#[derive(Clone, Copy, Debug)]
struct ScheduledRelease {
    due_at: Instant,
    key: KeyCode,
}

impl Runtime {
    fn from_load_result(load_result: LoadResult) -> Result<Self, AppError> {
        report_warnings(&load_result.warnings);

        let mouse_device = MouseDevice::open_and_grab(&load_result.config.device_selector)?;
        let virtual_mouse = VirtualMouse::build_from_source_caps(
            mouse_device.source_capabilities(),
            mouse_device.resolved_name(),
        )?;
        let virtual_keyboard = VirtualKeyboard::build(
            load_result.config.rules.registered_keys(),
            mouse_device.resolved_name(),
        )?;

        log_info(&format!(
            "grabbed source device {}",
            mouse_device.resolved_path().display()
        ));

        Ok(Self {
            config: load_result.config,
            active_mode_index: 0,
            mouse_device,
            virtual_mouse,
            virtual_keyboard,
            pending_mouse_events: Vec::new(),
            pending_keyboard_events: Vec::new(),
            active_button_outputs: HashMap::new(),
            pressed_output_counts: HashMap::new(),
            scheduled_releases: Vec::new(),
        })
    }

    async fn apply_reload(&mut self) -> Result<(), AppError> {
        let previous_mode_name = self
            .config
            .rules
            .current_mode_name(self.active_mode_index)
            .map(str::to_owned);
        let load_result = load_config(&self.config.source_path)?;
        report_warnings(&load_result.warnings);

        if load_result.config.device_selector != self.config.device_selector {
            let replacement_mouse =
                MouseDevice::open_and_grab(&load_result.config.device_selector)?;
            let replacement_virtual_mouse = VirtualMouse::build_from_source_caps(
                replacement_mouse.source_capabilities(),
                replacement_mouse.resolved_name(),
            )?;
            let replacement_virtual_keyboard = VirtualKeyboard::build(
                load_result.config.rules.registered_keys(),
                replacement_mouse.resolved_name(),
            )?;

            self.reset_keyboard_state()?;
            self.pending_mouse_events.clear();
            self.mouse_device = replacement_mouse;
            self.virtual_mouse = replacement_virtual_mouse;
            self.virtual_keyboard = replacement_virtual_keyboard;
            self.config = load_result.config;
            self.active_mode_index =
                resolve_reloaded_mode_index(&self.config.rules, previous_mode_name);
            return Ok(());
        }

        self.reset_keyboard_state()?;
        self.virtual_keyboard = VirtualKeyboard::build(
            load_result.config.rules.registered_keys(),
            self.mouse_device.resolved_name(),
        )?;
        self.config = load_result.config;
        self.active_mode_index =
            resolve_reloaded_mode_index(&self.config.rules, previous_mode_name);
        Ok(())
    }

    fn handle_event(&mut self, event: NormalizedMouseEvent) -> Result<(), AppError> {
        match route(&event, &self.config.rules, self.active_mode_index) {
            RoutedAction::PassThrough => self.pending_mouse_events.push(event),
            RoutedAction::Remap(sequence) => {
                let sequence = sequence.to_vec();
                self.handle_remap(&event, &sequence);
            }
            RoutedAction::SwitchMode => self.switch_mode(),
            RoutedAction::Flush => self.flush_pending()?,
            RoutedAction::Ignore => {}
        }

        Ok(())
    }

    fn flush_pending(&mut self) -> Result<(), AppError> {
        if !self.pending_mouse_events.is_empty() {
            self.virtual_mouse.emit_frame(&self.pending_mouse_events)?;
            self.pending_mouse_events.clear();
        }

        self.flush_pending_keyboard()?;
        Ok(())
    }

    fn flush_pending_keyboard(&mut self) -> Result<(), AppError> {
        if !self.pending_keyboard_events.is_empty() {
            self.virtual_keyboard
                .emit_frame(&self.pending_keyboard_events)?;
            self.pending_keyboard_events.clear();
        }

        Ok(())
    }

    fn handle_remap(&mut self, event: &NormalizedMouseEvent, sequence: &[KeyStroke]) {
        match event {
            NormalizedMouseEvent::Button { code, value: 1 } => {
                self.handle_button_press(*code, sequence)
            }
            NormalizedMouseEvent::Button { code, value: 0 } => {
                self.handle_button_release(*code, sequence)
            }
            _ => self.pending_keyboard_events.extend_from_slice(sequence),
        }
    }

    fn handle_button_press(&mut self, input_code: KeyCode, sequence: &[KeyStroke]) {
        let mut tracked_outputs = Vec::new();

        for stroke in sequence {
            match stroke.value {
                1 => match stroke.hold {
                    HoldBehavior::Tap => {
                        self.pending_keyboard_events
                            .push(KeyStroke::press(stroke.key));
                        self.pending_keyboard_events
                            .push(KeyStroke::release(stroke.key));
                    }
                    HoldBehavior::FollowInput(_) => {
                        self.press_output_key(stroke.key);
                        tracked_outputs.push(ActiveButtonOutput {
                            key: stroke.key,
                            hold: stroke.hold,
                        });
                    }
                },
                0 => self.release_output_key(stroke.key),
                _ => {}
            }
        }

        if tracked_outputs.is_empty() {
            self.active_button_outputs.remove(&input_code);
        } else {
            self.active_button_outputs
                .insert(input_code, tracked_outputs);
        }
    }

    fn handle_button_release(&mut self, input_code: KeyCode, sequence: &[KeyStroke]) {
        let active_outputs = self
            .active_button_outputs
            .remove(&input_code)
            .unwrap_or_default();
        let tracked_keys = active_outputs
            .iter()
            .map(|output| output.key)
            .collect::<HashSet<_>>();

        for output in active_outputs {
            self.release_output_for_hold(output.key, output.hold);
        }

        for stroke in sequence {
            match stroke.value {
                0 if !tracked_keys.contains(&stroke.key) => {
                    self.release_output_for_hold(stroke.key, stroke.hold)
                }
                1 => self.press_output_key(stroke.key),
                _ => {}
            }
        }
    }

    fn press_output_key(&mut self, key: KeyCode) {
        let count = self.pressed_output_counts.entry(key).or_insert(0);
        if *count == 0 {
            self.pending_keyboard_events.push(KeyStroke::press(key));
        }
        *count += 1;
    }

    fn release_output_key(&mut self, key: KeyCode) {
        let Some(count) = self.pressed_output_counts.get_mut(&key) else {
            return;
        };

        if *count > 1 {
            *count -= 1;
            return;
        }

        self.pressed_output_counts.remove(&key);
        self.pending_keyboard_events.push(KeyStroke::release(key));
    }

    fn release_output_for_hold(&mut self, key: KeyCode, hold: HoldBehavior) {
        match hold {
            HoldBehavior::Tap => {}
            HoldBehavior::FollowInput(0) => self.release_output_key(key),
            HoldBehavior::FollowInput(milliseconds) => {
                self.scheduled_releases.push(ScheduledRelease {
                    due_at: Instant::now() + Duration::from_millis(milliseconds),
                    key,
                });
            }
        }
    }

    fn release_due_keys(&mut self) -> Result<(), AppError> {
        let now = Instant::now();
        let mut retained = Vec::with_capacity(self.scheduled_releases.len());
        let scheduled_releases = std::mem::take(&mut self.scheduled_releases);

        for scheduled in scheduled_releases {
            if scheduled.due_at <= now {
                self.release_output_key(scheduled.key);
            } else {
                retained.push(scheduled);
            }
        }

        self.scheduled_releases = retained;
        self.flush_pending_keyboard()
    }

    fn reset_keyboard_state(&mut self) -> Result<(), AppError> {
        self.active_button_outputs.clear();
        self.scheduled_releases.clear();

        let keys = self
            .pressed_output_counts
            .keys()
            .copied()
            .collect::<Vec<_>>();
        self.pressed_output_counts.clear();

        for key in keys {
            self.pending_keyboard_events.push(KeyStroke::release(key));
        }

        self.flush_pending_keyboard()
    }

    fn switch_mode(&mut self) {
        if self.config.rules.mode_count() <= 1 {
            return;
        }

        let previous_mode = self
            .config
            .rules
            .current_mode_name(self.active_mode_index)
            .unwrap_or("unknown")
            .to_string();
        self.active_mode_index = self.config.rules.next_mode_index(self.active_mode_index);
        let next_mode = self
            .config
            .rules
            .current_mode_name(self.active_mode_index)
            .unwrap_or("unknown");

        log_info(&format!("mode switched: {previous_mode} -> {next_mode}"));
        notify_mode_change(next_mode);
    }
}

fn report_warnings(warnings: &[ConfigWarning]) {
    for warning in warnings {
        log_warn(&warning.to_string());
    }
}

fn log_info(message: &str) {
    eprintln!("[INFO] {message}");
}

fn log_warn(message: &str) {
    eprintln!("[WARN] {message}");
}

fn resolve_reloaded_mode_index(
    rules: &crate::router::CompiledRules,
    previous_mode_name: Option<String>,
) -> usize {
    previous_mode_name
        .as_deref()
        .and_then(|mode_name| rules.find_mode_index(mode_name))
        .unwrap_or(0)
}

fn notify_mode_change(mode_name: &str) {
    if let Err(err) = Notification::new()
        .summary("mousefold")
        .body(&format!("Mode changed to {mode_name}"))
        .show()
    {
        log_warn(&format!(
            "failed to send desktop notification for mode switch: {err}"
        ));
    }
}
