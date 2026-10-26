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

use aujar_core::{Position, Size};

pub use clipboard_cli::CliClipboard;
pub use evdev_source::EvdevSource;
pub use logind::LogindLocker;
pub use uinput_sink::{UinputSink, VIRTUAL_PREFIX};
pub use x11::X11Desktop;

use super::{
    Backends, Clipboard, Desktop, InputSink, InputSource,
    unavailable::{UnavailableSink, UnavailableSource},
};
use crate::input::Button;
use crate::{
    config::EngineConfig,
    error::{Error, Result},
};

/// Linux `BTN_*` codes for the mouse buttons we forward.
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;
const BTN_SIDE: u16 = 0x113;
const BTN_EXTRA: u16 = 0x114;
const BTN_FORWARD: u16 = 0x115;
const BTN_BACK: u16 = 0x116;

/// The neutral button for a Linux `BTN_*` code (`None` for buttons we do not forward).
fn button_from_evdev(code: u16) -> Option<Button> {
    match code {
        BTN_LEFT => Some(Button::Left),
        BTN_RIGHT => Some(Button::Right),
        BTN_MIDDLE => Some(Button::Middle),
        // Mice report their thumb buttons as SIDE/EXTRA or as BACK/FORWARD.
        BTN_SIDE | BTN_BACK => Some(Button::Back),
        BTN_EXTRA | BTN_FORWARD => Some(Button::Forward),
        _ => None,
    }
}

fn button_to_evdev(button: Button) -> u16 {
    match button {
        Button::Left => BTN_LEFT,
        Button::Right => BTN_RIGHT,
        Button::Middle => BTN_MIDDLE,
        Button::Back => BTN_SIDE,
        Button::Forward => BTN_EXTRA,
    }
}

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

            Box::new(UnavailableSource::new(error.reason()))
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

            Box::new(UnavailableSink::new(error.reason()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_convert_both_ways() {
        for button in Button::ALL {
            assert_eq!(button_from_evdev(button_to_evdev(button)), Some(button));
        }

        assert_eq!(button_from_evdev(BTN_BACK), Some(Button::Back));
        assert_eq!(button_from_evdev(BTN_FORWARD), Some(Button::Forward));
        // Tool and touch codes are not mouse buttons.
        assert_eq!(button_from_evdev(0x14a), None);
        assert_eq!(button_from_evdev(0x117), None);
    }
}
