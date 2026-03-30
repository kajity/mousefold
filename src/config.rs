use crate::router::{CompiledRules, KeyStroke, MouseButtonTrigger};
use evdev::KeyCode;
use serde::Deserialize;
use serde::de::{self, Deserializer};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct ActiveConfig {
    pub source_path: PathBuf,
    pub source_modified: SystemTime,
    pub device_selector: DeviceSelector,
    pub reload: ReloadConfig,
    pub rules: CompiledRules,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceSelector {
    Path(PathBuf),
    ById(PathBuf),
    Name(String),
}

impl DeviceSelector {
    /// Returns a human-readable selector for logs and diagnostics.
    pub fn describe(&self) -> String {
        match self {
            Self::Path(path) => format!("device.path={}", path.display()),
            Self::ById(path) => format!("device.by_id={}", path.display()),
            Self::Name(name) => format!("device.name={name}"),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReloadConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_reload_debounce_ms")]
    pub debounce_ms: u64,
}

impl Default for ReloadConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            debounce_ms: default_reload_debounce_ms(),
        }
    }
}

#[derive(Debug)]
pub struct LoadResult {
    pub config: ActiveConfig,
    pub warnings: Vec<ConfigWarning>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigWarning {
    ShadowedRule {
        input: String,
        preferred_index: usize,
        shadowed_index: usize,
        preferred_description: Option<String>,
        shadowed_description: Option<String>,
    },
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShadowedRule {
                input,
                preferred_index,
                shadowed_index,
                preferred_description,
                shadowed_description,
            } => write!(
                f,
                "conflicting remap for {input}: remaps[{preferred_index}] ({}) overrides remaps[{shadowed_index}] ({})",
                preferred_description.as_deref().unwrap_or("no description"),
                shadowed_description.as_deref().unwrap_or("no description")
            ),
        }
    }
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
            Self::Io(err) => write!(f, "failed to read config: {err}"),
            Self::Parse(err) => write!(f, "YAML parse error: {err}"),
            Self::Validation(err) => write!(f, "config validation failed: {err}"),
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
    device: RawDeviceConfig,
    #[serde(default)]
    reload: ReloadConfig,
    remaps: Vec<RemapRule>,
}

#[derive(Debug, Deserialize)]
struct RawDeviceConfig {
    path: Option<PathBuf>,
    by_id: Option<PathBuf>,
    name: Option<String>,
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

    let mut rules = HashMap::new();
    let mut warnings = Vec::new();

    for (index, rule) in parsed.remaps.iter().enumerate() {
        let input = MouseButtonTrigger {
            code: rule.input.code,
            value: rule.input.value,
        };
        let output = rule
            .output
            .iter()
            .map(|event| KeyStroke {
                key: event.key,
                value: event.value,
            })
            .collect::<Vec<_>>();

        if let Some((shadowed_index, shadowed_description, _)) =
            rules.insert(input, (index, rule.description.clone(), output))
        {
            warnings.push(ConfigWarning::ShadowedRule {
                input: describe_input(input),
                preferred_index: index,
                shadowed_index,
                preferred_description: rule.description.clone(),
                shadowed_description,
            });
        }
    }

    let compiled_rules = CompiledRules::new(
        rules
            .into_iter()
            .map(|(trigger, (_, _, output))| (trigger, output))
            .collect(),
    );

    Ok(LoadResult {
        config: ActiveConfig {
            source_path: path.to_path_buf(),
            source_modified: modified,
            device_selector: into_selector(parsed.device)?,
            reload: parsed.reload,
            rules: compiled_rules,
        },
        warnings,
    })
}

pub fn has_config_changed(
    path: &Path,
    previous_modified: SystemTime,
) -> Result<Option<SystemTime>, ConfigError> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    if modified > previous_modified {
        Ok(Some(modified))
    } else {
        Ok(None)
    }
}

fn validate_config(config: &ConfigFile) -> Result<(), ConfigError> {
    validate_device_config(&config.device)?;

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

        if !is_supported_button_code(rule.input.code) {
            return Err(ConfigError::Validation(format!(
                "remaps[{index}].input.code must be a supported mouse button code, got {:?}",
                rule.input.code
            )));
        }

        if !matches!(rule.input.value, 0 | 1) {
            return Err(ConfigError::Validation(format!(
                "remaps[{index}].input.value must be 0 or 1"
            )));
        }

        if rule.output.is_empty() {
            return Err(ConfigError::Validation(format!(
                "remaps[{index}].output must contain at least one key event"
            )));
        }

        for (output_index, output) in rule.output.iter().enumerate() {
            if !matches!(output.value, 0 | 1) {
                return Err(ConfigError::Validation(format!(
                    "remaps[{index}].output[{output_index}].value must be 0 or 1"
                )));
            }
        }
    }

    Ok(())
}

