//! Screen geometry, pointer position and screen locking on Windows.

use async_trait::async_trait;
use aujar_core::{Position, Size};
use windows_sys::Win32::{
    Foundation::POINT,
    System::Shutdown::LockWorkStation,
    UI::WindowsAndMessaging::{
        GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    },
};

use crate::{
    backend::{Desktop, ScreenLocker},
    error::{Error, Result},
};

/// The virtual desktop: the rectangle spanning every monitor. Its origin is negative when a
/// monitor sits left of or above the primary one.
#[derive(Debug, Clone, Copy)]
pub struct VirtualScreen {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn virtual_screen() -> VirtualScreen {
    // SAFETY: plain Win32 calls.
    unsafe {
        VirtualScreen {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32,
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32,
        }
    }
}

pub struct WindowsDesktop;

impl Desktop for WindowsDesktop {
    fn screen_size(&self) -> Result<Size> {
        let screen = virtual_screen();

        Ok(Size {
            width: screen.width,
            height: screen.height,
        })
    }

    fn pointer(&self) -> Result<Position> {
        let mut point = POINT { x: 0, y: 0 };

        // SAFETY: `point` is a valid out-pointer.
        if unsafe { GetCursorPos(&mut point) } == 0 {
            return Err(Error::Other("cannot read the pointer position".into()));
        }

        // The rest of Mouse Borders measures from the virtual desktop's top-left corner.
        let screen = virtual_screen();

        Ok(Position {
            x: point.x - screen.x,
            y: point.y - screen.y,
        })
    }
}

pub struct WindowsLocker;

#[async_trait]
impl ScreenLocker for WindowsLocker {
    async fn lock(&self) -> Result<()> {
        // SAFETY: plain Win32 call.
        if unsafe { LockWorkStation() } == 0 {
            return Err(Error::Other("LockWorkStation failed".into()));
        }

        Ok(())
    }
}
