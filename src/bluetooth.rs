use crate::config::BluetoothConfig;
use bluer::{Address, Device as BluerDevice, Error as BluerError, Session};
use log::{debug, info, warn};
use std::fmt;
use tokio::time::{Duration, sleep};

const DISCOVERY_RETRY_ATTEMPTS: usize = 10;
const DISCOVERY_RETRY_DELAY: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum BluetoothError {
    Bluez(BluerError),
    NoAdapters,
    DeviceNotFound { name: String },
    Pair { name: String, source: BluerError },
    Trust { name: String, source: BluerError },
    Connect { name: String, source: BluerError },
}

impl fmt::Display for BluetoothError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bluez(err) => write!(f, "bluetooth error: {err}"),
            Self::NoAdapters => write!(f, "no bluetooth adapters are available"),
            Self::DeviceNotFound { name } => {
                write!(f, "bluetooth device named \"{name}\" was not found")
            }
            Self::Pair { name, source } => {
                write!(f, "failed to pair bluetooth device \"{name}\": {source}")
            }
            Self::Trust { name, source } => {
                write!(f, "failed to trust bluetooth device \"{name}\": {source}")
            }
            Self::Connect { name, source } => {
                write!(f, "failed to connect bluetooth device \"{name}\": {source}")
            }
        }
    }
}

impl std::error::Error for BluetoothError {}

impl From<BluerError> for BluetoothError {
    fn from(value: BluerError) -> Self {
        Self::Bluez(value)
    }
}

#[derive(Clone, Debug)]
struct CandidateDevice {
    adapter_name: String,
    address: Address,
    connected: bool,
}

/// Ensures a configured Bluetooth mouse is connected before evdev device resolution begins.
pub async fn ensure_connected(
    device_name: &str,
    config: &BluetoothConfig,
) -> Result<(), BluetoothError> {
    let (session, candidate) = resolve_candidate_with_retry(device_name).await?;
    let adapter = session.adapter(&candidate.adapter_name)?;
    let device = adapter.device(candidate.address)?;

    debug!(
        "selected bluetooth device name={} adapter={} address={} connected={}",
        device_name, candidate.adapter_name, candidate.address, candidate.connected
    );

    if config.auto_pair && !device.is_paired().await? {
        device.pair().await.map_err(|source| BluetoothError::Pair {
            name: device_name.to_string(),
            source,
        })?;
        info!("paired bluetooth device {}", device_name);
    }

    if config.auto_trust && !device.is_trusted().await? {
        device
            .set_trusted(true)
            .await
            .map_err(|source| BluetoothError::Trust {
                name: device_name.to_string(),
                source,
            })?;
        info!("trusted bluetooth device {}", device_name);
    }

    if config.auto_connect && !device.is_connected().await? {
        device
            .connect()
            .await
            .map_err(|source| BluetoothError::Connect {
                name: device_name.to_string(),
                source,
            })?;
        info!("connected bluetooth device {}", device_name);
        std::thread::sleep(Duration::from_secs(2));
    }

    Ok(())
}

async fn resolve_candidate_with_retry(
    device_name: &str,
) -> Result<(Session, CandidateDevice), BluetoothError> {
    for attempt in 1..=DISCOVERY_RETRY_ATTEMPTS {
        match try_resolve_candidate(device_name).await {
            Ok(candidate) => return Ok(candidate),
            Err(err) if should_retry_discovery(&err) && attempt < DISCOVERY_RETRY_ATTEMPTS => {
                warn!(
                    "bluetooth discovery attempt {attempt}/{} for {} failed: {err}; retrying in {}s",
                    DISCOVERY_RETRY_ATTEMPTS,
                    device_name,
                    DISCOVERY_RETRY_DELAY.as_secs()
                );
                sleep(DISCOVERY_RETRY_DELAY).await;
            }
            Err(err) => return Err(err),
        }
    }

    Err(BluetoothError::DeviceNotFound {
        name: device_name.to_string(),
    })
}

async fn try_resolve_candidate(
    device_name: &str,
) -> Result<(Session, CandidateDevice), BluetoothError> {
    let session = Session::new().await?;
    let adapter_names = session.adapter_names().await?;
    if adapter_names.is_empty() {
        return Err(BluetoothError::NoAdapters);
    }

    let candidate = find_candidate(&session, &adapter_names, device_name).await?;
    Ok((session, candidate))
}

fn should_retry_discovery(err: &BluetoothError) -> bool {
    matches!(
        err,
        BluetoothError::NoAdapters | BluetoothError::DeviceNotFound { .. }
    )
}

async fn find_candidate(
    session: &Session,
    adapter_names: &[String],
    device_name: &str,
) -> Result<CandidateDevice, BluetoothError> {
    let mut connected_candidate = None;
    let mut disconnected_candidate = None;

    for adapter_name in adapter_names {
        let adapter = session.adapter(adapter_name)?;
        for address in adapter.device_addresses().await? {
            let device = adapter.device(address)?;
            if !device_matches_name(&device, device_name).await? {
                continue;
            }

            let candidate = CandidateDevice {
                adapter_name: adapter_name.clone(),
                address,
                connected: device.is_connected().await?,
            };

            if candidate.connected {
                connected_candidate.get_or_insert(candidate);
            } else {
                disconnected_candidate.get_or_insert(candidate);
            }
        }
    }

    disconnected_candidate
        .or(connected_candidate)
        .ok_or_else(|| BluetoothError::DeviceNotFound {
            name: device_name.to_string(),
        })
}

async fn device_matches_name(
    device: &BluerDevice,
    expected_name: &str,
) -> Result<bool, BluetoothError> {
    let alias = device.alias().await?;
    if alias == expected_name {
        return Ok(true);
    }

    Ok(device
        .name()
        .await?
        .is_some_and(|name| name == expected_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_discovery_errors_are_identified() {
        assert!(should_retry_discovery(&BluetoothError::NoAdapters));
        assert!(should_retry_discovery(&BluetoothError::DeviceNotFound {
            name: "Example".to_string(),
        }));
    }
}
