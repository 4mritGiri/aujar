#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session { X11, Wayland, Unknown }

pub fn session() -> Session {
    match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("x11") => Session::X11,
        Ok("wayland") => Session::Wayland,
        _ => Session::Unknown,
    }
}

pub trait WindowManager { fn available(&self) -> bool; }
pub struct UnsupportedWindowManager;
impl WindowManager for UnsupportedWindowManager { fn available(&self)->bool { false } }
