//! Everything that touches the desktop, behind traits.
//!
//! The engine only talks to these traits, so the same logic runs against the real backends
//! (Linux, Windows, macOS), the in-memory ones used by tests, or a future Wayland InputCapture
//! backend. Backends speak the platform-neutral vocabulary of [`crate::input`] and
//! [`crate::keymap`]; converting to native codes is their job.
//! A backend that cannot do something returns [`Error::Unsupported`] with the reason, and the
//! engine reports it instead of failing silently.

use std::collections::BTreeSet;

use async_trait::async_trait;
use aujar_core::{Position, Size};

use crate::{
    error::{Error, Result},
    input::Button,
    protocol::InputEvent,
};

#[cfg(any(windows, target_os = "macos"))]
pub mod clipboard_arboard;
#[cfg(all(target_os = "linux", feature = "linux-backend"))]
pub mod linux;
pub mod macinput;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod memory;
pub mod unavailable;
#[cfg(windows)]
pub mod windows;
pub mod winput;

/// Local input as captured from the keyboard and mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapturedInput {
    PointerMotion {
        dx: i32,
        dy: i32,
    },
    Button {
        button: Button,
        pressed: bool,
    },
    /// 120 units per notch.
    Wheel {
        horizontal: i32,
        vertical: i32,
    },
    /// A key by USB HID usage.
    Key {
        key: u16,
        pressed: bool,
    },
}

/// Reads local input devices.
#[async_trait]
pub trait InputSource: Send {
    /// The next captured event; `Ok(None)` when the source has closed. Must be cancel-safe:
    /// the engine races it against other work.
    async fn next(&mut self) -> Result<Option<CapturedInput>>;

    /// While grabbed, captured input no longer reaches the local desktop.
    fn set_grab(&mut self, grabbed: bool) -> Result<()>;
}

/// Low-level input injection into the local desktop. Use [`Injector`] rather than calling
/// this directly: it tracks what is held down.
pub trait InputSink: Send {
    /// Warp the pointer to an absolute position in desktop coordinates.
    fn pointer_abs(&mut self, position: Position) -> Result<()>;
    fn pointer_rel(&mut self, dx: i32, dy: i32) -> Result<()>;
    fn button(&mut self, button: Button, pressed: bool) -> Result<()>;
    /// 120 units per notch.
    fn wheel(&mut self, horizontal: i32, vertical: i32) -> Result<()>;
    /// Press or release a key, by USB HID usage.
    fn key(&mut self, key: u16, pressed: bool) -> Result<()>;
    /// Produce harmless input that resets idle timers (screen saver, auto-lock).
    fn nudge(&mut self) -> Result<()>;
    /// Tell the sink the desktop size, for backends that map absolute positions.
    fn set_screen(&mut self, _size: Size) {}
}

/// Wraps an [`InputSink`] and remembers which keys and buttons it holds down, so control
/// can be taken away without leaving anything stuck.
pub struct Injector {
    sink: Box<dyn InputSink>,
    keys: BTreeSet<u16>,
    buttons: BTreeSet<Button>,
}

impl Injector {
    pub fn new(sink: Box<dyn InputSink>) -> Self {
        Self {
            sink,
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
        }
    }

    pub fn apply(&mut self, event: &InputEvent) -> Result<()> {
        match *event {
            InputEvent::PointerAbs { x, y } => self.sink.pointer_abs(Position { x, y }),
            InputEvent::PointerRel { dx, dy } => self.sink.pointer_rel(dx, dy),
            InputEvent::Button { button, pressed } => {
                track(&mut self.buttons, button, pressed);
                self.sink.button(button, pressed)
            }
            InputEvent::Wheel {
                horizontal,
                vertical,
            } => self.sink.wheel(horizontal, vertical),
            InputEvent::Key { key, pressed } => {
                track(&mut self.keys, key, pressed);
                self.sink.key(key, pressed)
            }
        }
    }

    pub fn warp(&mut self, position: Position) -> Result<()> {
        self.sink.pointer_abs(position)
    }

    pub fn nudge(&mut self) -> Result<()> {
        self.sink.nudge()
    }

    pub fn set_screen(&mut self, size: Size) {
        self.sink.set_screen(size);
    }

    /// Release keys that were held on the physical keyboard when it was grabbed. The desktop
    /// saw them go down; this lets it see them go up.
    pub fn release_keys(&mut self, keys: &[u16]) -> Result<()> {
        let mut result = Ok(());

        for &key in keys {
            if let Err(error) = self.sink.key(key, false) {
                result = Err(error);
            }
        }

        result
    }

