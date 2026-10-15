//! Linux key codes, modifier tracking and the `Ctrl+Alt+<key>` shortcuts.
//!
//! Codes are the kernel's `KEY_*` values: physical key positions, not characters, so a
//! shortcut letter means the same key on QWERTY and AZERTY layouts alike.

use std::collections::{BTreeMap, BTreeSet};

use aujar_config::{MouseBordersHotkeys, SwitchKeys};

use crate::error::{Error, Result};

pub const KEY_ESC: u16 = 1;
pub const KEY_LEFTCTRL: u16 = 29;
pub const KEY_LEFTSHIFT: u16 = 42;
pub const KEY_RIGHTSHIFT: u16 = 54;
pub const KEY_LEFTALT: u16 = 56;
pub const KEY_RIGHTCTRL: u16 = 97;
pub const KEY_RIGHTALT: u16 = 100;
pub const KEY_LEFTMETA: u16 = 125;
pub const KEY_RIGHTMETA: u16 = 126;
/// Harmless key used to reset idle timers (see [`crate::backend::InputSink::nudge`]).
pub const KEY_F15: u16 = 186;

/// Row-major QWERTY positions for `a`..`z`, indexed by letter.
const LETTER_CODES: [u16; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45,
    21, 44,
];

/// `KEY_1`..`KEY_9`, `KEY_0`.
const DIGIT_CODES: [u16; 10] = [11, 2, 3, 4, 5, 6, 7, 8, 9, 10];

const FUNCTION_CODES: [u16; 12] = [59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 87, 88];

/// Key code for an ASCII letter or digit (case-insensitive).
pub fn char_code(c: char) -> Option<u16> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(LETTER_CODES[usize::from(c as u8 - b'a')]),
        c @ '0'..='9' => Some(DIGIT_CODES[usize::from(c as u8 - b'0')]),
        _ => None,
    }
}

