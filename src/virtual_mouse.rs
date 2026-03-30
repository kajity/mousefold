use crate::device::{NormalizedMouseEvent, SourceMouseCapabilities};
use evdev::{AttributeSet, InputEvent, KeyCode, uinput::VirtualDevice};
use std::fmt;

#[derive(Debug)]
pub struct VirtualMouse {
    device: VirtualDevice,
}

#[derive(Debug)]
pub enum VirtualMouseError {
    Io(std::io::Error),
}

impl fmt::Display for VirtualMouseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "virtual mouse error: {err}"),
        }
    }
}

impl std::error::Error for VirtualMouseError {}

impl From<std::io::Error> for VirtualMouseError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl VirtualMouse {
    /// Builds a virtual mouse from source device capabilities.
    pub fn build_from_source_caps(
        caps: &SourceMouseCapabilities,
        name: &str,
    ) -> Result<Self, VirtualMouseError> {
        let mut button_keys = AttributeSet::<KeyCode>::new();
        for key in caps.supported_keys().iter().filter(|key| key.0 >= 0x110) {
            button_keys.insert(key);
        }

        let device_name = format!("mousemux Virtual Mouse for {}", name);
        let mut builder = VirtualDevice::builder()?
            .name(&device_name)
            .with_relative_axes(caps.supported_relative_axes())?;

        if button_keys.iter().next().is_some() {
            builder = builder.with_keys(&button_keys)?;
        }

        Ok(Self {
            device: builder.build()?,
        })
    }

    /// Emits passthrough mouse events as one frame.
    pub fn emit_frame(&mut self, events: &[NormalizedMouseEvent]) -> Result<(), VirtualMouseError> {
        let input_events = events
            .iter()
            .filter_map(|event| (*event).to_input_event())
            .collect::<Vec<InputEvent>>();

        if input_events.is_empty() {
            return Ok(());
        }

        self.device.emit(&input_events)?;
        Ok(())
    }
}
