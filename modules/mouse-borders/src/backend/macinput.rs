//! The macOS-specific *decisions*, kept free of Apple APIs so they are tested everywhere.
//!
//! The event-tap and `CGEventPost` glue in `backend/macos` is thin: it hands raw values to the
//! functions here and does what they say. The numeric constants are the documented Quartz
//! values; the macOS build asserts at compile time that they equal `core-graphics`'s.

use std::{collections::BTreeSet, time::Duration};

use crate::{input::Button, keymap};

// ---- Quartz event types (CGEventType) ---------------------------------------------------
pub const LEFT_MOUSE_DOWN: u32 = 1;
pub const LEFT_MOUSE_UP: u32 = 2;
pub const RIGHT_MOUSE_DOWN: u32 = 3;
pub const RIGHT_MOUSE_UP: u32 = 4;
pub const MOUSE_MOVED: u32 = 5;
pub const LEFT_MOUSE_DRAGGED: u32 = 6;
pub const RIGHT_MOUSE_DRAGGED: u32 = 7;
pub const KEY_DOWN: u32 = 10;
pub const KEY_UP: u32 = 11;
pub const FLAGS_CHANGED: u32 = 12;
pub const SCROLL_WHEEL: u32 = 22;
pub const OTHER_MOUSE_DOWN: u32 = 25;
pub const OTHER_MOUSE_UP: u32 = 26;
pub const OTHER_MOUSE_DRAGGED: u32 = 27;

/// Marker we stamp on our own events (in the "source user data" field) so the event tap can
/// recognise and ignore them.
pub const OUR_EVENT_MARKER: i64 = 0x4155_4A52; // "AUJR"

/// Device-independent modifier flags as set on a `CGEvent`.
pub const FLAG_SHIFT: u64 = 0x0002_0000;
pub const FLAG_CONTROL: u64 = 0x0004_0000;
pub const FLAG_ALTERNATE: u64 = 0x0008_0000;
pub const FLAG_COMMAND: u64 = 0x0010_0000;

/// One modifier key: which virtual key it is, and its flag bits.
struct ModifierKey {
    keycode: u16,
    hid: u16,
    /// Per-side bit (from IOKit's `NX_DEVICE*KEYMASK`) present in the event flags.
    device_mask: u64,
    /// Device-independent bit, used when an event carries no per-side bits at all.
    generic_mask: u64,
}

const MODIFIERS: [ModifierKey; 8] = [
    ModifierKey {
        keycode: 0x3B,
        hid: 0xE0,
        device_mask: 0x0000_0001,
        generic_mask: FLAG_CONTROL,
    },
    ModifierKey {
        keycode: 0x38,
        hid: 0xE1,
        device_mask: 0x0000_0002,
        generic_mask: FLAG_SHIFT,
    },
    ModifierKey {
        keycode: 0x3A,
        hid: 0xE2,
        device_mask: 0x0000_0020,
        generic_mask: FLAG_ALTERNATE,
    },
    ModifierKey {
        keycode: 0x37,
        hid: 0xE3,
        device_mask: 0x0000_0008,
        generic_mask: FLAG_COMMAND,
    },
    ModifierKey {
        keycode: 0x3E,
        hid: 0xE4,
        device_mask: 0x0000_2000,
        generic_mask: FLAG_CONTROL,
    },
    ModifierKey {
        keycode: 0x3C,
        hid: 0xE5,
        device_mask: 0x0000_0004,
        generic_mask: FLAG_SHIFT,
    },
    ModifierKey {
        keycode: 0x3D,
        hid: 0xE6,
        device_mask: 0x0000_0040,
        generic_mask: FLAG_ALTERNATE,
    },
    ModifierKey {
        keycode: 0x36,
        hid: 0xE7,
        device_mask: 0x0000_0010,
        generic_mask: FLAG_COMMAND,
    },
];

const ALL_DEVICE_MASKS: u64 = 0x0000_2000
    | 0x0000_0001
    | 0x0000_0002
    | 0x0000_0004
    | 0x0000_0008
    | 0x0000_0010
    | 0x0000_0020
    | 0x0000_0040;

/// Virtual key code of Caps Lock, which reports state rather than press and release.
pub const CAPS_LOCK_KEYCODE: u16 = 0x39;