/// Key code of `F<n>`, `n` in `1..=12`.
pub fn function_code(n: u8) -> Option<u16> {
    usize::from(n)
        .checked_sub(1)
        .and_then(|i| FUNCTION_CODES.get(i))
        .copied()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

/// Which keys are physically down. Needed to release them when input is grabbed (the
/// desktop would otherwise see a key go down and never up).
#[derive(Debug, Default, Clone)]
pub struct KeyboardTracker {
    held: BTreeSet<u16>,
}

impl KeyboardTracker {
    pub fn update(&mut self, code: u16, pressed: bool) {
        if pressed {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
    }

    pub fn modifiers(&self) -> Modifiers {
        let down = |codes: &[u16]| codes.iter().any(|code| self.held.contains(code));

        Modifiers {
            shift: down(&[KEY_LEFTSHIFT, KEY_RIGHTSHIFT]),
            ctrl: down(&[KEY_LEFTCTRL, KEY_RIGHTCTRL]),
            alt: down(&[KEY_LEFTALT, KEY_RIGHTALT]),
            meta: down(&[KEY_LEFTMETA, KEY_RIGHTMETA]),
        }
    }

    pub fn held(&self) -> Vec<u16> {
        self.held.iter().copied().collect()
    }

    pub fn clear(&mut self) {
        self.held.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    ToggleEasyMouse,
    LockAll,
    Reconnect,
    ToggleMultiMachineInput,
    /// Jump to the machine in this slot.
    Switch(u8),
}

/// The configured `Ctrl+Alt+<key>` shortcuts.
#[derive(Debug, Clone)]
pub struct HotkeyBindings {
    bindings: BTreeMap<u16, HotkeyAction>,
}

impl HotkeyBindings {
    pub fn from_settings(settings: &MouseBordersHotkeys) -> Result<Self> {
        let mut bindings = BTreeMap::new();

        let mut bind =
            |code: u16, action: HotkeyAction, label: &str| match bindings.insert(code, action) {
                Some(previous) => Err(Error::Config(format!(
                    "hotkey for {label} collides with {previous:?}"
                ))),
                None => Ok(()),
            };

        for slot in 0..4u8 {
            let code = match settings.switch_keys {
                SwitchKeys::Digits => char_code(char::from(b'1' + slot)),
                SwitchKeys::FunctionKeys => function_code(slot + 1),
            }
            .ok_or_else(|| Error::Config("switch keys unavailable".into()))?;

            bind(code, HotkeyAction::Switch(slot), "switching machines")?;
        }

        for (value, action, label) in [
            (
                &settings.toggle_easy_mouse,
                HotkeyAction::ToggleEasyMouse,
                "toggle_easy_mouse",
            ),
            (
                &settings.lock_machines,
                HotkeyAction::LockAll,
                "lock_machines",
            ),
            (&settings.reconnect, HotkeyAction::Reconnect, "reconnect"),
            (
                &settings.multi_machine_input,
                HotkeyAction::ToggleMultiMachineInput,
                "multi_machine_input",
            ),
        ] {
            let mut chars = value.chars();
            let code = match (chars.next(), chars.next()) {
                (Some(c), None) => char_code(c),
                _ => None,
            }
            .ok_or_else(|| {
                Error::Config(format!(
                    "hotkey `{label}` must be a single letter or digit, not `{value}`"
                ))
            })?;

            bind(code, action, label)?;
        }

        Ok(Self { bindings })
    }

    /// The action for pressing `code` with `modifiers` held, if any.
    pub fn lookup(&self, code: u16, modifiers: Modifiers) -> Option<HotkeyAction> {
        if modifiers.ctrl && modifiers.alt {
            self.bindings.get(&code).copied()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> HotkeyBindings {
        HotkeyBindings::from_settings(&MouseBordersHotkeys::default()).unwrap()
    }

    fn ctrl_alt() -> Modifiers {
        Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::default()
        }
    }

    #[test]
    fn key_code_tables_match_the_kernel() {
        // KEY_A=30, KEY_Z=44, KEY_Q=16, KEY_M=50, KEY_1=2, KEY_0=11, KEY_F1=59, KEY_F11=87.
        assert_eq!(char_code('a'), Some(30));
        assert_eq!(char_code('Z'), Some(44));
        assert_eq!(char_code('q'), Some(16));
        assert_eq!(char_code('m'), Some(50));
        assert_eq!(char_code('1'), Some(2));
        assert_eq!(char_code('0'), Some(11));
        assert_eq!(function_code(1), Some(59));
        assert_eq!(function_code(11), Some(87));
        assert_eq!(function_code(0), None);
        assert_eq!(char_code('-'), None);
    }

    #[test]
    fn digits_switch_machines_by_default() {
        let keys = defaults();

        assert_eq!(
            keys.lookup(char_code('1').unwrap(), ctrl_alt()),
            Some(HotkeyAction::Switch(0))
        );
        assert_eq!(
            keys.lookup(char_code('4').unwrap(), ctrl_alt()),
            Some(HotkeyAction::Switch(3))
        );
        assert_eq!(
            keys.lookup(char_code('e').unwrap(), ctrl_alt()),
            Some(HotkeyAction::ToggleEasyMouse)
        );
    }

    #[test]
    fn function_keys_are_selectable() {
        let settings = MouseBordersHotkeys {
            switch_keys: SwitchKeys::FunctionKeys,
            ..MouseBordersHotkeys::default()
        };
        let keys = HotkeyBindings::from_settings(&settings).unwrap();

        assert_eq!(
            keys.lookup(function_code(2).unwrap(), ctrl_alt()),
            Some(HotkeyAction::Switch(1))
        );
        assert_eq!(keys.lookup(char_code('1').unwrap(), ctrl_alt()), None);
    }

    #[test]
    fn needs_both_ctrl_and_alt() {
        let keys = defaults();
        let code = char_code('l').unwrap();

        assert_eq!(keys.lookup(code, Modifiers::default()), None);
        assert_eq!(
            keys.lookup(
                code,
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                }
            ),
            None
        );
        assert_eq!(keys.lookup(code, ctrl_alt()), Some(HotkeyAction::LockAll));
    }

    #[test]
    fn collisions_and_bad_values_are_rejected() {
        let clash = MouseBordersHotkeys {
            lock_machines: "1".into(),
            ..MouseBordersHotkeys::default()
        };
        let long = MouseBordersHotkeys {
            reconnect: "rr".into(),
            ..MouseBordersHotkeys::default()
        };
        let empty = MouseBordersHotkeys {
            reconnect: String::new(),
            ..MouseBordersHotkeys::default()
        };
        let same = MouseBordersHotkeys {
            reconnect: "e".into(),
            ..MouseBordersHotkeys::default()
        };

        assert!(HotkeyBindings::from_settings(&clash).is_err());
        assert!(HotkeyBindings::from_settings(&long).is_err());
        assert!(HotkeyBindings::from_settings(&empty).is_err());
        assert!(HotkeyBindings::from_settings(&same).is_err());
    }

    #[test]
    fn tracker_reports_modifiers_and_held_keys() {
        let mut tracker = KeyboardTracker::default();

        tracker.update(KEY_RIGHTCTRL, true);
        tracker.update(KEY_LEFTALT, true);
        tracker.update(30, true);

        assert!(tracker.modifiers().ctrl && tracker.modifiers().alt);
        assert!(!tracker.modifiers().shift);
        assert_eq!(tracker.held(), [30, 56, 97]);

        tracker.update(KEY_RIGHTCTRL, false);
        assert!(!tracker.modifiers().ctrl);

        tracker.clear();
        assert!(tracker.held().is_empty());
    }
}
