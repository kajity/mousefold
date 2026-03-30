use crate::router::KeyStroke;
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, uinput::VirtualDevice};
use std::fmt;

#[derive(Debug)]
pub struct VirtualKeyboard {
    device: VirtualDevice,
}

#[derive(Debug)]
pub enum VirtualKeyboardError {
    Io(std::io::Error),
}

impl fmt::Display for VirtualKeyboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "virtual keyboard error: {err}"),
        }
    }
}

impl std::error::Error for VirtualKeyboardError {}

impl From<std::io::Error> for VirtualKeyboardError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl VirtualKeyboard {
    /// Builds a virtual keyboard from the keys referenced by compiled rules.
    pub fn build(keys: &[KeyCode], name: &str) -> Result<Self, VirtualKeyboardError> {
        let mut attribute_set = AttributeSet::<KeyCode>::new();
        for key in keys {
            attribute_set.insert(*key);
        }

        Ok(Self {
            device: VirtualDevice::builder()?
                .name(&format!("mousemux Virtual Keyboard for {}", name))
                .with_keys(&attribute_set)?
                .build()?,
        })
    }

    /// Emits a keyboard sequence as one synchronized frame.
    pub fn emit_frame(&mut self, events: &[KeyStroke]) -> Result<(), VirtualKeyboardError> {
        if events.is_empty() {
            return Ok(());
        }

        let input_events = events
            .iter()
            .map(|event| InputEvent::new_now(EventType::KEY.0, event.key.code(), event.value))
            .collect::<Vec<_>>();
        self.device.emit(&input_events)?;
        Ok(())
    }
}
