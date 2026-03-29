use evdev::KeyCode;
use serde::Deserialize;
use serde::de::{self, Deserializer};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct ActiveConfig {
    pub source_path: PathBuf,
    pub source_modified: SystemTime,
    pub device_path: String,
    pub reload_enabled: bool,
    pub reload_debounce_ms: u64,
    pub rules: HashMap<InputKey, Vec<OutputKeyEvent>>,
    pub registered_keys: Vec<KeyCode>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InputKey {
    pub code: KeyCode,
    pub value: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputKeyEvent {
    pub key: KeyCode,
    pub value: i32,
}

#[derive(Debug)]
pub struct LoadResult {
    pub config: ActiveConfig,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(serde_yaml::Error),
    Validation(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Parse(err) => write!(f, "YAML parse error: {err}"),
            Self::Validation(err) => write!(f, "validation error: {err}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_yaml::Error> for ConfigError {
    fn from(value: serde_yaml::Error) -> Self {
        Self::Parse(value)
    }
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    device: DeviceConfig,
    #[serde(default)]
    reload: ReloadConfig,
    remaps: Vec<RemapRule>,
}

#[derive(Debug, Deserialize)]
struct DeviceConfig {
    path: String,
}

#[derive(Debug, Deserialize)]
struct ReloadConfig {
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_reload_debounce_ms")]
    debounce_ms: u64,
}

impl Default for ReloadConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            debounce_ms: default_reload_debounce_ms(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct RemapRule {
    description: Option<String>,
    input: InputCondition,
    output: Vec<OutputKeyEventSerde>,
}

#[derive(Debug, Deserialize)]
struct InputCondition {
    #[serde(rename = "type")]
    event_type: InputType,
    #[serde(deserialize_with = "deserialize_key_code")]
    code: KeyCode,
    value: i32,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum InputType {
    Key,
}

#[derive(Debug, Deserialize)]
struct OutputKeyEventSerde {
    #[serde(deserialize_with = "deserialize_key_code")]
    key: KeyCode,
    value: i32,
}

pub fn load_config(path: &Path) -> Result<LoadResult, ConfigError> {
    let content = fs::read_to_string(path)?;
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    let parsed: ConfigFile = serde_yaml::from_str(&content)?;
    validate_config(&parsed)?;

    let mut warnings = Vec::new();
    let mut rules = HashMap::new();
    let mut registered_keys = HashSet::new();

    for (index, rule) in parsed.remaps.iter().enumerate() {
        let input = InputKey {
            code: rule.input.code,
            value: rule.input.value,
        };

        let output = rule
            .output
            .iter()
            .map(|event| {
                registered_keys.insert(event.key);
                OutputKeyEvent {
                    key: event.key,
                    value: event.value,
                }
            })
            .collect::<Vec<_>>();

        if let Some(previous) = rules.insert(input, output) {
            let description = rule.description.as_deref().unwrap_or("<no description>");
            warnings.push(format!(
                "conflicting remap for {:?}/{} at remaps[{}] ({description}); later rule overrides earlier rule with {} output event(s)",
                input.code,
                input.value,
                index,
                previous.len()
            ));
        }
    }

    let mut registered_keys = registered_keys.into_iter().collect::<Vec<_>>();
    registered_keys.sort_unstable_by_key(|key| key.code());

    Ok(LoadResult {
        config: ActiveConfig {
            source_path: path.to_path_buf(),
            source_modified: modified,
            device_path: parsed.device.path,
            reload_enabled: parsed.reload.enabled,
            reload_debounce_ms: parsed.reload.debounce_ms,
            rules,
            registered_keys,
        },
        warnings,
    })
}

fn validate_config(config: &ConfigFile) -> Result<(), ConfigError> {
    if config.device.path.trim().is_empty() {
        return Err(ConfigError::Validation(
            "device.path must not be empty".to_string(),
        ));
    }

    if config.remaps.is_empty() {
        return Err(ConfigError::Validation(
            "remaps must contain at least one rule".to_string(),
        ));
    }

    for (index, rule) in config.remaps.iter().enumerate() {
        if rule.input.event_type != InputType::Key {
            return Err(ConfigError::Validation(format!(
                "remaps[{index}].input.type must be \"key\""
            )));
        }

        if rule.output.is_empty() {
            return Err(ConfigError::Validation(format!(
                "remaps[{index}].output must contain at least one key event"
            )));
        }
    }

    Ok(())
}

fn default_true() -> bool {
    true
}

fn default_reload_debounce_ms() -> u64 {
    250
}

fn deserialize_key_code<'de, D>(deserializer: D) -> Result<KeyCode, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum KeyCodeValue {
        Name(String),
        Number(u16),
    }

    match KeyCodeValue::deserialize(deserializer)? {
        KeyCodeValue::Name(value) => KeyCode::from_str(&value)
            .map_err(|_| de::Error::custom(format!("unknown key code: {value}"))),
        KeyCodeValue::Number(value) => Ok(KeyCode::new(value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn last_rule_wins_on_conflict() {
        let path = PathBuf::from("/tmp/test.yaml");
        let modified = UNIX_EPOCH;
        let parsed = ConfigFile {
            device: DeviceConfig {
                path: "/dev/input/event0".to_string(),
            },
            reload: ReloadConfig::default(),
            remaps: vec![
                RemapRule {
                    description: Some("first".to_string()),
                    input: InputCondition {
                        event_type: InputType::Key,
                        code: KeyCode::BTN_RIGHT,
                        value: 1,
                    },
                    output: vec![OutputKeyEventSerde {
                        key: KeyCode::KEY_A,
                        value: 1,
                    }],
                },
                RemapRule {
                    description: Some("second".to_string()),
                    input: InputCondition {
                        event_type: InputType::Key,
                        code: KeyCode::BTN_RIGHT,
                        value: 1,
                    },
                    output: vec![OutputKeyEventSerde {
                        key: KeyCode::KEY_B,
                        value: 1,
                    }],
                },
            ],
        };

        validate_config(&parsed).unwrap();

        let mut rules = HashMap::new();
        for rule in &parsed.remaps {
            rules.insert(
                InputKey {
                    code: rule.input.code,
                    value: rule.input.value,
                },
                rule.output
                    .iter()
                    .map(|event| OutputKeyEvent {
                        key: event.key,
                        value: event.value,
                    })
                    .collect::<Vec<_>>(),
            );
        }

        let config = ActiveConfig {
            source_path: path,
            source_modified: modified,
            device_path: parsed.device.path,
            reload_enabled: true,
            reload_debounce_ms: 250,
            rules,
            registered_keys: vec![KeyCode::KEY_B],
        };

        let output = config
            .rules
            .get(&InputKey {
                code: KeyCode::BTN_RIGHT,
                value: 1,
            })
            .unwrap();
        assert_eq!(output[0].key, KeyCode::KEY_B);
    }

    #[test]
    fn deserialize_named_key_code() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(deserialize_with = "deserialize_key_code")]
            code: KeyCode,
        }

        let parsed: Wrapper = serde_yaml::from_str("code: BTN_RIGHT").unwrap();
        assert_eq!(parsed.code, KeyCode::BTN_RIGHT);
    }

    #[test]
    fn deserialize_numeric_key_code() {
        #[derive(Deserialize)]
        struct Wrapper {
            #[serde(deserialize_with = "deserialize_key_code")]
            code: KeyCode,
        }

        let parsed: Wrapper = serde_yaml::from_str("code: 278").unwrap();
        assert_eq!(parsed.code, KeyCode::new(278));
    }
}
