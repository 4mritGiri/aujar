use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type WindowId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub position: Position,
    pub size: Size,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Session {
    X11,
    Wayland,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub name: String,
    pub version: String,
    pub session: Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleId(pub Uuid);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    ReadClipboard,
    WriteClipboard,
    ReadWindows,
    ControlWindows,
    ScreenCapture,
    GlobalHotkeys,
    ExecuteCommand,
    FileSystem,
}

impl Capability {
    pub const ALL: [Capability; 8] = [
        Self::ReadClipboard,
        Self::WriteClipboard,
        Self::ReadWindows,
        Self::ControlWindows,
        Self::ScreenCapture,
        Self::GlobalHotkeys,
        Self::ExecuteCommand,
        Self::FileSystem,
    ];

    /// Parse the stable snake_case name produced by [`Capability::as_str`].
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|capability| capability.as_str() == name)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadClipboard => "read_clipboard",
            Self::WriteClipboard => "write_clipboard",
            Self::ReadWindows => "read_windows",
            Self::ControlWindows => "control_windows",
            Self::ScreenCapture => "screen_capture",
            Self::GlobalHotkeys => "global_hotkeys",
            Self::ExecuteCommand => "execute_command",
            Self::FileSystem => "filesystem",
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::Capability;

    #[test]
    fn capability_names_round_trip() {
        for capability in Capability::ALL {
            assert_eq!(Capability::parse(capability.as_str()), Some(capability));
        }

        assert_eq!(Capability::parse("nope"), None);
    }
}
