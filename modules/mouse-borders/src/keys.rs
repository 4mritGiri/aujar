//! Modifier tracking and the `Ctrl+Alt+<key>` shortcuts.
//!
//! Keys are USB HID usages (see [`crate::keymap`]): physical key positions, not characters,
//! so a shortcut letter means the same key on QWERTY and AZERTY layouts alike, and on every
//! operating system. "Alt" is Option on a Mac and "Meta" is the Windows key or Command.

use std::collections::{BTreeMap, BTreeSet};

use aujar_config::{MouseBordersHotkeys, SwitchKeys};

use crate::error::{Error, Result};

pub const KEY_ESC: u16 = 0x29;
pub const KEY_LEFTCTRL: u16 = 0xE0;
pub const KEY_LEFTSHIFT: u16 = 0xE1;
pub const KEY_LEFTALT: u16 = 0xE2;
pub const KEY_LEFTMETA: u16 = 0xE3;
pub const KEY_RIGHTCTRL: u16 = 0xE4;
pub const KEY_RIGHTSHIFT: u16 = 0xE5;
pub const KEY_RIGHTALT: u16 = 0xE6;
pub const KEY_RIGHTMETA: u16 = 0xE7;
/// Harmless key used to reset idle timers (see [`crate::backend::InputSink::nudge`]).
pub const KEY_F15: u16 = 0x6A;

/// HID usage for an ASCII letter or digit (case-insensitive).
pub fn char_code(c: char) -> Option<u16> {
    match c.to_ascii_lowercase() {
        c @ 'a'..='z' => Some(0x04 + u16::from(c as u8 - b'a')),
        c @ '1'..='9' => Some(0x1E + u16::from(c as u8 - b'1')),
        '0' => Some(0x27),
        _ => None,
    }
}

/// HID usage of `F<n>`, `n` in `1..=12`.
pub fn function_code(n: u8) -> Option<u16> {
    (1..=12).contains(&n).then(|| 0x3A + u16::from(n) - 1)
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
        // USB HID usages: A=0x04, Z=0x1D, Q=0x14, M=0x10, 1=0x1E, 0=0x27, F1=0x3A, F11=0x44.
        assert_eq!(char_code('a'), Some(0x04));
        assert_eq!(char_code('Z'), Some(0x1D));
        assert_eq!(char_code('q'), Some(0x14));
        assert_eq!(char_code('m'), Some(0x10));
        assert_eq!(char_code('1'), Some(0x1E));
        assert_eq!(char_code('0'), Some(0x27));
        assert_eq!(function_code(1), Some(0x3A));
        assert_eq!(function_code(11), Some(0x44));
        assert_eq!(function_code(13), None);
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
        tracker.update(0x04, true);

        assert!(tracker.modifiers().ctrl && tracker.modifiers().alt);
        assert!(!tracker.modifiers().shift);
        assert_eq!(tracker.held(), [0x04, KEY_LEFTALT, KEY_RIGHTCTRL]);

        tracker.update(KEY_RIGHTCTRL, false);
        assert!(!tracker.modifiers().ctrl);

        tracker.clear();
        assert!(tracker.held().is_empty());
    }
}
