//! One table of physical keys, with the code each operating system uses for them.
//!
//! On the wire, keys travel as **USB HID usages** (keyboard page `0x07`), which mean the same
//! thing on every OS. Each platform backend converts between its own codes and HID with the
//! functions here, so a key pressed on a Windows keyboard arrives on a Mac as the same
//! *physical* key (the receiving machine's layout then decides what it types).
//!
//! | Column      | Meaning                                                              |
//! |-------------|----------------------------------------------------------------------|
//! | `hid`       | USB HID usage id                                                     |
//! | `evdev`     | Linux `KEY_*` code                                                   |
//! | `scancode`  | Windows scan code (set 1); `0xE0xx` for "extended" keys              |
//! | `mac`       | macOS virtual key code (`kVK_*`), ANSI layout positions              |
//!
//! A key a platform does not have is `None` there (macOS has no Print Screen, for example).

/// One physical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalKey {
    /// Linux name without the `KEY_` prefix, used in logs and to check the table against the
    /// kernel's own names.
    pub name: &'static str,
    pub hid: u16,
    pub evdev: Option<u16>,
    pub scancode: Option<u16>,
    pub mac: Option<u16>,
}

const NONE: u16 = u16::MAX;

const fn opt(value: u16) -> Option<u16> {
    if value == NONE { None } else { Some(value) }
}

const fn key(name: &'static str, hid: u16, evdev: u16, scancode: u16, mac: u16) -> PhysicalKey {
    PhysicalKey {
        name,
        hid,
        evdev: opt(evdev),
        scancode: opt(scancode),
        mac: opt(mac),
    }
}

