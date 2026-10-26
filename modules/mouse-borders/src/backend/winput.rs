//! The Windows-specific *decisions*, kept free of Windows APIs so they are tested everywhere.
//!
//! The hook and `SendInput` glue in `backend/windows` is thin: it hands raw values to the
//! functions here and does what they say. The numeric constants are the documented Win32
//! values; the Windows build asserts at compile time that they equal `windows-sys`'s.

use super::CapturedInput;
use crate::{input::Button, keymap};

// ---- Window messages seen by the low-level hooks --------------------------------------
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const WM_MOUSEMOVE: u32 = 0x0200;
pub const WM_LBUTTONDOWN: u32 = 0x0201;
pub const WM_LBUTTONUP: u32 = 0x0202;
pub const WM_RBUTTONDOWN: u32 = 0x0204;
pub const WM_RBUTTONUP: u32 = 0x0205;
pub const WM_MBUTTONDOWN: u32 = 0x0207;
pub const WM_MBUTTONUP: u32 = 0x0208;
pub const WM_MOUSEWHEEL: u32 = 0x020A;
pub const WM_XBUTTONDOWN: u32 = 0x020B;
pub const WM_XBUTTONUP: u32 = 0x020C;
pub const WM_MOUSEHWHEEL: u32 = 0x020E;

pub const LLKHF_EXTENDED: u32 = 0x01;
pub const LLKHF_INJECTED: u32 = 0x10;
pub const LLMHF_INJECTED: u32 = 0x01;
pub const VK_PAUSE: u32 = 0x13;
pub const XBUTTON1: u32 = 1;
pub const XBUTTON2: u32 = 2;

// ---- SendInput flags -------------------------------------------------------------------
pub const MOUSEEVENTF_MOVE: u32 = 0x0001;
pub const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
pub const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
pub const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
pub const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
pub const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
pub const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
pub const MOUSEEVENTF_XDOWN: u32 = 0x0080;
pub const MOUSEEVENTF_XUP: u32 = 0x0100;
pub const MOUSEEVENTF_WHEEL: u32 = 0x0800;
pub const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
pub const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
pub const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;
pub const KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
pub const KEYEVENTF_KEYUP: u32 = 0x0002;
pub const KEYEVENTF_SCANCODE: u32 = 0x0008;

/// A key event from the keyboard hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    /// Sent by software (including our own injection), not by a keyboard.
    pub injected: bool,
    /// `None` for keys with no neutral equivalent (they are never forwarded).
    pub input: Option<CapturedInput>,
}

/// Interpret a `WH_KEYBOARD_LL` event.
pub fn translate_key(message: u32, scan_code: u32, vk_code: u32, flags: u32) -> KeyEvent {
    let pressed = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
    let injected = flags & LLKHF_INJECTED != 0;

    // Pause has no ordinary scan code (it arrives as a multi-byte sequence).
    let key = if vk_code == VK_PAUSE {
        Some(0x48)
    } else {
        let extended = if flags & LLKHF_EXTENDED != 0 {
            0xE000
        } else {
            0
        };

        keymap::from_scancode(extended | (scan_code as u16 & 0xFF))
    };

    KeyEvent {
        injected,
        input: key.map(|key| CapturedInput::Key { key, pressed }),
    }
}

/// A mouse button change, if `message` is one.
pub fn translate_button(message: u32, mouse_data: u32) -> Option<(Button, bool)> {
    // For X buttons the high word says which one.
    let x_button = match (mouse_data >> 16) & 0xFFFF {
        XBUTTON1 => Some(Button::Back),
        XBUTTON2 => Some(Button::Forward),
        _ => None,
    };

    match message {
        WM_LBUTTONDOWN => Some((Button::Left, true)),
        WM_LBUTTONUP => Some((Button::Left, false)),
        WM_RBUTTONDOWN => Some((Button::Right, true)),
        WM_RBUTTONUP => Some((Button::Right, false)),
        WM_MBUTTONDOWN => Some((Button::Middle, true)),
        WM_MBUTTONUP => Some((Button::Middle, false)),
        WM_XBUTTONDOWN => x_button.map(|button| (button, true)),
        WM_XBUTTONUP => x_button.map(|button| (button, false)),
        _ => None,
    }
}

/// A wheel event, in the 120-per-notch unit used everywhere.
pub fn translate_wheel(message: u32, mouse_data: u32) -> Option<CapturedInput> {
    // The delta is a signed 16-bit value in the high word.
    let delta = i32::from((mouse_data >> 16) as u16 as i16);

    match message {
        WM_MOUSEWHEEL => Some(CapturedInput::Wheel {
            horizontal: 0,
            vertical: delta,
        }),
        WM_MOUSEHWHEEL => Some(CapturedInput::Wheel {
            horizontal: delta,
            vertical: 0,
        }),
        _ => None,
    }
}