/// macOS reports modifier keys as "flags changed" events carrying the key's code and the new
/// flag state. Returns the HID key and whether it went down. `None` if `keycode` is not a
/// modifier.
pub fn modifier_change(keycode: u16, flags: u64) -> Option<(u16, bool)> {
    let modifier = MODIFIERS.iter().find(|m| m.keycode == keycode)?;

    let pressed = if flags & ALL_DEVICE_MASKS != 0 {
        flags & modifier.device_mask != 0
    } else {
        flags & modifier.generic_mask != 0
    };

    Some((modifier.hid, pressed))
}

/// A key down/up event, as a HID key. `None` for keys with no neutral equivalent.
pub fn translate_key(keycode: u16) -> Option<u16> {
    keymap::from_mac(keycode)
}

/// Mouse button numbers as macOS numbers them for "other" buttons.
const BUTTON_MIDDLE: i64 = 2;
const BUTTON_BACK: i64 = 3;
const BUTTON_FORWARD: i64 = 4;

/// A button change from an event type and (for "other" buttons) the button number.
pub fn translate_button(event_type: u32, button_number: i64) -> Option<(Button, bool)> {
    let other = |pressed| match button_number {
        BUTTON_MIDDLE => Some((Button::Middle, pressed)),
        BUTTON_BACK => Some((Button::Back, pressed)),
        BUTTON_FORWARD => Some((Button::Forward, pressed)),
        _ => None,
    };

    match event_type {
        LEFT_MOUSE_DOWN => Some((Button::Left, true)),
        LEFT_MOUSE_UP => Some((Button::Left, false)),
        RIGHT_MOUSE_DOWN => Some((Button::Right, true)),
        RIGHT_MOUSE_UP => Some((Button::Right, false)),
        OTHER_MOUSE_DOWN => other(true),
        OTHER_MOUSE_UP => other(false),
        _ => None,
    }
}

/// Whether the event type is mouse motion (with or without a button held).
pub fn is_motion(event_type: u32) -> bool {
    matches!(
        event_type,
        MOUSE_MOVED | LEFT_MOUSE_DRAGGED | RIGHT_MOUSE_DRAGGED | OTHER_MOUSE_DRAGGED
    )
}

/// The event type to post for a button change.
pub fn button_event_type(button: Button, pressed: bool) -> u32 {
    match (button, pressed) {
        (Button::Left, true) => LEFT_MOUSE_DOWN,
        (Button::Left, false) => LEFT_MOUSE_UP,
        (Button::Right, true) => RIGHT_MOUSE_DOWN,
        (Button::Right, false) => RIGHT_MOUSE_UP,
        (_, true) => OTHER_MOUSE_DOWN,
        (_, false) => OTHER_MOUSE_UP,
    }
}

/// The button number to stamp on an "other" button event.
pub fn button_number(button: Button) -> i64 {
    match button {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => BUTTON_MIDDLE,
        Button::Back => BUTTON_BACK,
        Button::Forward => BUTTON_FORWARD,
    }
}

/// While a button is held, motion must be posted as a *drag* or applications will not see it.
pub fn motion_event_type(held: &BTreeSet<Button>) -> u32 {
    if held.contains(&Button::Left) {
        LEFT_MOUSE_DRAGGED
    } else if held.contains(&Button::Right) {
        RIGHT_MOUSE_DRAGGED
    } else if held.is_empty() {
        MOUSE_MOVED
    } else {
        OTHER_MOUSE_DRAGGED
    }
}

/// Turns the hi-resolution wheel units used on the wire (120 per notch) into whole lines,
/// keeping the remainder so slow scrolling is not lost.
#[derive(Debug, Default)]
pub struct ScrollAccumulator {
    horizontal: i32,
    vertical: i32,
}

impl ScrollAccumulator {
    /// Add scroll amounts; returns whole `(horizontal, vertical)` lines to post now.
    pub fn add(&mut self, horizontal: i32, vertical: i32) -> (i32, i32) {
        self.horizontal += horizontal;
        self.vertical += vertical;

        let lines = (self.horizontal / 120, self.vertical / 120);

        self.horizontal %= 120;
        self.vertical %= 120;

        lines
    }
}

/// The modifier flags to put on every event we post, from the modifier keys currently held.
/// Applications read Cmd+C and friends from these flags, not from the key sequence.
#[derive(Debug, Default)]
pub struct HeldModifiers {
    held: BTreeSet<u16>,
}

