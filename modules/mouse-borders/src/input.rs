//! The vocabulary of input that is the same on every platform.
//!
//! Keys are USB HID usages (see [`crate::keymap`]); mouse buttons are this enum. Each
//! platform backend converts to and from its native codes at the edge.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Button {
    Left,
    Right,
    Middle,
    /// The "back" thumb button.
    Back,
    /// The "forward" thumb button.
    Forward,
}

impl Button {
    pub const ALL: [Button; 5] = [
        Button::Left,
        Button::Right,
        Button::Middle,
        Button::Back,
        Button::Forward,
    ];
}