fn validate_device_config(device: &RawDeviceConfig) -> Result<(), ConfigError> {
    let selector_count = usize::from(device.path.is_some())
        + usize::from(device.by_id.is_some())
        + usize::from(device.name.is_some());

    if selector_count != 1 {
        return Err(ConfigError::Validation(
            "device must specify exactly one of path, by_id, or name".to_string(),
        ));
    }

    if let Some(path) = &device.path
        && path.as_os_str().is_empty()
    {
        return Err(ConfigError::Validation(
            "device.path must not be empty".to_string(),
        ));
    }

    if let Some(path) = &device.by_id
        && path.as_os_str().is_empty()
    {
        return Err(ConfigError::Validation(
            "device.by_id must not be empty".to_string(),
        ));
    }

    if let Some(name) = &device.name
        && name.trim().is_empty()
    {
        return Err(ConfigError::Validation(
            "device.name must not be empty".to_string(),
        ));
    }

    Ok(())
}

fn into_selector(raw: RawDeviceConfig) -> Result<DeviceSelector, ConfigError> {
    match (raw.path, raw.by_id, raw.name) {
        (Some(path), None, None) => Ok(DeviceSelector::Path(path)),
        (None, Some(path), None) => Ok(DeviceSelector::ById(path)),
        (None, None, Some(name)) => Ok(DeviceSelector::Name(name)),
        _ => Err(ConfigError::Validation(
            "device must specify exactly one of path, by_id, or name".to_string(),
        )),
    }
}

fn describe_input(input: MouseButtonTrigger) -> String {
    format!("{:?}/{}", input.code, input.value)
}

fn is_supported_button_code(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::BTN_LEFT
            | KeyCode::BTN_RIGHT
            | KeyCode::BTN_MIDDLE
            | KeyCode::BTN_SIDE
            | KeyCode::BTN_EXTRA
            | KeyCode::BTN_FORWARD
            | KeyCode::BTN_BACK
            | KeyCode::BTN_TASK
    )
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
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn config_requires_exactly_one_device_selector() {
        let parsed = ConfigFile {
            device: RawDeviceConfig {
                path: Some(PathBuf::from("/dev/input/event0")),
                by_id: Some(PathBuf::from("/dev/input/by-id/test")),
                name: None,
            },
            reload: ReloadConfig::default(),
            remaps: vec![sample_rule(KeyCode::BTN_RIGHT, 1, KeyCode::KEY_A, 1)],
        };

        let err = validate_config(&parsed).unwrap_err();
        assert!(
            err.to_string()
                .contains("device must specify exactly one of path, by_id, or name")
        );
    }

    #[test]
    fn last_rule_wins_on_conflict() {
        let tmp_path = std::env::temp_dir().join(format!(
            "mousemux-config-{}.yaml",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        fs::write(
            &tmp_path,
            r#"
device:
  path: /dev/input/event0
remaps:
  - input:
      type: key
      code: BTN_RIGHT
      value: 1
    output:
      - key: KEY_A
        value: 1
  - input:
      type: key
      code: BTN_RIGHT
      value: 1
    output:
      - key: KEY_B
        value: 1
"#,
        )
        .unwrap();

        let result = load_config(&tmp_path).unwrap();
        let action = crate::router::route(
            &crate::device::NormalizedMouseEvent::Button {
                code: KeyCode::BTN_RIGHT,
                value: 1,
            },
            &result.config.rules,
        );

        assert_eq!(result.warnings.len(), 1);
        match action {
            crate::router::RoutedAction::Remap(sequence) => {
                assert_eq!(sequence[0].key, KeyCode::KEY_B);
            }
            _ => panic!("expected remap"),
        }

        let _ = fs::remove_file(tmp_path);
    }

    #[test]
    fn invalid_output_value_is_rejected() {
        let parsed = ConfigFile {
            device: RawDeviceConfig {
                path: Some(PathBuf::from("/dev/input/event0")),
                by_id: None,
                name: None,
            },
            reload: ReloadConfig::default(),
            remaps: vec![RemapRule {
                description: None,
                input: InputCondition {
                    event_type: InputType::Key,
                    code: KeyCode::BTN_RIGHT,
                    value: 1,
                },
                output: vec![OutputKeyEventSerde {
                    key: KeyCode::KEY_LEFTMETA,
                    value: 2,
                }],
            }],
        };

        let err = validate_config(&parsed).unwrap_err();
        assert!(err.to_string().contains("output[0].value must be 0 or 1"));
    }

    fn sample_rule(
        input_code: KeyCode,
        input_value: i32,
        output_key: KeyCode,
        output_value: i32,
    ) -> RemapRule {
        RemapRule {
            description: None,
            input: InputCondition {
                event_type: InputType::Key,
                code: input_code,
                value: input_value,
            },
            output: vec![OutputKeyEventSerde {
                key: output_key,
                value: output_value,
            }],
        }
    }
}