/// Every key Mouse Borders can forward.
pub const KEYS: &[PhysicalKey] = &[
    // Letters
    key("A", 0x04, 30, 0x1E, 0x00),
    key("B", 0x05, 48, 0x30, 0x0B),
    key("C", 0x06, 46, 0x2E, 0x08),
    key("D", 0x07, 32, 0x20, 0x02),
    key("E", 0x08, 18, 0x12, 0x0E),
    key("F", 0x09, 33, 0x21, 0x03),
    key("G", 0x0A, 34, 0x22, 0x05),
    key("H", 0x0B, 35, 0x23, 0x04),
    key("I", 0x0C, 23, 0x17, 0x22),
    key("J", 0x0D, 36, 0x24, 0x26),
    key("K", 0x0E, 37, 0x25, 0x28),
    key("L", 0x0F, 38, 0x26, 0x25),
    key("M", 0x10, 50, 0x32, 0x2E),
    key("N", 0x11, 49, 0x31, 0x2D),
    key("O", 0x12, 24, 0x18, 0x1F),
    key("P", 0x13, 25, 0x19, 0x23),
    key("Q", 0x14, 16, 0x10, 0x0C),
    key("R", 0x15, 19, 0x13, 0x0F),
    key("S", 0x16, 31, 0x1F, 0x01),
    key("T", 0x17, 20, 0x14, 0x11),
    key("U", 0x18, 22, 0x16, 0x20),
    key("V", 0x19, 47, 0x2F, 0x09),
    key("W", 0x1A, 17, 0x11, 0x0D),
    key("X", 0x1B, 45, 0x2D, 0x07),
    key("Y", 0x1C, 21, 0x15, 0x10),
    key("Z", 0x1D, 44, 0x2C, 0x06),
    // Digits (top row)
    key("1", 0x1E, 2, 0x02, 0x12),
    key("2", 0x1F, 3, 0x03, 0x13),
    key("3", 0x20, 4, 0x04, 0x14),
    key("4", 0x21, 5, 0x05, 0x15),
    key("5", 0x22, 6, 0x06, 0x17),
    key("6", 0x23, 7, 0x07, 0x16),
    key("7", 0x24, 8, 0x08, 0x1A),
    key("8", 0x25, 9, 0x09, 0x1C),
    key("9", 0x26, 10, 0x0A, 0x19),
    key("0", 0x27, 11, 0x0B, 0x1D),
    // Editing and punctuation
    key("ENTER", 0x28, 28, 0x1C, 0x24),
    key("ESC", 0x29, 1, 0x01, 0x35),
    key("BACKSPACE", 0x2A, 14, 0x0E, 0x33),
    key("TAB", 0x2B, 15, 0x0F, 0x30),
    key("SPACE", 0x2C, 57, 0x39, 0x31),
    key("MINUS", 0x2D, 12, 0x0C, 0x1B),
    key("EQUAL", 0x2E, 13, 0x0D, 0x18),
    key("LEFTBRACE", 0x2F, 26, 0x1A, 0x21),
    key("RIGHTBRACE", 0x30, 27, 0x1B, 0x1E),
    key("BACKSLASH", 0x31, 43, 0x2B, 0x2A),
    key("SEMICOLON", 0x33, 39, 0x27, 0x29),
    key("APOSTROPHE", 0x34, 40, 0x28, 0x27),
    key("GRAVE", 0x35, 41, 0x29, 0x32),
    key("COMMA", 0x36, 51, 0x33, 0x2B),
    key("DOT", 0x37, 52, 0x34, 0x2F),
    key("SLASH", 0x38, 53, 0x35, 0x2C),
    key("CAPSLOCK", 0x39, 58, 0x3A, 0x39),
    // Function keys
    key("F1", 0x3A, 59, 0x3B, 0x7A),
    key("F2", 0x3B, 60, 0x3C, 0x78),
    key("F3", 0x3C, 61, 0x3D, 0x63),
    key("F4", 0x3D, 62, 0x3E, 0x76),
    key("F5", 0x3E, 63, 0x3F, 0x60),
    key("F6", 0x3F, 64, 0x40, 0x61),
    key("F7", 0x40, 65, 0x41, 0x62),
    key("F8", 0x41, 66, 0x42, 0x64),
    key("F9", 0x42, 67, 0x43, 0x65),
    key("F10", 0x43, 68, 0x44, 0x6D),
    key("F11", 0x44, 87, 0x57, 0x67),
    key("F12", 0x45, 88, 0x58, 0x6F),
    // System and navigation
    key("SYSRQ", 0x46, 99, 0xE037, NONE), // Print Screen
    key("SCROLLLOCK", 0x47, 70, 0x46, NONE),
    key("PAUSE", 0x48, 119, NONE, NONE),
    key("INSERT", 0x49, 110, 0xE052, 0x72),
    key("HOME", 0x4A, 102, 0xE047, 0x73),
    key("PAGEUP", 0x4B, 104, 0xE049, 0x74),
    key("DELETE", 0x4C, 111, 0xE053, 0x75),
    key("END", 0x4D, 107, 0xE04F, 0x77),
    key("PAGEDOWN", 0x4E, 109, 0xE051, 0x79),
    key("RIGHT", 0x4F, 106, 0xE04D, 0x7C),
    key("LEFT", 0x50, 105, 0xE04B, 0x7B),
    key("DOWN", 0x51, 108, 0xE050, 0x7D),
    key("UP", 0x52, 103, 0xE048, 0x7E),
    // Keypad
    key("NUMLOCK", 0x53, 69, 0x45, 0x47),
    key("KPSLASH", 0x54, 98, 0xE035, 0x4B),
    key("KPASTERISK", 0x55, 55, 0x37, 0x43),
    key("KPMINUS", 0x56, 74, 0x4A, 0x4E),
    key("KPPLUS", 0x57, 78, 0x4E, 0x45),
    key("KPENTER", 0x58, 96, 0xE01C, 0x4C),
    key("KP1", 0x59, 79, 0x4F, 0x53),
    key("KP2", 0x5A, 80, 0x50, 0x54),
    key("KP3", 0x5B, 81, 0x51, 0x55),
    key("KP4", 0x5C, 75, 0x4B, 0x56),
    key("KP5", 0x5D, 76, 0x4C, 0x57),
    key("KP6", 0x5E, 77, 0x4D, 0x58),
    key("KP7", 0x5F, 71, 0x47, 0x59),
    key("KP8", 0x60, 72, 0x48, 0x5B),
    key("KP9", 0x61, 73, 0x49, 0x5C),
    key("KP0", 0x62, 82, 0x52, 0x52),
    key("KPDOT", 0x63, 83, 0x53, 0x41),
    key("102ND", 0x64, 86, 0x56, 0x0A), // the extra key on ISO keyboards
    key("COMPOSE", 0x65, 127, 0xE05D, 0x6E), // menu / application key
    key("KPEQUAL", 0x67, 117, NONE, 0x51),
    // Extra function keys
    key("F13", 0x68, 183, 0x64, 0x69),
    key("F14", 0x69, 184, 0x65, 0x6B),
    key("F15", 0x6A, 185, 0x66, 0x71),
    key("F16", 0x6B, 186, 0x67, 0x6A),
    key("F17", 0x6C, 187, 0x68, 0x40),
    key("F18", 0x6D, 188, 0x69, 0x4F),
    key("F19", 0x6E, 189, 0x6A, 0x50),
    key("F20", 0x6F, 190, 0x6B, 0x5A),
    key("F21", 0x70, 191, 0x6C, NONE),
    key("F22", 0x71, 192, 0x6D, NONE),
    key("F23", 0x72, 193, 0x6E, NONE),
    key("F24", 0x73, 194, 0x76, NONE),
    // Modifiers
    key("LEFTCTRL", 0xE0, 29, 0x1D, 0x3B),
    key("LEFTSHIFT", 0xE1, 42, 0x2A, 0x38),
    key("LEFTALT", 0xE2, 56, 0x38, 0x3A),
    key("LEFTMETA", 0xE3, 125, 0xE05B, 0x37),
    key("RIGHTCTRL", 0xE4, 97, 0xE01D, 0x3E),
    key("RIGHTSHIFT", 0xE5, 54, 0x36, 0x3C),
    key("RIGHTALT", 0xE6, 100, 0xE038, 0x3D),
    key("RIGHTMETA", 0xE7, 126, 0xE05C, 0x36),
];

