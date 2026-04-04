use crate::config::DeviceSelector;
use evdev::{
    AttributeSet, Device, EventStream, EventSummary, InputEvent, KeyCode, RelativeAxisCode,
};
use log::warn;
use std::fmt;
use std::path::{Path, PathBuf};
use tokio::time::{Duration, sleep};

const DEVICE_RESOLVE_RETRY_ATTEMPTS: usize = 10;
const DEVICE_RESOLVE_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Clone, Debug)]
pub struct SourceMouseCapabilities {
    supported_keys: AttributeSet<KeyCode>,
    supported_relative_axes: AttributeSet<RelativeAxisCode>,
}

impl SourceMouseCapabilities {
    /// Returns key capabilities from the source mouse.
    pub fn supported_keys(&self) -> &AttributeSet<KeyCode> {
        &self.supported_keys
    }

    /// Returns relative-axis capabilities from the source mouse.
    pub fn supported_relative_axes(&self) -> &AttributeSet<RelativeAxisCode> {
        &self.supported_relative_axes
    }
}

pub struct MouseDevice {
    event_stream: EventStream,
    source_capabilities: SourceMouseCapabilities,
    resolved_path: PathBuf,
    resolved_name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalizedMouseEvent {
    Button { code: KeyCode, value: i32 },
    Relative { code: RelativeAxisCode, value: i32 },
    SyncReport,
    OtherIgnored,
}

impl NormalizedMouseEvent {
    /// Converts a passthrough-safe event back into an evdev event.
    pub fn to_input_event(self) -> Option<InputEvent> {
        match self {
            Self::Button { code, value } => {
                Some(InputEvent::new_now(evdev::EventType::KEY.0, code.0, value))
            }
            Self::Relative { code, value } => Some(InputEvent::new_now(
                evdev::EventType::RELATIVE.0,
                code.0,
                value,
            )),
            Self::SyncReport | Self::OtherIgnored => None,
        }
    }
}

#[derive(Debug)]
pub enum DeviceError {
    Io(std::io::Error),
    NotFound { selector: String },
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "device I/O error: {err}"),
            Self::NotFound { selector } => {
                write!(f, "failed to resolve input device for {selector}")
            }
        }
    }
}

impl std::error::Error for DeviceError {}

impl From<std::io::Error> for DeviceError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl MouseDevice {
    /// Waits for a selector to become resolvable before opening the device.
    pub async fn wait_until_available(selector: &DeviceSelector) -> Result<(), DeviceError> {
        for attempt in 1..=DEVICE_RESOLVE_RETRY_ATTEMPTS {
            match resolve_device(selector) {
                Ok(_) => return Ok(()),
                Err(err) if should_retry_resolution(selector, &err) => {
                    if attempt == DEVICE_RESOLVE_RETRY_ATTEMPTS {
                        return Err(err);
                    }

                    warn!(
                        "device resolve attempt {attempt}/{} for {} failed: {err}; retrying in {}s",
                        DEVICE_RESOLVE_RETRY_ATTEMPTS,
                        selector.describe(),
                        DEVICE_RESOLVE_RETRY_DELAY.as_secs()
                    );
                    sleep(DEVICE_RESOLVE_RETRY_DELAY).await;
                }
                Err(err) => return Err(err),
            }
        }

        Err(DeviceError::NotFound {
            selector: selector.describe(),
        })
    }

    /// Opens the configured mouse, captures its capabilities, and grabs the device.
    pub fn open_and_grab(selector: &DeviceSelector) -> Result<Self, DeviceError> {
        Self::open(selector, true)
    }

    /// Opens the configured mouse without grabbing it for monitor mode.
    pub fn open_for_monitor(selector: &DeviceSelector) -> Result<Self, DeviceError> {
        Self::open(selector, false)
    }

    fn open(selector: &DeviceSelector, grab: bool) -> Result<Self, DeviceError> {
        let (resolved_path, mut device) = resolve_device(selector)?;
        let resolved_name = device.name().unwrap_or("unknown-device").to_string();
        let source_capabilities = read_source_capabilities(&device);
        if grab {
            device.grab()?;
        }
        let event_stream = device.into_event_stream()?;

        Ok(Self {
            event_stream,
            source_capabilities,
            resolved_path,
            resolved_name,
        })
    }

    /// Returns source device capabilities for virtual-device setup.
    pub fn source_capabilities(&self) -> &SourceMouseCapabilities {
        &self.source_capabilities
    }

    /// Returns the resolved device path currently being monitored.
    pub fn resolved_path(&self) -> &Path {
        &self.resolved_path
    }

    /// Returns the resolved device name currently being monitored.
    pub fn resolved_name(&self) -> &str {
        &self.resolved_name
    }

    /// Awaits the next normalized event from the grabbed device.
    pub async fn next_event(&mut self) -> Result<NormalizedMouseEvent, DeviceError> {
        let event = self.event_stream.next_event().await?;
        Ok(match event.destructure() {
            EventSummary::Key(_, code, value) => NormalizedMouseEvent::Button { code, value },
            EventSummary::RelativeAxis(_, code, value) => {
                NormalizedMouseEvent::Relative { code, value }
            }
            EventSummary::Synchronization(_, evdev::SynchronizationCode::SYN_REPORT, _) => {
                NormalizedMouseEvent::SyncReport
            }
            _ => NormalizedMouseEvent::OtherIgnored,
        })
    }
}

fn resolve_device(selector: &DeviceSelector) -> Result<(PathBuf, Device), DeviceError> {
    match selector {
        DeviceSelector::Path(path) | DeviceSelector::ById(path) => {
            let device = Device::open(path)?;
            Ok((path.clone(), device))
        }
        DeviceSelector::Name(expected_name) => evdev::enumerate()
            .find(|(_, device)| device.name().is_some_and(|name| name == expected_name))
            .ok_or_else(|| DeviceError::NotFound {
                selector: format!("device.name={expected_name}"),
            }),
    }
}

fn should_retry_resolution(selector: &DeviceSelector, err: &DeviceError) -> bool {
    matches!(
        (selector, err),
        (DeviceSelector::Name(_), DeviceError::NotFound { .. })
    )
}

fn read_source_capabilities(device: &Device) -> SourceMouseCapabilities {
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

    SourceMouseCapabilities {
        supported_keys,
        supported_relative_axes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_selector_not_found_is_retryable() {
        let selector = DeviceSelector::Name("Example Mouse".to_string());
        let err = DeviceError::NotFound {
            selector: selector.describe(),
        };

        assert!(should_retry_resolution(&selector, &err));
    }

    #[test]
    fn path_selector_not_found_is_not_retryable() {
        let selector = DeviceSelector::Path(PathBuf::from("/dev/input/event999"));
        let err = DeviceError::NotFound {
            selector: selector.describe(),
        };

        assert!(!should_retry_resolution(&selector, &err));
    }
}
