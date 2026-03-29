mod config;

use clap::Parser;
use config::{ActiveConfig, ConfigError, InputKey, load_config};
use evdev::{
    AttributeSet, Device, EventStream, EventSummary, EventType, InputEvent, KeyCode,
    RelativeAxisCode, uinput::VirtualDevice,
};
use std::error::Error;
use std::path::{Path, PathBuf};
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{Duration, Instant, interval};

#[derive(Debug, Parser)]
#[command(name = "ex-g-pro-remapper")]
#[command(about = "Mouse to keyboard remapper daemon")]
struct Cli {
    /// Path to the YAML configuration file
    #[arg(long)]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let config_path = cli.config;
    let (load_result, capabilities, mut virtual_device, mut event_stream) =
        bootstrap(&config_path)?;
    let mut active_config = load_result.config;
    let mut source_capabilities = capabilities;

    for warning in load_result.warnings {
        log_warn(&warning);
    }

    log_info(&format!(
        "started with config={} device={}",
        active_config.source_path.display(),
        active_config.device_path
    ));

    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut reload_tick = interval(Duration::from_millis(250));
    let mut last_reload_attempt = Instant::now()
        .checked_sub(Duration::from_millis(active_config.reload_debounce_ms))
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
            result = event_stream.next_event() => {
                match result {
                    Ok(event) => handle_event(&active_config, &mut virtual_device, event)?,
                    Err(err) => return Err(format!("failed to read input event: {err}").into()),
                }
            }
            _ = reload_tick.tick(), if active_config.reload_enabled => {
                if !should_reload(&active_config.source_path, active_config.source_modified)? {
                    continue;
                }

                let now = Instant::now();
                if now.duration_since(last_reload_attempt).as_millis() < u128::from(active_config.reload_debounce_ms) {
                    continue;
                }
                last_reload_attempt = now;

                match reload_runtime(&active_config).await {
                    Ok(reloaded) => {
                        if reloaded.config.device_path != active_config.device_path {
                            drop(event_stream);
                            let (capabilities, stream) = open_event_stream(&reloaded.config.device_path)?;
                            source_capabilities = capabilities;
                            event_stream = stream;
                        }

                        active_config = reloaded.config;
                        virtual_device = build_virtual_device(&active_config, &source_capabilities)?;

                        for warning in reloaded.warnings {
                            log_warn(&warning);
                        }

                        log_info(&format!(
                            "reloaded config={} device={}",
                            active_config.source_path.display(),
                            active_config.device_path
                        ));
                    }
                    Err(err) => {
                        log_warn(&format!(
                            "reload failed for {}: {err}; keeping previous configuration",
                            active_config.source_path.display()
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

struct ReloadedRuntime {
    config: ActiveConfig,
    warnings: Vec<String>,
}

struct SourceCapabilities {
    supported_keys: AttributeSet<KeyCode>,
    supported_relative_axes: AttributeSet<RelativeAxisCode>,
}

fn bootstrap(
    config_path: &Path,
) -> Result<
    (
        config::LoadResult,
        SourceCapabilities,
        VirtualDevice,
        EventStream,
    ),
    Box<dyn Error>,
> {
    let load_result = load_config(config_path)?;
    let (capabilities, event_stream) = open_event_stream(&load_result.config.device_path)?;
    let virtual_device = build_virtual_device(&load_result.config, &capabilities)?;
    Ok((load_result, capabilities, virtual_device, event_stream))
}

async fn reload_runtime(current: &ActiveConfig) -> Result<ReloadedRuntime, Box<dyn Error>> {
    let load_result = load_config(&current.source_path)?;

    Ok(ReloadedRuntime {
        config: load_result.config,
        warnings: load_result.warnings,
    })
}

fn build_virtual_device(
    config: &ActiveConfig,
    capabilities: &SourceCapabilities,
) -> Result<VirtualDevice, Box<dyn Error>> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for key in capabilities.supported_keys.iter() {
        keys.insert(key);
    }
    for key in &config.registered_keys {
        keys.insert(*key);
    }

    let mut builder = VirtualDevice::builder()?
        .name("ex-g-pro-remapper Virtual Device")
        .with_keys(&keys)?;

    if capabilities.supported_relative_axes.iter().next().is_some() {
        builder = builder.with_relative_axes(&capabilities.supported_relative_axes)?;
    }

    let device = builder.build()?;

    Ok(device)
}

fn open_event_stream(
    device_path: &str,
) -> Result<(SourceCapabilities, EventStream), Box<dyn Error>> {
    let mut device = Device::open(device_path)?;
    let capabilities = read_source_capabilities(&device);
    device.grab()?;
    let stream = device.into_event_stream()?;
    Ok((capabilities, stream))
}

fn handle_event(
    config: &ActiveConfig,
    virtual_device: &mut VirtualDevice,
    event: InputEvent,
) -> Result<(), Box<dyn Error>> {
    match event.destructure() {
        EventSummary::Key(_, code, value) => {
            if let Some(output_events) = config.rules.get(&InputKey { code, value }) {
                let remapped_events = output_events
                    .iter()
                    .map(|output| {
                        InputEvent::new(EventType::KEY.0, output.key.code(), output.value)
                    })
                    .collect::<Vec<_>>();

                virtual_device.emit(&remapped_events)?;
            } else {
                virtual_device.emit(&[event])?;
            }
        }
        EventSummary::RelativeAxis(_, _, _) => {
            virtual_device.emit(&[event])?;
        }
        _ => {}
    }

    Ok(())
}

fn read_source_capabilities(device: &Device) -> SourceCapabilities {
    let mut supported_keys = AttributeSet::<KeyCode>::new();
    if let Some(keys) = device.supported_keys() {
        for key in keys.iter() {
            supported_keys.insert(key);
        }
    }

    let mut supported_relative_axes = AttributeSet::<RelativeAxisCode>::new();
    if let Some(axes) = device.supported_relative_axes() {
        for axis in axes.iter() {
            supported_relative_axes.insert(axis);
        }
    }

    SourceCapabilities {
        supported_keys,
        supported_relative_axes,
    }
}

fn should_reload(
    path: &Path,
    previous_modified: std::time::SystemTime,
) -> Result<bool, ConfigError> {
    let metadata = std::fs::metadata(path)?;
    let modified = metadata.modified()?;
    Ok(modified > previous_modified)
}

fn log_info(message: &str) {
    eprintln!("[INFO] {message}");
}

fn log_warn(message: &str) {
    eprintln!("[WARN] {message}");
}