fn find(predicate: impl Fn(&PhysicalKey) -> bool) -> Option<&'static PhysicalKey> {
    KEYS.iter().find(|key| predicate(key))
}

/// HID usage for a Linux `KEY_*` code.
pub fn from_evdev(code: u16) -> Option<u16> {
    find(|key| key.evdev == Some(code)).map(|key| key.hid)
}

pub fn to_evdev(hid: u16) -> Option<u16> {
    find(|key| key.hid == hid).and_then(|key| key.evdev)
}

/// HID usage for a Windows scan code. Extended keys carry `0xE000` in the high byte.
pub fn from_scancode(scancode: u16) -> Option<u16> {
    find(|key| key.scancode == Some(scancode)).map(|key| key.hid)
}

pub fn to_scancode(hid: u16) -> Option<u16> {
    find(|key| key.hid == hid).and_then(|key| key.scancode)
}

/// HID usage for a macOS virtual key code.
pub fn from_mac(code: u16) -> Option<u16> {
    find(|key| key.mac == Some(code)).map(|key| key.hid)
}

pub fn to_mac(hid: u16) -> Option<u16> {
    find(|key| key.hid == hid).and_then(|key| key.mac)
}

/// A readable name for logs (`"A"`, `"LEFTCTRL"`), or the number if unknown.
pub fn name(hid: u16) -> String {
    find(|key| key.hid == hid).map_or_else(|| format!("0x{hid:02X}"), |key| key.name.to_owned())
}

