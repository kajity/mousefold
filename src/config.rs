use crate::router::{
    CompiledRules, CompiledSwitchMode, HoldBehavior, KeyStroke, ModeBindings, MouseButtonTrigger,
};
use evdev::KeyCode;
use serde::Deserialize;
use serde::de::{self, Deserializer};
use serde_yaml::Mapping;
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
        mode_name: String,
        input: String,
        preferred_rule: String,
        shadowed_rule: String,
        preferred_description: Option<String>,
        shadowed_description: Option<String>,
    },
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ShadowedRule {
                mode_name,
                input,
                preferred_rule,
                shadowed_rule,
                preferred_description,
                shadowed_description,
            } => write!(
                f,
                "conflicting remap for {input} in mode {mode_name}: remap \"{preferred_rule}\" ({}) overrides \"{shadowed_rule}\" ({})",
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
    remaps: Mapping,
    #[serde(default)]
    mode_switches: Option<ModeSwitchesConfig>,
}

#[derive(Debug, Deserialize)]
struct RawDeviceConfig {
    path: Option<PathBuf>,
    by_id: Option<PathBuf>,
    name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct RemapRule {
    description: Option<String>,
    input: InputCondition,
    output: Vec<OutputKeyEventSerde>,
}

#[derive(Clone, Debug, Deserialize)]
struct InputCondition {
    #[serde(rename = "type")]
    event_type: InputType,
    #[serde(deserialize_with = "deserialize_key_code")]
    code: KeyCode,
    #[serde(default)]
    value: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum InputType {
    Key,
}

#[derive(Clone, Debug, Deserialize)]
struct OutputKeyEventSerde {
    #[serde(deserialize_with = "deserialize_key_code")]
    key: KeyCode,
    #[serde(default)]
    value: Option<i32>,
    #[serde(default)]
    hold: HoldBehavior,
}

#[derive(Debug, Deserialize)]
struct ModeSwitchesConfig {
    modes: Vec<ModeConfig>,
    input: ModeSwitchInput,
}

#[derive(Debug, Deserialize)]
struct ModeConfig {
    name: String,
    #[serde(default)]
    remaps: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ModeSwitchInput {
    #[serde(rename = "type")]
    event_type: InputType,
    #[serde(deserialize_with = "deserialize_key_code")]
    code: KeyCode,
    #[serde(default)]
    value: Option<i32>,
}

#[derive(Clone, Debug)]
struct NamedRemapRule {
    name: String,
    rule: RemapRule,
}

pub fn load_config(path: &Path) -> Result<LoadResult, ConfigError> {
    let content = fs::read_to_string(path)?;
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?;
    parse_config_content(path, modified, &content)
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

fn parse_config_content(
    path: &Path,
    modified: SystemTime,
    content: &str,
) -> Result<LoadResult, ConfigError> {
    let parsed: ConfigFile = serde_yaml::from_str(content)?;
    let named_remaps = parse_named_remaps(&parsed.remaps)?;
    validate_config(&parsed, &named_remaps)?;

    let (rules, warnings) = compile_rules(&parsed, &named_remaps)?;

    Ok(LoadResult {
        config: ActiveConfig {
            source_path: path.to_path_buf(),
            source_modified: modified,
            device_selector: into_selector(parsed.device)?,
            reload: parsed.reload,
            rules,
        },
        warnings,
    })
}

fn parse_named_remaps(mapping: &Mapping) -> Result<Vec<NamedRemapRule>, ConfigError> {
    let mut named_remaps = Vec::with_capacity(mapping.len());

    for (index, (key, value)) in mapping.iter().enumerate() {
        let name = key.as_str().ok_or_else(|| {
            ConfigError::Validation(format!("remaps key at index {index} must be a string"))
        })?;
        let rule = serde_yaml::from_value::<RemapRule>(value.clone())?;
        named_remaps.push(NamedRemapRule {
            name: name.to_string(),
            rule,
        });
    }

    Ok(named_remaps)
}

fn validate_config(
    config: &ConfigFile,
    named_remaps: &[NamedRemapRule],
) -> Result<(), ConfigError> {
    validate_device_config(&config.device)?;

    if named_remaps.is_empty() {
        return Err(ConfigError::Validation(
            "remaps must contain at least one rule".to_string(),
        ));
    }

    let mut remap_names = HashSet::new();
    for (index, remap) in named_remaps.iter().enumerate() {
        if !remap_names.insert(remap.name.as_str()) {
            return Err(ConfigError::Validation(format!(
                "remaps.{name} is duplicated",
                name = remap.name
            )));
        }

        validate_remap_rule(&remap.rule, &format!("remaps.{}", remap.name), index)?;
    }

    if let Some(mode_switches) = &config.mode_switches {
        validate_mode_switches(mode_switches, named_remaps)?;
    }

    Ok(())
}

fn validate_remap_rule(
    rule: &RemapRule,
    label: &str,
    fallback_index: usize,
) -> Result<(), ConfigError> {
    if rule.input.event_type != InputType::Key {
        return Err(ConfigError::Validation(format!(
            "{label}.input.type must be \"key\""
        )));
    }

    if !is_supported_button_code(rule.input.code) {
        return Err(ConfigError::Validation(format!(
            "{label}.input.code must be a supported mouse button code, got {:?}",
            rule.input.code
        )));
    }

    if let Some(value) = rule.input.value
        && !matches!(value, 0 | 1)
    {
        return Err(ConfigError::Validation(format!(
            "{label}.input.value must be 0 or 1"
        )));
    }

    if rule.output.is_empty() {
        return Err(ConfigError::Validation(format!(
            "{label}.output must contain at least one key event"
        )));
    }

    for (output_index, output) in rule.output.iter().enumerate() {
        if let Some(value) = output.value
            && !matches!(value, 0 | 1)
        {
            return Err(ConfigError::Validation(format!(
                "{label}.output[{output_index}].value must be 0 or 1"
            )));
        }
    }

    if label.is_empty() {
        return Err(ConfigError::Validation(format!(
            "remaps[{fallback_index}] label must not be empty"
        )));
    }

    Ok(())
}

fn validate_mode_switches(
    mode_switches: &ModeSwitchesConfig,
    named_remaps: &[NamedRemapRule],
) -> Result<(), ConfigError> {
    if mode_switches.modes.is_empty() {
        return Err(ConfigError::Validation(
            "mode_switches.modes must contain at least one mode".to_string(),
        ));
    }

    if mode_switches.input.event_type != InputType::Key {
        return Err(ConfigError::Validation(
            "mode_switches.input.type must be \"key\"".to_string(),
        ));
    }

    if !is_supported_button_code(mode_switches.input.code) {
        return Err(ConfigError::Validation(format!(
            "mode_switches.input.code must be a supported mouse button code, got {:?}",
            mode_switches.input.code
        )));
    }

    if let Some(value) = mode_switches.input.value
        && !matches!(value, 0 | 1)
    {
        return Err(ConfigError::Validation(
            "mode_switches.input.value must be 0 or 1".to_string(),
        ));
    }

    let available_remaps = named_remaps
        .iter()
        .map(|remap| remap.name.as_str())
        .collect::<HashSet<_>>();
    let mut seen_modes = HashSet::new();

    for (index, mode) in mode_switches.modes.iter().enumerate() {
        if mode.name.trim().is_empty() {
            return Err(ConfigError::Validation(format!(
                "mode_switches.modes[{index}].name must not be empty"
            )));
        }

        if !seen_modes.insert(mode.name.as_str()) {
            return Err(ConfigError::Validation(format!(
                "mode_switches.modes[{index}].name duplicates mode \"{}\"",
                mode.name
            )));
        }

        for (remap_index, remap_name) in mode.remaps.iter().enumerate() {
            if !available_remaps.contains(remap_name.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "mode_switches.modes[{index}].remaps[{remap_index}] references unknown remap \"{remap_name}\""
                )));
            }
        }
    }

