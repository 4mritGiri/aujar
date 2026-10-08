use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

impl fmt::Display for Capability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
