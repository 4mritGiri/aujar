//! Shortcut triggers written as in the settings, `CTRL+ALT+space` or `SUPER+SHIFT+S`, and the
//! codes each platform registers them with.

use std::fmt;

/// A key that can end a global shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// `A` to `Z`.
    Letter(u8),
    /// `0` to `9`.
    Digit(u8),
    /// `F1` to `F12`.
    Function(u8),
    Space,
    Tab,
    Period,
    Comma,
    Slash,
    Minus,
    Equal,
}

/// Modifiers and a key. `super_key` is Windows' Win key and macOS' Command key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trigger {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
    pub key: Key,
}

impl std::str::FromStr for Trigger {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let Some((key, modifiers)) = parts.split_last() else {
            return Err("an empty shortcut".to_owned());
        };

        let mut trigger = Trigger {
            ctrl: false,
            alt: false,
            shift: false,
            super_key: false,
            key: parse_key(key).ok_or_else(|| format!("`{key}` cannot end a shortcut"))?,
        };

        for modifier in modifiers {
            match modifier.to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => trigger.ctrl = true,
                "ALT" | "OPTION" | "OPT" => trigger.alt = true,
                "SHIFT" => trigger.shift = true,
                "SUPER" | "WIN" | "META" | "LOGO" | "CMD" | "COMMAND" => trigger.super_key = true,
                other => return Err(format!("unknown modifier `{other}`")),
            }
        }

        // A bare letter would fire whenever someone types it.
        if !(trigger.ctrl || trigger.alt || trigger.super_key)
            && !matches!(trigger.key, Key::Function(_))
        {
            return Err(format!(
                "`{text}` needs Ctrl, Alt or Super (or an F key), or it would fire while typing"
            ));
        }

        Ok(trigger)
    }
}

fn parse_key(name: &str) -> Option<Key> {
    let upper = name.to_ascii_uppercase();
    let bytes = upper.as_bytes();

    Some(match upper.as_str() {
        "SPACE" => Key::Space,
        "TAB" => Key::Tab,
        "." | "PERIOD" => Key::Period,
        "," | "COMMA" => Key::Comma,
        "/" | "SLASH" => Key::Slash,
        "-" | "MINUS" => Key::Minus,
        "=" | "EQUAL" => Key::Equal,
        _ if bytes.len() == 1 && bytes[0].is_ascii_uppercase() => Key::Letter(bytes[0]),
        _ if bytes.len() == 1 && bytes[0].is_ascii_digit() => Key::Digit(bytes[0] - b'0'),
        _ if upper.starts_with('F') => match upper[1..].parse::<u8>() {
            Ok(n @ 1..=12) => Key::Function(n),
            _ => return None,
        },
        _ => return None,
    })
}

impl Trigger {
    /// The Windows virtual-key code (`VK_*`).
    pub fn windows_vk(&self) -> u32 {
        match self.key {
            Key::Letter(letter) => u32::from(letter),
            Key::Digit(digit) => u32::from(b'0' + digit),
            Key::Function(n) => 0x6F + u32::from(n),
            Key::Space => 0x20,
            Key::Tab => 0x09,
            Key::Period => 0xBE,
            Key::Comma => 0xBC,
            Key::Slash => 0xBF,
            Key::Minus => 0xBD,
            Key::Equal => 0xBB,
        }
    }

    /// The macOS virtual key code (`kVK_*`, ANSI positions).
    pub fn mac_keycode(&self) -> u16 {
        const LETTERS: [u16; 26] = [
            0x00, 0x0B, 0x08, 0x02, 0x0E, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2E, 0x2D,
            0x1F, 0x23, 0x0C, 0x0F, 0x01, 0x11, 0x20, 0x09, 0x0D, 0x07, 0x10, 0x06,
        ];
        const DIGITS: [u16; 10] = [0x1D, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1A, 0x1C, 0x19];
        const FUNCTIONS: [u16; 12] = [
            0x7A, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F,
        ];

        match self.key {
            Key::Letter(letter) => LETTERS[usize::from(letter - b'A')],
            Key::Digit(digit) => DIGITS[usize::from(digit)],
            Key::Function(n) => FUNCTIONS[usize::from(n - 1)],
            Key::Space => 0x31,
            Key::Tab => 0x30,
            Key::Period => 0x2F,
            Key::Comma => 0x2B,
            Key::Slash => 0x2C,
            Key::Minus => 0x1B,
            Key::Equal => 0x18,
        }
    }