/// Whether a Windows scan code is an extended (`0xE0`-prefixed) one.
pub const fn is_extended(scancode: u16) -> bool {
    scancode & 0xFF00 == 0xE000
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn all_unique(values: impl Iterator<Item = u16>) -> bool {
        let mut seen = HashSet::new();

        values.into_iter().all(|value| seen.insert(value))
    }

    #[test]
    fn no_code_is_used_twice_in_any_column() {
        assert!(all_unique(KEYS.iter().map(|key| key.hid)), "hid");
        assert!(all_unique(KEYS.iter().filter_map(|key| key.evdev)), "evdev");
        assert!(
            all_unique(KEYS.iter().filter_map(|key| key.scancode)),
            "scancode"
        );
        assert!(all_unique(KEYS.iter().filter_map(|key| key.mac)), "mac");

        let mut names = HashSet::new();

        assert!(KEYS.iter().all(|key| names.insert(key.name)), "names");
    }

    #[test]
    fn every_key_converts_there_and_back_on_every_platform() {
        for key in KEYS {
            if let Some(code) = key.evdev {
                assert_eq!(from_evdev(code), Some(key.hid), "{}", key.name);
                assert_eq!(to_evdev(key.hid), Some(code));
            }
            if let Some(code) = key.scancode {
                assert_eq!(from_scancode(code), Some(key.hid), "{}", key.name);
                assert_eq!(to_scancode(key.hid), Some(code));
            }
            if let Some(code) = key.mac {
                assert_eq!(from_mac(code), Some(key.hid), "{}", key.name);
                assert_eq!(to_mac(key.hid), Some(code));
            }
        }
    }

    #[test]
    fn the_hid_column_is_laid_out_as_the_usb_table_says() {
        let hid = |name: &str| KEYS.iter().find(|key| key.name == name).unwrap().hid;

        // Letters, digits and function keys are contiguous runs.
        for (index, letter) in ('A'..='Z').enumerate() {
            assert_eq!(hid(&letter.to_string()), 0x04 + index as u16);
        }
        for (index, digit) in ('1'..='9').enumerate() {
            assert_eq!(hid(&digit.to_string()), 0x1E + index as u16);
        }
        assert_eq!(hid("0"), 0x27);
        for number in 1..=12u16 {
            assert_eq!(hid(&format!("F{number}")), 0x3A + number - 1);
        }
        for number in 13..=24u16 {
            assert_eq!(hid(&format!("F{number}")), 0x68 + number - 13);
        }

        // The eight modifiers are E0..E7 in this order.
        for (offset, name) in [
            "LEFTCTRL",
            "LEFTSHIFT",
            "LEFTALT",
            "LEFTMETA",
            "RIGHTCTRL",
            "RIGHTSHIFT",
            "RIGHTALT",
            "RIGHTMETA",
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(hid(name), 0xE0 + offset as u16);
        }
    }

    /// Linux key codes up to 88 were defined to equal the PC scan codes, so the Windows and
    /// Linux columns must agree there. This cross-checks one against the other.
    #[test]
    fn windows_scancodes_match_linux_codes_in_the_classic_block() {
        for key in KEYS {
            let (Some(evdev), Some(scancode)) = (key.evdev, key.scancode) else {
                continue;
            };

            if evdev <= 88 && !is_extended(scancode) {
                assert_eq!(evdev, scancode, "{}", key.name);
            }
        }
    }

    #[test]
    fn extended_keys_are_the_navigation_cluster_and_right_hand_modifiers() {
        let extended: Vec<&str> = KEYS
            .iter()
            .filter(|key| key.scancode.is_some_and(is_extended))
            .map(|key| key.name)
            .collect();

        for name in [
            "HOME",
            "END",
            "UP",
            "DOWN",
            "LEFT",
            "RIGHT",
            "INSERT",
            "DELETE",
            "RIGHTCTRL",
            "RIGHTALT",
            "KPENTER",
            "KPSLASH",
        ] {
            assert!(extended.contains(&name), "{name} should be extended");
        }
        for name in ["A", "ENTER", "LEFTCTRL", "LEFTALT", "SPACE", "F1"] {
            assert!(!extended.contains(&name), "{name} should not be extended");
        }
    }

    #[test]
    fn well_known_codes_on_each_platform() {
        // (hid, evdev, windows scancode, macOS virtual key)
        let anchors = [
            (0x04, 30, 0x1E, 0x00),    // A
            (0x28, 28, 0x1C, 0x24),    // Enter
            (0x29, 1, 0x01, 0x35),     // Escape
            (0x2C, 57, 0x39, 0x31),    // Space
            (0xE0, 29, 0x1D, 0x3B),    // Left Ctrl (Control)
            (0xE2, 56, 0x38, 0x3A),    // Left Alt (Option)
            (0xE3, 125, 0xE05B, 0x37), // Left Meta (Windows key / Command)
            (0x4F, 106, 0xE04D, 0x7C), // Right arrow
        ];

        for (hid, evdev, scancode, mac) in anchors {
            assert_eq!(to_evdev(hid), Some(evdev));
            assert_eq!(to_scancode(hid), Some(scancode));
            assert_eq!(to_mac(hid), Some(mac));
        }
    }

    #[test]
    fn keys_a_platform_lacks_are_reported_as_such() {
        // macOS has no Print Screen or Pause; Windows reports Pause specially.
        assert_eq!(to_mac(0x46), None);
        assert_eq!(to_mac(0x48), None);
        assert_eq!(to_scancode(0x48), None);
        assert_eq!(from_evdev(9999), None);
        assert_eq!(from_scancode(0x7777), None);
        assert_eq!(from_mac(0x7F7F), None);
    }

    #[test]
    fn names_are_readable() {
        assert_eq!(name(0x04), "A");
        assert_eq!(name(0xE0), "LEFTCTRL");
        assert_eq!(name(0xFFF0), "0xFFF0");
    }

    /// The Linux column is checked against the kernel's own names, so a typo cannot hide.
    #[cfg(all(target_os = "linux", feature = "linux-backend"))]
    #[test]
    fn linux_codes_agree_with_the_kernels_names() {
        for key in KEYS {
            if let Some(code) = key.evdev {
                assert_eq!(
                    format!("{:?}", evdev::KeyCode::new(code)),
                    format!("KEY_{}", key.name),
                    "evdev code {code}"
                );
            }
        }
    }
}
