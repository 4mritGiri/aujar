//! Injects input with `SendInput`.
//!
//! Injected events carry the "injected" flag, which our own hooks recognise and ignore, so
//! injection never feeds back into capture.

use std::mem::size_of;

use aujar_core::Position;
use windows_sys::Win32::{
    Foundation::GetLastError,
    UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, MOUSEINPUT, SendInput, VK_PAUSE,
    },
};

use super::desktop::virtual_screen;
use crate::{
    backend::{InputSink, winput},
    error::{Error, Result},
    input::Button,
    keys::KEY_F15,
};

// The pure module repeats these Win32 values; fail the build if any is wrong.
macro_rules! same {
    ($($name:ident),+ $(,)?) => {
        $(const _: () = assert!(winput::$name as u64 == windows_sys::Win32::UI::Input::KeyboardAndMouse::$name as u64);)+
    };
}

same!(
    MOUSEEVENTF_MOVE,
    MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP,
    MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_ABSOLUTE,
    KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE,
);

pub struct SendInputSink;

fn mouse(flags: u32, dx: i32, dy: i32, data: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn keyboard(virtual_key: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(inputs: &[INPUT]) -> Result<()> {
    // SAFETY: `inputs` is a valid slice of INPUT structures and the size matches.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    };

    if sent as usize == inputs.len() {
        return Ok(());
    }

    // SAFETY: plain Win32 call.
    let error = unsafe { GetLastError() };

    Err(Error::unsupported(
        "input injection",
        format!(
            "SendInput was refused (error {error}); the window in front may belong to a \
             higher-privilege program, or a secure screen is showing"
        ),
    ))
}

impl InputSink for SendInputSink {
    fn pointer_abs(&mut self, position: Position) -> Result<()> {
        let screen = virtual_screen();
        let x = winput::normalize_absolute(position.x, screen.width);
        let y = winput::normalize_absolute(position.y, screen.height);

        send(&[mouse(
            winput::MOUSEEVENTF_MOVE
                | winput::MOUSEEVENTF_ABSOLUTE
                | winput::MOUSEEVENTF_VIRTUALDESK,
            x,
            y,
            0,
        )])
    }

    fn pointer_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        send(&[mouse(winput::MOUSEEVENTF_MOVE, dx, dy, 0)])
    }

    fn button(&mut self, button: Button, pressed: bool) -> Result<()> {
        let (flags, data) = winput::button_input(button, pressed);

        send(&[mouse(flags, 0, 0, data)])
    }

    fn wheel(&mut self, horizontal: i32, vertical: i32) -> Result<()> {
        let mut inputs = Vec::with_capacity(2);

        if vertical != 0 {
            inputs.push(mouse(winput::MOUSEEVENTF_WHEEL, 0, 0, vertical as u32));
        }

        if horizontal != 0 {
            inputs.push(mouse(winput::MOUSEEVENTF_HWHEEL, 0, 0, horizontal as u32));
        }

        if inputs.is_empty() {
            return Ok(());
        }

        send(&inputs)
    }

    fn key(&mut self, key: u16, pressed: bool) -> Result<()> {
        // Pause has no scan code of its own: send it as a virtual key.
        if key == 0x48 {
            let flags = if pressed { 0 } else { winput::KEYEVENTF_KEYUP };

            return send(&[keyboard(VK_PAUSE, 0, flags)]);
        }

        let (scan, flags) = winput::key_input(key, pressed).ok_or_else(|| {
            Error::unsupported(
                "input injection",
                format!(
                    "the key {} has no Windows equivalent",
                    crate::keymap::name(key)
                ),
            )
        })?;

        send(&[keyboard(0, scan, flags)])
    }

    fn nudge(&mut self) -> Result<()> {
        // F15 counts as activity to every idle timer and is bound to nothing by default.
        self.key(KEY_F15, true)?;
        self.key(KEY_F15, false)
    }
}