    /// Release everything this injector pressed.
    pub fn release_all(&mut self) -> Result<()> {
        let mut result = Ok(());

        for key in std::mem::take(&mut self.keys) {
            if let Err(error) = self.sink.key(key, false) {
                result = Err(error);
            }
        }

        for button in std::mem::take(&mut self.buttons) {
            if let Err(error) = self.sink.button(button, false) {
                result = Err(error);
            }
        }

        result
    }
}

fn track<T: Ord>(held: &mut BTreeSet<T>, item: T, pressed: bool) {
    if pressed {
        held.insert(item);
    } else {
        held.remove(&item);
    }
}

/// What the engine needs to know about the local desktop.
pub trait Desktop: Send + Sync {
    /// Size of the whole desktop (bounding box of all monitors).
    fn screen_size(&self) -> Result<Size>;

    /// Absolute pointer position. Backends that cannot read it (most Wayland compositors)
    /// return [`Error::Unsupported`]; edge switching is then unavailable, shortcuts still work.
    fn pointer(&self) -> Result<Position>;

    fn set_cursor_visible(&self, _visible: bool) -> Result<()> {
        Err(Error::unsupported(
            "hiding the pointer",
            "this session does not allow it",
        ))
    }
}

/// Content shared through the clipboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardContent {
    Text(String),
    Png(Vec<u8>),
}

#[async_trait]
pub trait Clipboard: Send + Sync {
    /// A cheap value that changes whenever the clipboard does, where the OS offers one
    /// (Windows sequence number, macOS change count). Lets the engine skip reading (and
    /// re-encoding a large image) while nothing changed.
    fn change_token(&self) -> Option<u64> {
        None
    }

    async fn read(&self) -> Result<Option<ClipboardContent>>;
    async fn write(&self, content: &ClipboardContent) -> Result<()>;
}

#[async_trait]
pub trait ScreenLocker: Send + Sync {
    async fn lock(&self) -> Result<()>;
}

/// The best backends this operating system supports: evdev/uinput/X11 on Linux, hooks and
/// `SendInput` on Windows, an event tap and Quartz events on macOS. Anywhere else the pieces
/// are stand-ins that report why nothing works.
pub fn platform_backends(config: &crate::config::EngineConfig) -> Backends {
    #[cfg(all(target_os = "linux", feature = "linux-backend"))]
    {
        linux::backends(config)
    }

    #[cfg(windows)]
    {
        windows::backends(config)
    }

    #[cfg(target_os = "macos")]
    {
        macos::backends(config)
    }

    #[cfg(not(any(
        all(target_os = "linux", feature = "linux-backend"),
        windows,
        target_os = "macos"
    )))]
    {
        let _ = config;
        let reason = "this operating system (or build) has no Mouse Borders backend";

        Backends {
            source: Box::new(unavailable::UnavailableSource::new(reason)),
            sink: Box::new(unavailable::UnavailableSink::new(reason)),
            desktop: Box::new(unavailable::UnavailableDesktop::new(reason)),
            clipboard: None,
            locker: Box::new(unavailable::UnavailableLocker::new(reason)),
            notes: vec![reason.to_owned()],
        }
    }
}

/// Everything the engine needs from the desktop.
pub struct Backends {
    pub source: Box<dyn InputSource>,
    pub sink: Box<dyn InputSink>,
    pub desktop: Box<dyn Desktop>,
    /// `None` when clipboard sharing is unavailable or switched off.
    pub clipboard: Option<Box<dyn Clipboard>>,
    pub locker: Box<dyn ScreenLocker>,
    /// Things worth telling the user at startup (a backend that fell back or is unavailable).
    pub notes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::{memory::MemorySink, *};

    #[test]
    fn injector_releases_only_what_it_pressed() {
        let (sink, log) = MemorySink::new();
        let mut injector = Injector::new(Box::new(sink));

        injector
            .apply(&InputEvent::Key {
                key: 0x04,
                pressed: true,
            })
            .unwrap();
        injector
            .apply(&InputEvent::Key {
                key: 0x16,
                pressed: true,
            })
            .unwrap();
        injector
            .apply(&InputEvent::Key {
                key: 0x16,
                pressed: false,
            })
            .unwrap();
        injector
            .apply(&InputEvent::Button {
                button: Button::Left,
                pressed: true,
            })
            .unwrap();
        log.clear();

        injector.release_all().unwrap();

        let events = log.take();
        assert_eq!(events.len(), 2, "{events:?}");

        // Nothing left to release the second time.
        injector.release_all().unwrap();
        assert!(log.take().is_empty());
    }

    #[test]
    fn release_keys_lets_the_desktop_see_key_ups() {
        let (sink, log) = MemorySink::new();
        let mut injector = Injector::new(Box::new(sink));

        injector.release_keys(&[0xE0, 0xE1]).unwrap();

        assert_eq!(log.take().len(), 2);
    }
}