/// Turns the absolute cursor positions the mouse hook reports into relative motion.
///
/// While input is *not* captured the cursor really moves, so motion is the change from the
/// previous position. While captured the hook swallows events and the cursor stays where it
/// was parked, so every reported position is the parked position plus that one movement.
#[derive(Debug, Default)]
pub struct MotionTracker {
    last: Option<(i32, i32)>,
    parked_at: Option<(i32, i32)>,
}

impl MotionTracker {
    /// Capture started (`Some(park)`) or ended (`None`).
    pub fn set_capture(&mut self, park: Option<(i32, i32)>) {
        self.parked_at = park;
        // The cursor jumped; do not report the jump as movement.
        self.last = None;
    }

    pub fn is_capturing(&self) -> bool {
        self.parked_at.is_some()
    }

    /// Motion for a hook event at `position`; `None` when there is none to report.
    pub fn delta(&mut self, position: (i32, i32)) -> Option<(i32, i32)> {
        let delta = match self.parked_at {
            Some(park) => (position.0 - park.0, position.1 - park.1),
            None => {
                let previous = self.last.replace(position)?;

                (position.0 - previous.0, position.1 - previous.1)
            }
        };

        (delta != (0, 0)).then_some(delta)
    }

    /// An event we injected moved the cursor: remember where it is, report nothing.
    pub fn note_injected(&mut self, position: (i32, i32)) {
        if self.parked_at.is_none() {
            self.last = Some(position);
        }
    }
}

