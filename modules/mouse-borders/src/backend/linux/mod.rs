//! Linux backends: evdev/uinput for input, X11 for the pointer, CLI tools for the clipboard.
//!
//! Works on X11 and Wayland alike for input, with one difference: only X11 lets us read the
//! pointer position, so on Wayland switching at screen edges is unavailable (shortcuts still
//! work). Whatever is missing is reported in [`Backends::notes`] and the status.

mod clipboard_cli;
mod drm;
mod evdev_source;
mod logind;
mod uinput_sink;
mod x11;

use std::path::Path;

use async_trait::async_trait;
use aujar_core::{Position, Size};

pub use clipboard_cli::CliClipboard;
pub use evdev_source::EvdevSource;
pub use logind::LogindLocker;
pub use uinput_sink::{UinputSink, VIRTUAL_PREFIX};
pub use x11::X11Desktop;

use super::{Backends, CapturedInput, Clipboard, Desktop, InputSink, InputSource};
use crate::{
    config::EngineConfig,
    error::{Error, Result},
};

/// Which kind of graphical session this process runs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    X11,
    Wayland,
    /// No desktop session (a system service).
    Headless,
}

impl Session {
    pub fn detect() -> Self {
        let var = |name: &str| std::env::var(name).unwrap_or_default().to_lowercase();
        let session_type = var("XDG_SESSION_TYPE");

        match session_type.as_str() {
            "x11" => Self::X11,
            "wayland" => Self::Wayland,
            _ if std::env::var_os("WAYLAND_DISPLAY").is_some() => Self::Wayland,
            _ if std::env::var_os("DISPLAY").is_some() => Self::X11,
            _ => Self::Headless,
        }
    }
}

/// An input source that explains why there is none.
struct UnavailableSource(String);

#[async_trait]
impl InputSource for UnavailableSource {
    async fn next(&mut self) -> Result<Option<CapturedInput>> {
        Err(Error::unsupported("input capture", &self.0))
    }

    fn set_grab(&mut self, _grabbed: bool) -> Result<()> {
        Err(Error::unsupported("input capture", &self.0))
    }
}

/// An input sink that explains why there is none.
struct UnavailableSink(String);

impl InputSink for UnavailableSink {
    fn pointer_abs(&mut self, _: Position) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn pointer_rel(&mut self, _: i32, _: i32) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn button(&mut self, _: u16, _: bool) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn wheel(&mut self, _: i32, _: i32) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn key(&mut self, _: u16, _: bool) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }

    fn nudge(&mut self) -> Result<()> {
        Err(Error::unsupported("input injection", &self.0))
    }
}

/// A desktop whose pointer position cannot be read (Wayland, or no session at all).
struct BlindDesktop {
    size: Result<Size>,
    reason: &'static str,
}

impl Desktop for BlindDesktop {
    fn screen_size(&self) -> Result<Size> {
        match &self.size {
            Ok(size) => Ok(*size),
            Err(error) => Err(Error::Other(error.to_string())),
        }
    }

    fn pointer(&self) -> Result<Position> {
        Err(Error::unsupported("switching at screen edges", self.reason))
    }
}

fn blind_desktop(config: &EngineConfig, session: Session) -> BlindDesktop {
    // An explicit setting beats guessing from the kernel's view of the outputs.
    let size = config
        .screen_override
        .map(Ok)
        .unwrap_or_else(|| drm::detect(Path::new("/sys/class/drm")));

    BlindDesktop {
        size,
        reason: match session {
            Session::Wayland => {
                "Wayland does not let applications read the pointer position; \
                 use the keyboard shortcuts (Ctrl+Alt+1..4) to switch"
            }
            _ => "there is no graphical session",
        },
    }
}

/// Build the best backends this machine supports.
pub fn backends(config: &EngineConfig) -> Backends {
    backends_for(config, Session::detect())
}

pub fn backends_for(config: &EngineConfig, session: Session) -> Backends {
    let mut notes = Vec::new();

    let source: Box<dyn InputSource> = match EvdevSource::new() {
        Ok(source) => Box::new(source),
        Err(error) => {
            notes.push(error.to_string());

            Box::new(UnavailableSource(error.reason()))
        }
    };

    let desktop: Box<dyn Desktop> = match session {
        Session::X11 => match X11Desktop::connect() {
            Ok(desktop) => Box::new(desktop),
            Err(error) => {
                notes.push(format!("{error}; switching at screen edges is unavailable"));

                Box::new(blind_desktop(config, session))
            }
        },
        other => Box::new(blind_desktop(config, other)),
    };

    let screen = desktop
        .screen_size()
        .ok()
        .or(config.screen_override)
        .unwrap_or(Size {
            width: 1920,
            height: 1080,
        });

    let sink: Box<dyn InputSink> = match UinputSink::new(screen) {
        Ok(sink) => Box::new(sink),
        Err(error) => {
            notes.push(format!(
                "this machine cannot be controlled: {}",
                error.reason()
            ));

            Box::new(UnavailableSink(error.reason()))
        }
    };

    let clipboard: Option<Box<dyn Clipboard>> = match (config.share_clipboard, session) {
        (false, _) | (_, Session::Headless) => None,
        (true, session) => match CliClipboard::detect(session == Session::Wayland) {
            Ok(clipboard) => Some(Box::new(clipboard)),
            Err(error) => {
                notes.push(error.to_string());

                None
            }
        },
    };

    Backends {
        source,
        sink,
        desktop,
        clipboard,
        locker: Box::new(LogindLocker),
        notes,
    }
}