    Ok(())
}

fn compile_rules(
    config: &ConfigFile,
    named_remaps: &[NamedRemapRule],
) -> Result<(CompiledRules, Vec<ConfigWarning>), ConfigError> {
    let remap_lookup = named_remaps
        .iter()
        .map(|remap| (remap.name.as_str(), remap))
        .collect::<HashMap<_, _>>();

    let mode_configs = if let Some(mode_switches) = &config.mode_switches {
        mode_switches
            .modes
            .iter()
            .map(|mode| (mode.name.clone(), mode.remaps.clone()))
            .collect::<Vec<_>>()
    } else {
        vec![(
            "default".to_string(),
            named_remaps
                .iter()
                .map(|remap| remap.name.clone())
                .collect::<Vec<_>>(),
        )]
    };

    let mut warnings = Vec::new();
    let mut modes = Vec::with_capacity(mode_configs.len());

    for (mode_name, remap_names) in mode_configs {
        let mut remaps = HashMap::new();

        for remap_name in remap_names {
            let named_remap = remap_lookup.get(remap_name.as_str()).ok_or_else(|| {
                ConfigError::Validation(format!(
                    "mode {mode_name} references unknown remap \"{remap_name}\""
                ))
            })?;

            for (trigger, output) in expand_remap_rule(&named_remap.rule) {
                if let Some((shadowed_rule, shadowed_description, _)) = remaps.insert(
                    trigger,
                    (
                        named_remap.name.clone(),
                        named_remap.rule.description.clone(),
                        output,
                    ),
                ) {
                    warnings.push(ConfigWarning::ShadowedRule {
                        mode_name: mode_name.clone(),
                        input: describe_input(trigger),
                        preferred_rule: named_remap.name.clone(),
                        shadowed_rule,
                        preferred_description: named_remap.rule.description.clone(),
                        shadowed_description,
                    });
                }
            }
        }

        modes.push(ModeBindings::new(
            mode_name,
            remaps
                .into_iter()
                .map(|(trigger, (_, _, output))| (trigger, output))
                .collect(),
        ));
    }

    let mode_switch = config.mode_switches.as_ref().map(|mode_switches| {
        CompiledSwitchMode::new(MouseButtonTrigger {
            code: mode_switches.input.code,
            value: mode_switches.input.value.unwrap_or(1),
        })
    });

    Ok((CompiledRules::new(modes, mode_switch), warnings))
}

fn expand_remap_rule(rule: &RemapRule) -> Vec<(MouseButtonTrigger, Vec<KeyStroke>)> {
    let input_values = match rule.input.value {
        Some(value) => vec![value],
        None => vec![0, 1],
    };

    input_values
        .into_iter()
        .map(|input_value| {
            let trigger = MouseButtonTrigger {
                code: rule.input.code,
                value: input_value,
            };
            let output = rule
                .output
                .iter()
                .map(|event| KeyStroke {
                    key: event.key,
                    value: event.value.unwrap_or(input_value),
                    hold: event.hold,
                })
                .collect::<Vec<_>>();
            (trigger, output)
        })
        .collect()
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

fn into_selector(device: RawDeviceConfig) -> Result<DeviceSelector, ConfigError> {
    match (device.path, device.by_id, device.name) {
        (Some(path), None, None) => Ok(DeviceSelector::Path(path)),
        (None, Some(path), None) => Ok(DeviceSelector::ById(path)),
        (None, None, Some(name)) => Ok(DeviceSelector::Name(name)),
        _ => Err(ConfigError::Validation(
            "device must specify exactly one of path, by_id, or name".to_string(),
        )),
    }
}

fn describe_input(input: MouseButtonTrigger) -> String {
    format!("{:?}:{}", input.code, input.value)
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
    let raw = String::deserialize(deserializer)?;
    KeyCode::from_str(&raw).map_err(|_| de::Error::custom(format!("unknown key code: {raw}")))
}

impl<'de> Deserialize<'de> for HoldBehavior {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Option::<u64>::deserialize(deserializer)?;
        Ok(match value {
            Some(milliseconds) => Self::FollowInput(milliseconds),
            None => Self::Tap,
        })
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::KeyCode;
    use std::path::Path;
    use std::time::SystemTime;

    #[test]
    fn omitted_values_expand_press_and_release() {
        let result = parse_config_content(
            Path::new("/tmp/test.yaml"),
            SystemTime::UNIX_EPOCH,
            r#"
device:
  name: "Example Mouse"
remaps:
  enter:
    input:
      type: key
      code: BTN_FORWARD
    output:
      - key: KEY_ENTER
"#,
        )
        .expect("config should parse");

        assert_eq!(result.config.rules.mode_count(), 1);
        assert_eq!(result.config.rules.current_mode_name(0), Some("default"));

        let press = result
            .config
            .rules
            .remap_for(
                0,
                MouseButtonTrigger {
                    code: KeyCode::BTN_FORWARD,
                    value: 1,
                },
            )
            .expect("press mapping");
        let release = result
            .config
            .rules
            .remap_for(
                0,
                MouseButtonTrigger {
                    code: KeyCode::BTN_FORWARD,
                    value: 0,
                },
            )
            .expect("release mapping");

        assert_eq!(press[0].value, 1);
        assert_eq!(release[0].value, 0);
        assert_eq!(press[0].hold, HoldBehavior::FollowInput(0));
        assert_eq!(release[0].hold, HoldBehavior::FollowInput(0));
    }

    #[test]
    fn mode_switches_compile_named_modes() {
        let result = parse_config_content(
            Path::new("/tmp/test.yaml"),
            SystemTime::UNIX_EPOCH,
            r#"
device:
  name: "Example Mouse"
remaps:
  enter:
    input:
      type: key
      code: BTN_FORWARD
    output:
      - key: KEY_ENTER
  meta:
    input:
      type: key
      code: BTN_BACK
      value: 1
    output:
      - key: KEY_LEFTMETA
        value: 1
mode_switches:
  modes:
    - name: default
      remaps: [enter]
    - name: sub
      remaps: [meta]
  input:
    type: key
    code: BTN_SIDE
"#,
        )
        .expect("config should parse");

        assert_eq!(result.config.rules.mode_count(), 2);
        assert_eq!(result.config.rules.current_mode_name(0), Some("default"));
        assert_eq!(
            result.config.rules.mode_switch_trigger(),
            Some(MouseButtonTrigger {
                code: KeyCode::BTN_SIDE,
                value: 1,
            })
        );
        assert!(
            result
                .config
                .rules
                .remap_for(
                    1,
                    MouseButtonTrigger {
                        code: KeyCode::BTN_BACK,
                        value: 1,
                    },
                )
                .is_some()
        );
    }

    #[test]
    fn unknown_mode_remap_reference_fails_validation() {
        let err = parse_config_content(
            Path::new("/tmp/test.yaml"),
            SystemTime::UNIX_EPOCH,
            r#"
device:
  name: "Example Mouse"
remaps:
  enter:
    input:
      type: key
      code: BTN_FORWARD
    output:
      - key: KEY_ENTER
mode_switches:
  modes:
    - name: default
      remaps: [missing]
  input:
    type: key
    code: BTN_SIDE
"#,
        )
        .expect_err("config should be rejected");

        assert!(
            err.to_string()
                .contains("references unknown remap \"missing\"")
        );
    }

    #[test]
    fn hold_null_compiles_to_tap_behavior() {
        let result = parse_config_content(
            Path::new("/tmp/test.yaml"),
            SystemTime::UNIX_EPOCH,
            r#"
device:
  name: "Example Mouse"
remaps:
  tap-enter:
    input:
      type: key
      code: BTN_FORWARD
    output:
      - key: KEY_ENTER
        hold: null
"#,
        )
        .expect("config should parse");

        let press = result
            .config
            .rules
            .remap_for(
                0,
                MouseButtonTrigger {
                    code: KeyCode::BTN_FORWARD,
                    value: 1,
                },
            )
            .expect("press mapping");

        assert_eq!(press[0].hold, HoldBehavior::Tap);
    }

    #[test]
    fn hold_milliseconds_are_preserved() {
        let result = parse_config_content(
            Path::new("/tmp/test.yaml"),
            SystemTime::UNIX_EPOCH,
            r#"
device:
  name: "Example Mouse"
remaps:
  delayed-enter:
    input:
      type: key
      code: BTN_FORWARD
    output:
      - key: KEY_ENTER
        hold: 120
"#,
        )
        .expect("config should parse");

        let release = result
            .config
            .rules
            .remap_for(
                0,
                MouseButtonTrigger {
                    code: KeyCode::BTN_FORWARD,
                    value: 0,
                },
            )
            .expect("release mapping");

        assert_eq!(release[0].hold, HoldBehavior::FollowInput(120));
    }
}