/// `SendInput` flags and data for a mouse button change.
pub fn button_input(button: Button, pressed: bool) -> (u32, u32) {
    match (button, pressed) {
        (Button::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (Button::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (Button::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (Button::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (Button::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (Button::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (Button::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
        (Button::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
        (Button::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
        (Button::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
    }
}

/// How to inject a key: the scan code byte and the `KEYEVENTF_*` flags. `None` if the key
/// has no Windows scan code.
pub fn key_input(key: u16, pressed: bool) -> Option<(u16, u32)> {
    let scancode = keymap::to_scancode(key)?;
    let mut flags = KEYEVENTF_SCANCODE;

    if keymap::is_extended(scancode) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }

    if !pressed {
        flags |= KEYEVENTF_KEYUP;
    }

    Some((scancode & 0xFF, flags))
}

/// Position on the 0..=65535 scale `SendInput` uses with `MOUSEEVENTF_ABSOLUTE`, for a
/// point `offset` pixels into a span of `extent` pixels.
pub fn normalize_absolute(offset: i32, extent: u32) -> i32 {
    let max = i64::from(extent.saturating_sub(1).max(1));

    (i64::from(offset).clamp(0, max) * 65_535 / max) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_use_the_scan_code_and_the_extended_flag() {
        // A: scan code 0x1E. Right arrow: extended 0x4D.
        let a = translate_key(WM_KEYDOWN, 0x1E, 0x41, 0);
        let right = translate_key(WM_KEYUP, 0x4D, 0x27, LLKHF_EXTENDED);

        assert_eq!(
            a.input,
            Some(CapturedInput::Key {
                key: 0x04,
                pressed: true
            })
        );
        assert_eq!(
            right.input,
            Some(CapturedInput::Key {
                key: 0x4F,
                pressed: false
            })
        );
        assert!(!a.injected);
    }

    #[test]
    fn extended_and_plain_scan_codes_are_different_keys() {
        // 0x1D is Left Ctrl; extended 0x1D is Right Ctrl.
        let left = translate_key(WM_KEYDOWN, 0x1D, 0xA2, 0).input;
        let right = translate_key(WM_KEYDOWN, 0x1D, 0xA3, LLKHF_EXTENDED).input;

        assert_eq!(
            left,
            Some(CapturedInput::Key {
                key: 0xE0,
                pressed: true
            })
        );
        assert_eq!(
            right,
            Some(CapturedInput::Key {
                key: 0xE4,
                pressed: true
            })
        );
    }

    #[test]
    fn system_key_messages_and_pause_are_handled() {
        // Alt combinations arrive as SYSKEYDOWN.
        let alt = translate_key(WM_SYSKEYDOWN, 0x38, 0xA4, 0).input;
        assert_eq!(
            alt,
            Some(CapturedInput::Key {
                key: 0xE2,
                pressed: true
            })
        );

        // Pause is recognised by its virtual key, whatever scan code it carries.
        let pause = translate_key(WM_KEYDOWN, 0x45, VK_PAUSE, 0).input;
        assert_eq!(
            pause,
            Some(CapturedInput::Key {
                key: 0x48,
                pressed: true
            })
        );
    }

    #[test]
    fn injected_events_are_flagged_and_unknown_keys_dropped() {
        assert!(translate_key(WM_KEYDOWN, 0x1E, 0x41, LLKHF_INJECTED).injected);
        // The "fake shift" Windows adds around some extended keys is not a real key.
        assert_eq!(
            translate_key(WM_KEYDOWN, 0x2A, 0xA0, LLKHF_EXTENDED).input,
            None
        );
    }

    #[test]
    fn mouse_buttons_including_the_thumb_buttons() {
        assert_eq!(
            translate_button(WM_LBUTTONDOWN, 0),
            Some((Button::Left, true))
        );
        assert_eq!(
            translate_button(WM_RBUTTONUP, 0),
            Some((Button::Right, false))
        );
        assert_eq!(
            translate_button(WM_MBUTTONDOWN, 0),
            Some((Button::Middle, true))
        );
        assert_eq!(
            translate_button(WM_XBUTTONDOWN, XBUTTON1 << 16),
            Some((Button::Back, true))
        );
        assert_eq!(
            translate_button(WM_XBUTTONUP, XBUTTON2 << 16),
            Some((Button::Forward, false))
        );
        assert_eq!(translate_button(WM_XBUTTONDOWN, 7 << 16), None);
        assert_eq!(translate_button(WM_MOUSEMOVE, 0), None);
    }

    #[test]
    fn wheel_deltas_are_signed_and_per_axis() {
        let up = translate_wheel(WM_MOUSEWHEEL, 120 << 16);
        let down = translate_wheel(WM_MOUSEWHEEL, (-120i16 as u16 as u32) << 16);
        let right = translate_wheel(WM_MOUSEHWHEEL, 240 << 16);

        assert_eq!(
            up,
            Some(CapturedInput::Wheel {
                horizontal: 0,
                vertical: 120
            })
        );
        assert_eq!(
            down,
            Some(CapturedInput::Wheel {
                horizontal: 0,
                vertical: -120
            })
        );
        assert_eq!(
            right,
            Some(CapturedInput::Wheel {
                horizontal: 240,
                vertical: 0
            })
        );
        assert_eq!(translate_wheel(WM_MOUSEMOVE, 0), None);
    }

    #[test]
    fn motion_is_the_change_in_position_while_the_cursor_moves_freely() {
        let mut tracker = MotionTracker::default();

        // The first position only establishes where the cursor is.
        assert_eq!(tracker.delta((100, 100)), None);
        assert_eq!(tracker.delta((105, 98)), Some((5, -2)));
        assert_eq!(tracker.delta((105, 98)), None);
    }

    #[test]
    fn while_captured_motion_is_measured_from_the_parked_position() {
        let mut tracker = MotionTracker::default();

        tracker.set_capture(Some((960, 540)));

        // The cursor never moves (events are swallowed), so each event is parked + movement.
        assert_eq!(tracker.delta((963, 540)), Some((3, 0)));
        assert_eq!(tracker.delta((960, 530)), Some((0, -10)));
        assert_eq!(tracker.delta((960, 540)), None);
        assert!(tracker.is_capturing());
    }

    #[test]
    fn ending_capture_does_not_report_the_jump_back() {
        let mut tracker = MotionTracker::default();

        tracker.delta((10, 10));
        tracker.set_capture(Some((960, 540)));
        tracker.set_capture(None);

        // First event after release only re-establishes the position.
        assert_eq!(tracker.delta((500, 500)), None);
        assert_eq!(tracker.delta((502, 500)), Some((2, 0)));
        assert!(!tracker.is_capturing());
    }

    #[test]
    fn our_own_cursor_jumps_are_not_reported_as_movement() {
        let mut tracker = MotionTracker::default();

        tracker.delta((10, 10));
        tracker.note_injected((800, 600));

        assert_eq!(tracker.delta((801, 600)), Some((1, 0)));
    }

    #[test]
    fn button_injection_flags() {
        assert_eq!(button_input(Button::Left, true), (MOUSEEVENTF_LEFTDOWN, 0));
        assert_eq!(
            button_input(Button::Middle, false),
            (MOUSEEVENTF_MIDDLEUP, 0)
        );
        assert_eq!(button_input(Button::Back, true), (MOUSEEVENTF_XDOWN, 1));
        assert_eq!(button_input(Button::Forward, false), (MOUSEEVENTF_XUP, 2));
    }

    #[test]
    fn key_injection_uses_scan_codes_and_flags() {
        assert_eq!(key_input(0x04, true), Some((0x1E, KEYEVENTF_SCANCODE)));
        assert_eq!(
            key_input(0x04, false),
            Some((0x1E, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP))
        );
        // Right arrow is extended.
        assert_eq!(
            key_input(0x4F, true),
            Some((0x4D, KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY))
        );
        // Pause and macOS-only keys have no scan code.
        assert_eq!(key_input(0x48, true), None);
    }

    #[test]
    fn absolute_coordinates_span_the_whole_range() {
        assert_eq!(normalize_absolute(0, 1920), 0);
        assert_eq!(normalize_absolute(1919, 1920), 65_535);
        assert_eq!(normalize_absolute(960, 1920), 960 * 65_535 / 1919);
        assert_eq!(normalize_absolute(-5, 1920), 0);
        assert_eq!(normalize_absolute(99_999, 1920), 65_535);
        assert_eq!(normalize_absolute(3, 0), 65_535);
    }
}
