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

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            position: Position { x, y },
            size: Size { width, height },
        }
    }

    /// Exclusive right edge (`x + width`).
    pub const fn right(&self) -> i32 {
        self.position.x.saturating_add(self.size.width as i32)
    }

    /// Exclusive bottom edge (`y + height`).
    pub const fn bottom(&self) -> i32 {
        self.position.y.saturating_add(self.size.height as i32)
    }

    pub const fn contains(&self, point: Position) -> bool {
        point.x >= self.position.x
            && point.x < self.right()
            && point.y >= self.position.y
            && point.y < self.bottom()
    }

    /// Nearest point inside the rectangle. An empty rectangle yields its origin.
    pub fn clamp(&self, point: Position) -> Position {
        let max_x = (self.right() - 1).max(self.position.x);
        let max_y = (self.bottom() - 1).max(self.position.y);

        Position {
            x: point.x.clamp(self.position.x, max_x),
            y: point.y.clamp(self.position.y, max_y),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Session {
    X11,
    Wayland,
    MacOs,
    Windows,
    Unknown,
}

/// Operating system hosting Aujar. This is separate from [`Session`]: Linux has several
/// desktop protocols, while Windows and macOS each expose one native desktop API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
    #[default]
    Other,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
            Self::Windows => "Windows",
            Self::Other => "Other",
        }
    }
}

/// A stable capability identifier for the PowerToys-compatible product surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DesktopFeature {
    Launcher,
    PowerRename,
    ImageResizer,
    ColorPicker,
    ScreenRuler,
    ClipboardHistory,
    Awake,
    TextExtractor,
    MouseWithoutBorders,
    AlwaysOnTop,
    FancyZones,
    KeyboardManager,
    WindowManagement,
}

impl DesktopFeature {
    pub const ALL: [Self; 13] = [
        Self::Launcher,
        Self::PowerRename,
        Self::ImageResizer,
        Self::ColorPicker,
        Self::ScreenRuler,
        Self::ClipboardHistory,
        Self::Awake,
        Self::TextExtractor,
        Self::MouseWithoutBorders,
        Self::AlwaysOnTop,
        Self::FancyZones,
        Self::KeyboardManager,
        Self::WindowManagement,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Launcher => "launcher",
            Self::PowerRename => "power_rename",
            Self::ImageResizer => "image_resizer",
            Self::ColorPicker => "color_picker",
            Self::ScreenRuler => "screen_ruler",
            Self::ClipboardHistory => "clipboard_history",
            Self::Awake => "awake",
            Self::TextExtractor => "text_extractor",
            Self::MouseWithoutBorders => "mouse_without_borders",
            Self::AlwaysOnTop => "always_on_top",
            Self::FancyZones => "fancy_zones",
            Self::KeyboardManager => "keyboard_manager",
            Self::WindowManagement => "window_management",
        }
    }
}

/// How far a backend can take a feature on the current desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupportLevel {
    Available,
    Partial,
    Planned,
    Unsupported,
}

impl SupportLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Partial => "partial",
            Self::Planned => "planned",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureSupport {
    pub feature: DesktopFeature,
    pub level: SupportLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Health {
    pub name: String,
    pub version: String,
    pub session: Session,
    #[serde(default)]
    pub platform: Platform,
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
    /// Capture local keyboard/mouse input and inject input into the local desktop.
    ControlInput,
    /// Listen for and connect to other machines on the network.
    Network,
}

impl Capability {
    pub const ALL: [Capability; 10] = [
        Self::ReadClipboard,
        Self::WriteClipboard,
        Self::ReadWindows,
        Self::ControlWindows,
        Self::ScreenCapture,
        Self::GlobalHotkeys,
        Self::ExecuteCommand,
        Self::FileSystem,
        Self::ControlInput,
        Self::Network,
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
            Self::ControlInput => "control_input",
            Self::Network => "network",
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
    use super::{Capability, DesktopFeature, Platform, Position, Rect};

    #[test]
    fn capability_names_round_trip() {
        for capability in Capability::ALL {
            assert_eq!(Capability::parse(capability.as_str()), Some(capability));
        }

        assert_eq!(Capability::parse("nope"), None);
    }

    #[test]
    fn rect_edges_and_containment() {
        let rect = Rect::new(10, 20, 100, 50);

        assert_eq!((rect.right(), rect.bottom()), (110, 70));
        assert!(rect.contains(Position { x: 10, y: 20 }));
        assert!(rect.contains(Position { x: 109, y: 69 }));
        assert!(!rect.contains(Position { x: 110, y: 69 }));
        assert!(!rect.contains(Position { x: 9, y: 20 }));
    }

    #[test]
    fn rect_clamp_stays_inside() {
        let rect = Rect::new(0, 0, 1920, 1080);

        assert_eq!(
            rect.clamp(Position { x: -5, y: 5000 }),
            Position { x: 0, y: 1079 }
        );
        assert_eq!(
            rect.clamp(Position { x: 1920, y: 10 }),
            Position { x: 1919, y: 10 }
        );
        assert_eq!(
            Rect::new(3, 4, 0, 0).clamp(Position { x: 99, y: 99 }),
            Position { x: 3, y: 4 }
        );
    }

    #[test]
    fn platform_and_feature_names_are_stable() {
        assert!(!Platform::current().as_str().is_empty());
        assert_eq!(DesktopFeature::PowerRename.as_str(), "power_rename");
        assert_eq!(DesktopFeature::ALL.len(), 13);
    }
}