impl HeldModifiers {
    pub fn update(&mut self, hid: u16, pressed: bool) {
        if !(0xE0..=0xE7).contains(&hid) {
            return;
        }

        if pressed {
            self.held.insert(hid);
        } else {
            self.held.remove(&hid);
        }
    }

    pub fn flags(&self) -> u64 {
        self.held.iter().fold(0, |flags, hid| {
            flags
                | match hid {
                    0xE0 | 0xE4 => FLAG_CONTROL,
                    0xE1 | 0xE5 => FLAG_SHIFT,
                    0xE2 | 0xE6 => FLAG_ALTERNATE,
                    _ => FLAG_COMMAND,
                }
        })
    }
}

/// Counts consecutive clicks so double and triple clicks work on the machine we control.
#[derive(Debug, Default)]
pub struct ClickTracker {
    last: Option<LastClick>,
}

#[derive(Debug)]
struct LastClick {
    button: Button,
    at: Duration,
    position: (f64, f64),
    count: i64,
}

/// Clicks within this time and distance of each other are one multi-click.
const MULTI_CLICK_TIME: Duration = Duration::from_millis(500);
const MULTI_CLICK_DISTANCE: f64 = 4.0;

impl ClickTracker {
    /// A button went down at `position`, `at` this time since some fixed start. Returns the
    /// click count (1 for a single click, 2 for a double click, ...).
    pub fn press(&mut self, button: Button, position: (f64, f64), at: Duration) -> i64 {
        let count = match &self.last {
            Some(last)
                if last.button == button
                    && at.saturating_sub(last.at) <= MULTI_CLICK_TIME
                    && (position.0 - last.position.0).abs() <= MULTI_CLICK_DISTANCE
                    && (position.1 - last.position.1).abs() <= MULTI_CLICK_DISTANCE =>
            {
                last.count + 1
            }
            _ => 1,
        };

        self.last = Some(LastClick {
            button,
            at,
            position,
            count,
        });

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LCTRL_BIT: u64 = 0x1;
    const RCTRL_BIT: u64 = 0x2000;
    const LSHIFT_BIT: u64 = 0x2;

    #[test]
    fn modifier_keys_report_which_side_went_down() {
        // Left Control down: its device bit is set, along with the generic flag.
        assert_eq!(
            modifier_change(0x3B, FLAG_CONTROL | LCTRL_BIT),
            Some((0xE0, true))
        );
        // Right Control down while left is not.
        assert_eq!(
            modifier_change(0x3E, FLAG_CONTROL | RCTRL_BIT),
            Some((0xE4, true))
        );
        // Left Control released while right is still held: the flag stays set, the bit is gone.
        assert_eq!(
            modifier_change(0x3B, FLAG_CONTROL | RCTRL_BIT),
            Some((0xE0, false))
        );
        // Both released.
        assert_eq!(modifier_change(0x3E, 0), Some((0xE4, false)));
    }

    #[test]
    fn events_without_per_side_bits_fall_back_to_the_generic_flag() {
        assert_eq!(modifier_change(0x38, FLAG_SHIFT), Some((0xE1, true)));
        assert_eq!(modifier_change(0x38, 0), Some((0xE1, false)));
        // Mixed: a different modifier's device bit is present, ours is not.
        assert_eq!(
            modifier_change(0x38, FLAG_SHIFT | LCTRL_BIT),
            Some((0xE1, false))
        );
        assert_eq!(
            modifier_change(0x38, FLAG_SHIFT | LSHIFT_BIT),
            Some((0xE1, true))
        );
    }

    #[test]
    fn option_and_command_map_to_alt_and_meta() {
        assert_eq!(modifier_change(0x3A, 0x20), Some((0xE2, true))); // Option
        assert_eq!(modifier_change(0x37, 0x08), Some((0xE3, true))); // Command
        assert_eq!(modifier_change(0x36, 0x10), Some((0xE7, true))); // Right Command
        assert_eq!(modifier_change(0x3D, 0x40), Some((0xE6, true))); // Right Option
        assert_eq!(modifier_change(0x00, 0xFFFF), None); // 'A' is not a modifier
    }

    #[test]
    fn ordinary_keys_translate_through_the_table() {
        assert_eq!(translate_key(0x00), Some(0x04)); // A
        assert_eq!(translate_key(0x24), Some(0x28)); // Return
        assert_eq!(translate_key(0x7B), Some(0x50)); // Left arrow
        assert_eq!(translate_key(0x7F7F), None);
    }

    #[test]
    fn buttons_including_the_numbered_other_buttons() {
        assert_eq!(
            translate_button(LEFT_MOUSE_DOWN, 0),
            Some((Button::Left, true))
        );
        assert_eq!(
            translate_button(RIGHT_MOUSE_UP, 1),
            Some((Button::Right, false))
        );
        assert_eq!(
            translate_button(OTHER_MOUSE_DOWN, 2),
            Some((Button::Middle, true))
        );
        assert_eq!(
            translate_button(OTHER_MOUSE_DOWN, 3),
            Some((Button::Back, true))
        );
        assert_eq!(
            translate_button(OTHER_MOUSE_UP, 4),
            Some((Button::Forward, false))
        );
        assert_eq!(translate_button(OTHER_MOUSE_DOWN, 9), None);
        assert_eq!(translate_button(MOUSE_MOVED, 0), None);
    }

    #[test]
    fn injection_types_round_trip_with_translation() {
        for button in Button::ALL {
            for pressed in [true, false] {
                let event = button_event_type(button, pressed);

                assert_eq!(
                    translate_button(event, button_number(button)),
                    Some((button, pressed)),
                    "{button:?} {pressed}"
                );
            }
        }
    }

    #[test]
    fn motion_with_a_button_held_is_posted_as_a_drag() {
        let mut held = BTreeSet::new();

        assert_eq!(motion_event_type(&held), MOUSE_MOVED);
        held.insert(Button::Left);
        assert_eq!(motion_event_type(&held), LEFT_MOUSE_DRAGGED);
        held.insert(Button::Right);
        assert_eq!(motion_event_type(&held), LEFT_MOUSE_DRAGGED);
        held.remove(&Button::Left);
        assert_eq!(motion_event_type(&held), RIGHT_MOUSE_DRAGGED);
        held.clear();
        held.insert(Button::Middle);
        assert_eq!(motion_event_type(&held), OTHER_MOUSE_DRAGGED);
        assert!(is_motion(LEFT_MOUSE_DRAGGED) && is_motion(MOUSE_MOVED) && !is_motion(KEY_DOWN));
    }

    #[test]
    fn scrolling_accumulates_fractions_of_a_notch() {
        let mut scroll = ScrollAccumulator::default();

        assert_eq!(scroll.add(0, 120), (0, 1));
        assert_eq!(scroll.add(0, 40), (0, 0));
        assert_eq!(scroll.add(0, 40), (0, 0));
        assert_eq!(scroll.add(0, 40), (0, 1), "three thirds make a line");
        assert_eq!(scroll.add(-240, -120), (-2, -1));
        assert_eq!(scroll.add(0, -60), (0, 0));
        assert_eq!(scroll.add(0, -60), (0, -1));
    }

    #[test]
    fn held_modifiers_become_flags_for_posted_events() {
        let mut held = HeldModifiers::default();

        assert_eq!(held.flags(), 0);
        held.update(0xE3, true); // Command
        held.update(0xE1, true); // Shift
        assert_eq!(held.flags(), FLAG_COMMAND | FLAG_SHIFT);
        held.update(0xE5, true); // right Shift as well: still one Shift flag
        held.update(0xE1, false);
        assert_eq!(held.flags(), FLAG_COMMAND | FLAG_SHIFT);
        held.update(0xE5, false);
        held.update(0xE3, false);
        assert_eq!(held.flags(), 0);
        // A normal key is not a modifier.
        held.update(0x04, true);
        assert_eq!(held.flags(), 0);
    }

    #[test]
    fn rapid_clicks_in_place_count_up_and_slow_or_distant_ones_reset() {
        let mut clicks = ClickTracker::default();
        let ms = Duration::from_millis;

        assert_eq!(clicks.press(Button::Left, (100.0, 100.0), ms(0)), 1);
        assert_eq!(clicks.press(Button::Left, (101.0, 100.0), ms(200)), 2);
        assert_eq!(clicks.press(Button::Left, (100.0, 101.0), ms(400)), 3);
        // Too slow.
        assert_eq!(clicks.press(Button::Left, (100.0, 100.0), ms(1200)), 1);
        // Too far.
        assert_eq!(clicks.press(Button::Left, (300.0, 100.0), ms(1300)), 1);
        // A different button starts over.
        assert_eq!(clicks.press(Button::Right, (300.0, 100.0), ms(1400)), 1);
    }
}
