use crate::device::NormalizedMouseEvent;
use evdev::KeyCode;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MouseButtonTrigger {
    pub code: KeyCode,
    pub value: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: KeyCode,
    pub value: i32,
    pub hold: HoldBehavior,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldBehavior {
    FollowInput(u64),
    Tap,
}

impl Default for HoldBehavior {
    fn default() -> Self {
        Self::FollowInput(0)
    }
}

impl KeyStroke {
    /// Builds one runtime key event that follows the input state.
    pub fn press(key: KeyCode) -> Self {
        Self {
            key,
            value: 1,
            hold: HoldBehavior::FollowInput(0),
        }
    }

    /// Builds one runtime key release event.
    pub fn release(key: KeyCode) -> Self {
        Self {
            key,
            value: 0,
            hold: HoldBehavior::FollowInput(0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ModeBindings {
    name: String,
    remaps: HashMap<MouseButtonTrigger, Vec<KeyStroke>>,
}

impl ModeBindings {
    /// Creates one named mode with precompiled remap bindings.
    pub fn new(name: String, remaps: HashMap<MouseButtonTrigger, Vec<KeyStroke>>) -> Self {
        Self { name, remaps }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompiledSwitchMode {
    trigger: MouseButtonTrigger,
}

impl CompiledSwitchMode {
    /// Creates a compiled mode-switch trigger.
    pub fn new(trigger: MouseButtonTrigger) -> Self {
        Self { trigger }
    }
}

#[derive(Clone, Debug, Default)]
pub struct CompiledRules {
    modes: Vec<ModeBindings>,
    mode_switch: Option<CompiledSwitchMode>,
    registered_keys: Vec<KeyCode>,
}

impl CompiledRules {
    /// Builds lookup tables for runtime routing.
    pub fn new(modes: Vec<ModeBindings>, mode_switch: Option<CompiledSwitchMode>) -> Self {
        let mut registered_keys = modes
            .iter()
            .flat_map(|mode| {
                mode.remaps
                    .values()
                    .flat_map(|sequence| sequence.iter().map(|stroke| stroke.key))
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        registered_keys.sort_unstable_by_key(|key| key.code());

        Self {
            modes,
            mode_switch,
            registered_keys,
        }
    }

    /// Returns key capabilities required by the virtual keyboard.
    pub fn registered_keys(&self) -> &[KeyCode] {
        &self.registered_keys
    }

    /// Returns the number of configured modes.
    pub fn mode_count(&self) -> usize {
        self.modes.len()
    }

    /// Returns the mode name for the given index.
    pub fn current_mode_name(&self, mode_index: usize) -> Option<&str> {
        self.modes.get(mode_index).map(|mode| mode.name.as_str())
    }

    /// Finds a mode index by name.
    pub fn find_mode_index(&self, mode_name: &str) -> Option<usize> {
        self.modes.iter().position(|mode| mode.name == mode_name)
    }

    /// Returns the next mode index in cyclic order.
    pub fn next_mode_index(&self, current_mode_index: usize) -> usize {
        if self.modes.is_empty() {
            0
        } else {
            (current_mode_index + 1) % self.modes.len()
        }
    }

    /// Returns the compiled mode-switch trigger, if configured.
    pub fn mode_switch_trigger(&self) -> Option<MouseButtonTrigger> {
        self.mode_switch.map(|mode_switch| mode_switch.trigger)
    }

    /// Returns the remap sequence for one trigger within one mode.
    pub fn remap_for(
        &self,
        mode_index: usize,
        trigger: MouseButtonTrigger,
    ) -> Option<&[KeyStroke]> {
        self.modes
            .get(mode_index)
            .and_then(|mode| mode.remaps.get(&trigger))
            .map(Vec::as_slice)
    }
}

pub enum RoutedAction<'a> {
    PassThrough,
    Remap(&'a [KeyStroke]),
    SwitchMode,
    Flush,
    Ignore,
}

/// Resolves one normalized mouse event into passthrough, remap, or mode-switch output.
pub fn route<'a>(
    event: &NormalizedMouseEvent,
    rules: &'a CompiledRules,
    active_mode_index: usize,
) -> RoutedAction<'a> {
    match event {
        NormalizedMouseEvent::Button { code, value } => {
            let trigger = MouseButtonTrigger {
                code: *code,
                value: *value,
            };

            if rules.mode_switch_trigger() == Some(trigger) {
                return RoutedAction::SwitchMode;
            }

            rules
                .remap_for(active_mode_index, trigger)
                .map_or(RoutedAction::PassThrough, RoutedAction::Remap)
        }
        NormalizedMouseEvent::Relative { .. } => RoutedAction::PassThrough,
        NormalizedMouseEvent::SyncReport => RoutedAction::Flush,
        NormalizedMouseEvent::OtherIgnored => RoutedAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::NormalizedMouseEvent;

    fn sample_rules() -> CompiledRules {
        CompiledRules::new(
            vec![
                ModeBindings::new(
                    "default".to_string(),
                    HashMap::from([(
                        MouseButtonTrigger {
                            code: KeyCode::BTN_RIGHT,
                            value: 1,
                        },
                        vec![KeyStroke {
                            key: KeyCode::KEY_LEFTMETA,
                            value: 1,
                            hold: HoldBehavior::FollowInput(0),
                        }],
                    )]),
                ),
                ModeBindings::new(
                    "sub".to_string(),
                    HashMap::from([(
                        MouseButtonTrigger {
                            code: KeyCode::BTN_RIGHT,
                            value: 1,
                        },
                        vec![KeyStroke {
                            key: KeyCode::KEY_TAB,
                            value: 1,
                            hold: HoldBehavior::FollowInput(0),
                        }],
                    )]),
                ),
            ],
            Some(CompiledSwitchMode::new(MouseButtonTrigger {
                code: KeyCode::BTN_SIDE,
                value: 1,
            })),
        )
    }

    #[test]
    fn button_match_remaps_to_keyboard() {
        let rules = sample_rules();

        let action = route(
            &NormalizedMouseEvent::Button {
                code: KeyCode::BTN_RIGHT,
                value: 1,
            },
            &rules,
            0,
        );

        match action {
            RoutedAction::Remap(sequence) => {
                assert_eq!(sequence.len(), 1);
                assert_eq!(sequence[0].key, KeyCode::KEY_LEFTMETA);
            }
            _ => panic!("expected remap"),
        }
    }

    #[test]
    fn active_mode_changes_remap_target() {
        let rules = sample_rules();

        let action = route(
            &NormalizedMouseEvent::Button {
                code: KeyCode::BTN_RIGHT,
                value: 1,
            },
            &rules,
            1,
        );

        match action {
            RoutedAction::Remap(sequence) => {
                assert_eq!(sequence.len(), 1);
                assert_eq!(sequence[0].key, KeyCode::KEY_TAB);
            }
            _ => panic!("expected remap"),
        }
    }

    #[test]
    fn mode_switch_takes_precedence_over_remap() {
        let rules = CompiledRules::new(
            vec![ModeBindings::new(
                "default".to_string(),
                HashMap::from([(
                    MouseButtonTrigger {
                        code: KeyCode::BTN_SIDE,
                        value: 1,
                    },
                    vec![KeyStroke {
                        key: KeyCode::KEY_ENTER,
                        value: 1,
                        hold: HoldBehavior::FollowInput(0),
                    }],
                )]),
            )],
            Some(CompiledSwitchMode::new(MouseButtonTrigger {
                code: KeyCode::BTN_SIDE,
                value: 1,
            })),
        );

        let action = route(
            &NormalizedMouseEvent::Button {
                code: KeyCode::BTN_SIDE,
                value: 1,
            },
            &rules,
            0,
        );

        assert!(matches!(action, RoutedAction::SwitchMode));
    }

    #[test]
    fn unmatched_button_passes_through() {
        let rules = CompiledRules::default();
        let action = route(
            &NormalizedMouseEvent::Button {
                code: KeyCode::BTN_LEFT,
                value: 1,
            },
            &rules,
            0,
        );
        assert!(matches!(action, RoutedAction::PassThrough));
    }

    #[test]
    fn relative_events_pass_through() {
        let rules = CompiledRules::default();
        let action = route(
            &NormalizedMouseEvent::Relative {
                code: evdev::RelativeAxisCode::REL_X,
                value: 10,
            },
            &rules,
            0,
        );
        assert!(matches!(action, RoutedAction::PassThrough));
    }
}