    /// The XDG shortcuts syntax the portal suggests (`CTRL+ALT+space`, `LOGO+SHIFT+s`).
    pub fn xdg(&self) -> String {
        let mut parts = Vec::new();

        if self.ctrl {
            parts.push("CTRL".to_owned());
        }
        if self.alt {
            parts.push("ALT".to_owned());
        }
        if self.shift {
            parts.push("SHIFT".to_owned());
        }
        if self.super_key {
            parts.push("LOGO".to_owned());
        }

        parts.push(match self.key {
            Key::Letter(letter) => (letter as char).to_ascii_lowercase().to_string(),
            Key::Digit(digit) => digit.to_string(),
            Key::Function(n) => format!("F{n}"),
            Key::Space => "space".to_owned(),
            Key::Tab => "Tab".to_owned(),
            Key::Period => "period".to_owned(),
            Key::Comma => "comma".to_owned(),
            Key::Slash => "slash".to_owned(),
            Key::Minus => "minus".to_owned(),
            Key::Equal => "equal".to_owned(),
        });

        parts.join("+")
    }
}

/// For people: `Ctrl+Alt+Space`, with the platform's name for Super.
impl fmt::Display for Trigger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let super_name = if cfg!(target_os = "macos") {
            "Cmd"
        } else if cfg!(windows) {
            "Win"
        } else {
            "Super"
        };
        let alt_name = if cfg!(target_os = "macos") {
            "Option"
        } else {
            "Alt"
        };

        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, alt_name),
            (self.shift, "Shift"),
            (self.super_key, super_name),
        ] {
            if on {
                write!(formatter, "{name}+")?;
            }
        }

        match self.key {
            Key::Letter(letter) => write!(formatter, "{}", letter as char),
            Key::Digit(digit) => write!(formatter, "{digit}"),
            Key::Function(n) => write!(formatter, "F{n}"),
            Key::Space => write!(formatter, "Space"),
            Key::Tab => write!(formatter, "Tab"),
            Key::Period => write!(formatter, "."),
            Key::Comma => write!(formatter, ","),
            Key::Slash => write!(formatter, "/"),
            Key::Minus => write!(formatter, "-"),
            Key::Equal => write!(formatter, "="),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Trigger {
        text.parse().unwrap()
    }

    #[test]
    fn parses_settings_spellings() {
        let launcher = parse("CTRL+ALT+space");
        assert!(launcher.ctrl && launcher.alt && !launcher.shift && !launcher.super_key);
        assert_eq!(launcher.key, Key::Space);

        let snip = parse("super + shift + s");
        assert!(snip.super_key && snip.shift);
        assert_eq!(snip.key, Key::Letter(b'S'));

        assert_eq!(parse("Cmd+Shift+4").key, Key::Digit(4));
        assert_eq!(parse("F9").key, Key::Function(9));
    }

    #[test]
    fn refuses_what_cannot_be_a_global_shortcut() {
        assert!("S".parse::<Trigger>().is_err());
        assert!("SHIFT+S".parse::<Trigger>().is_err());
        assert!("CTRL+HYPER+S".parse::<Trigger>().is_err());
        assert!("CTRL+F13".parse::<Trigger>().is_err());
        assert!("CTRL+ALT+".parse::<Trigger>().is_err());
    }

    #[test]
    fn windows_virtual_keys() {
        assert_eq!(parse("CTRL+S").windows_vk(), 0x53);
        assert_eq!(parse("CTRL+5").windows_vk(), 0x35);
        assert_eq!(parse("F1").windows_vk(), 0x70);
        assert_eq!(parse("F12").windows_vk(), 0x7B);
        assert_eq!(parse("CTRL+ALT+space").windows_vk(), 0x20);
    }

    #[test]
    fn mac_key_codes() {
        assert_eq!(parse("CMD+A").mac_keycode(), 0x00);
        assert_eq!(parse("CMD+S").mac_keycode(), 0x01);
        assert_eq!(parse("CMD+T").mac_keycode(), 0x11);
        assert_eq!(parse("CMD+0").mac_keycode(), 0x1D);
        assert_eq!(parse("CTRL+ALT+space").mac_keycode(), 0x31);
        assert_eq!(parse("F5").mac_keycode(), 0x60);
    }

    #[test]
    fn portal_syntax_and_labels() {
        assert_eq!(parse("SUPER+SHIFT+S").xdg(), "SHIFT+LOGO+s");
        assert_eq!(parse("ctrl+alt+SPACE").xdg(), "CTRL+ALT+space");
        assert!(parse("CTRL+ALT+space").to_string().ends_with("Space"));
    }
}
