use crate::bluetooth::BluetoothError;
use crate::config::ConfigError;
use crate::device::DeviceError;
use crate::virtual_keyboard::VirtualKeyboardError;
use crate::virtual_mouse::VirtualMouseError;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum AppError {
    Cli(String),
    Logger(log::SetLoggerError),
    Config(ConfigError),
    Bluetooth(BluetoothError),
    Device(DeviceError),
    Mouse(VirtualMouseError),
    Keyboard(VirtualKeyboardError),
    Signal(std::io::Error),
    PidFile {
        path: PathBuf,
        source: std::io::Error,
    },
    PidFileFormat {
        path: PathBuf,
        content: String,
    },
    SignalSend {
        pid: i32,
        source: std::io::Error,
    },
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cli(message) => write!(f, "{message}"),
            Self::Logger(err) => write!(f, "failed to initialize logger: {err}"),
            Self::Config(err) => write!(f, "{err}"),
            Self::Bluetooth(err) => write!(f, "{err}"),
            Self::Device(err) => write!(f, "{err}"),
            Self::Mouse(err) => write!(f, "{err}"),
            Self::Keyboard(err) => write!(f, "{err}"),
            Self::Signal(err) => write!(f, "failed to install signal handler: {err}"),
            Self::PidFile { path, source } => {
                write!(f, "failed to access pid file {}: {source}", path.display())
            }
            Self::PidFileFormat { path, content } => write!(
                f,
                "failed to parse pid file {}: invalid pid content {:?}",
                path.display(),
                content
            ),
            Self::SignalSend { pid, source } => {
                write!(f, "failed to send reload signal to pid {pid}: {source}")
            }
        }
    }
}

impl std::error::Error for AppError {}

impl From<ConfigError> for AppError {
    fn from(value: ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<DeviceError> for AppError {
    fn from(value: DeviceError) -> Self {
        Self::Device(value)
    }
}

impl From<BluetoothError> for AppError {
    fn from(value: BluetoothError) -> Self {
        Self::Bluetooth(value)
    }
}

impl From<VirtualMouseError> for AppError {
    fn from(value: VirtualMouseError) -> Self {
        Self::Mouse(value)
    }
}

impl From<VirtualKeyboardError> for AppError {
    fn from(value: VirtualKeyboardError) -> Self {
        Self::Keyboard(value)
    }
}

impl From<log::SetLoggerError> for AppError {
    fn from(value: log::SetLoggerError) -> Self {
        Self::Logger(value)
    }
}
