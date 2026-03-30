use crate::config::{ConfigWarning, LoadResult, has_config_changed, load_config};
use crate::device::{MouseDevice, NormalizedMouseEvent};
use crate::error::AppError;
use crate::router::{KeyStroke, RoutedAction, route};
use crate::virtual_keyboard::VirtualKeyboard;
use crate::virtual_mouse::VirtualMouse;
use clap::Parser;
use std::path::PathBuf;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{Duration, Instant, interval};

#[derive(Debug, Parser)]
#[command(name = "mousemux")]
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
    let mut reload_tick = interval(Duration::from_millis(250));
    let mut last_reload_attempt = Instant::now()
        .checked_sub(Duration::from_millis(runtime.config.reload.debounce_ms))
        .unwrap_or_else(Instant::now);

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
            _ = reload_tick.tick(), if runtime.config.reload.enabled => {
                if has_config_changed(&runtime.config.source_path, runtime.config.source_modified)?.is_none() {
                    continue;
                }

                let now = Instant::now();
                if now.duration_since(last_reload_attempt).as_millis()
                    < u128::from(runtime.config.reload.debounce_ms)
                {
                    continue;
                }
                last_reload_attempt = now;

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
    mouse_device: MouseDevice,
    virtual_mouse: VirtualMouse,
    virtual_keyboard: VirtualKeyboard,
    pending_mouse_events: Vec<NormalizedMouseEvent>,
    pending_keyboard_events: Vec<KeyStroke>,
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
            mouse_device,
            virtual_mouse,
            virtual_keyboard,
            pending_mouse_events: Vec::new(),
            pending_keyboard_events: Vec::new(),
        })
    }

    async fn apply_reload(&mut self) -> Result<(), AppError> {
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

            self.pending_mouse_events.clear();
            self.pending_keyboard_events.clear();
            self.mouse_device = replacement_mouse;
            self.virtual_mouse = replacement_virtual_mouse;
            self.virtual_keyboard = replacement_virtual_keyboard;
            self.config = load_result.config;
            return Ok(());
        }

        self.pending_keyboard_events.clear();
        self.virtual_keyboard = VirtualKeyboard::build(
            load_result.config.rules.registered_keys(),
            self.mouse_device.resolved_name(),
        )?;
        self.config = load_result.config;
        Ok(())
    }

    fn handle_event(&mut self, event: NormalizedMouseEvent) -> Result<(), AppError> {
        match route(&event, &self.config.rules) {
            RoutedAction::PassThrough => self.pending_mouse_events.push(event),
            RoutedAction::Remap(sequence) => {
                self.pending_keyboard_events.extend_from_slice(sequence);
            }
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

        if !self.pending_keyboard_events.is_empty() {
            self.virtual_keyboard
                .emit_frame(&self.pending_keyboard_events)?;
            self.pending_keyboard_events.clear();
        }

        Ok(())
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
