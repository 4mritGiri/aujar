//! Captures keyboard and mouse input with low-level hooks.
//!
//! `WH_KEYBOARD_LL` and `WH_MOUSE_LL` run on a dedicated thread with its own message loop.
//! While input is *captured* the hooks swallow events (so the local desktop never sees them)
//! and the cursor is parked at the screen centre so mouse motion can still be measured. The
//! decisions live in [`super::super::winput`]; this file is only the Win32 plumbing.
//!
//! Limits: Windows delivers input to a hook only if the hooking process is at least as
//! privileged as the window receiving it, so input aimed at an elevated window needs an
//! elevated Aujar. Ctrl+Alt+Del and the secure desktop are never visible.

use std::{
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};

use async_trait::async_trait;
use tokio::sync::mpsc;
use windows_sys::Win32::{
    Foundation::{GetLastError, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, GetSystemMetrics, HC_ACTION, KBDLLHOOKSTRUCT, MSG,
        MSLLHOOKSTRUCT, PostThreadMessageW, SM_CXSCREEN, SM_CYSCREEN, SetCursorPos,
        SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_MOUSEMOVE, WM_QUIT,
    },
};

use crate::{
    backend::{CapturedInput, InputSource, winput},
    error::{Error, Result},
};

// The pure module repeats these Win32 values; fail the build if any is wrong.
macro_rules! same {
    ($($name:ident),+ $(,)?) => {
        $(const _: () = assert!(winput::$name as u64 == windows_sys::Win32::UI::WindowsAndMessaging::$name as u64);)+
    };
}

same!(
    WM_KEYDOWN,
    WM_KEYUP,
    WM_SYSKEYDOWN,
    WM_SYSKEYUP,
    WM_MOUSEMOVE,
    WM_LBUTTONDOWN,
    WM_LBUTTONUP,
    WM_RBUTTONDOWN,
    WM_RBUTTONUP,
    WM_MBUTTONDOWN,
    WM_MBUTTONUP,
    WM_MOUSEWHEEL,
    WM_XBUTTONDOWN,
    WM_XBUTTONUP,
    WM_MOUSEHWHEEL,
    LLKHF_EXTENDED,
    LLKHF_INJECTED,
    LLMHF_INJECTED,
    XBUTTON1,
    XBUTTON2,
);
const _: () = assert!(
    winput::VK_PAUSE as u64 == windows_sys::Win32::UI::Input::KeyboardAndMouse::VK_PAUSE as u64
);

/// Whether the hooks currently swallow input.
static CAPTURING: AtomicBool = AtomicBool::new(false);
static SENDER: Mutex<Option<mpsc::UnboundedSender<CapturedInput>>> = Mutex::new(None);
static MOTION: Mutex<Option<winput::MotionTracker>> = Mutex::new(None);

fn emit(input: CapturedInput) {
    if let Ok(sender) = SENDER.lock()
        && let Some(sender) = sender.as_ref()
    {
        let _ = sender.send(input);
    }
}

fn with_motion<T>(action: impl FnOnce(&mut winput::MotionTracker) -> T) -> Option<T> {
    let mut guard = MOTION.lock().ok()?;

    Some(action(
        guard.get_or_insert_with(winput::MotionTracker::default),
    ))
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code as u32 == HC_ACTION {
        // SAFETY: for WH_KEYBOARD_LL with HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let event = winput::translate_key(wparam as u32, info.scanCode, info.vkCode, info.flags);

        if !event.injected {
            if let Some(input) = event.input {
                emit(input);
            }

            if CAPTURING.load(Ordering::Relaxed) {
                return 1;
            }
        }
    }

    // SAFETY: forwarding the hook call unchanged, as the API requires.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code as u32 == HC_ACTION {
        // SAFETY: for WH_MOUSE_LL with HC_ACTION, `lparam` points to an MSLLHOOKSTRUCT.
        let info = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
        let message = wparam as u32;
        let position = (info.pt.x, info.pt.y);

        if info.flags & winput::LLMHF_INJECTED != 0 {
            // Our own injection (or another program's): not user input.
            if message == WM_MOUSEMOVE {
                with_motion(|motion| motion.note_injected(position));
            }
        } else {
            if message == WM_MOUSEMOVE {
                if let Some(Some((dx, dy))) = with_motion(|motion| motion.delta(position)) {
                    emit(CapturedInput::PointerMotion { dx, dy });
                }
            } else if let Some((button, pressed)) =
                winput::translate_button(message, info.mouseData)
            {
                emit(CapturedInput::Button { button, pressed });
            } else if let Some(wheel) = winput::translate_wheel(message, info.mouseData) {
                emit(wheel);
            }

            if CAPTURING.load(Ordering::Relaxed) {
                return 1;
            }
        }
    }

    // SAFETY: forwarding the hook call unchanged, as the API requires.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}

/// Runs on the hook thread: install the hooks, report success, pump messages until `WM_QUIT`.
fn hook_thread(ready: std::sync::mpsc::Sender<std::result::Result<u32, String>>) {
    // SAFETY: plain Win32 calls with valid arguments; the hook procedures above match the
    // required signature and live for the whole program.
    unsafe {
        let module = GetModuleHandleW(ptr::null());
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);

        if keyboard.is_null() || mouse.is_null() {
            let error = GetLastError();

            if !keyboard.is_null() {
                UnhookWindowsHookEx(keyboard);
            }

            if !mouse.is_null() {
                UnhookWindowsHookEx(mouse);
            }

            let _ = ready.send(Err(format!("SetWindowsHookEx failed (error {error})")));

            return;
        }

        let _ = ready.send(Ok(GetCurrentThreadId()));

        let mut message: MSG = std::mem::zeroed();

        while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {}

        UnhookWindowsHookEx(keyboard);
        UnhookWindowsHookEx(mouse);
    }
}

pub struct HookSource {
    events: mpsc::UnboundedReceiver<CapturedInput>,
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl HookSource {
    pub fn new() -> Result<Self> {
        let (sender, events) = mpsc::unbounded_channel();

        *SENDER
            .lock()
            .map_err(|_| Error::Other("hook state poisoned".into()))? = Some(sender);
        with_motion(|motion| motion.set_capture(None));
        CAPTURING.store(false, Ordering::SeqCst);

        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("aujar-mwb-hooks".into())
            .spawn(move || hook_thread(ready_tx))?;

        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(Self {
                events,
                thread_id,
                thread: Some(thread),
            }),
            Ok(Err(reason)) => Err(Error::unsupported("input capture", reason)),
            Err(_) => Err(Error::unsupported(
                "input capture",
                "the hook thread stopped",
            )),
        }
    }
}

impl Drop for HookSource {
    fn drop(&mut self) {
        CAPTURING.store(false, Ordering::SeqCst);

        // SAFETY: asks the hook thread to leave its message loop.
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }

        if let Ok(mut sender) = SENDER.lock() {
            *sender = None;
        }
    }
}

#[async_trait]
impl InputSource for HookSource {
    async fn next(&mut self) -> Result<Option<CapturedInput>> {
        Ok(self.events.recv().await)
    }

    fn set_grab(&mut self, grabbed: bool) -> Result<()> {
        if grabbed {
            // Park the cursor mid-screen: motion is then measured from there, and can go in any
            // direction however close to an edge the pointer was.
            // SAFETY: plain Win32 calls.
            let park = unsafe {
                (
                    GetSystemMetrics(SM_CXSCREEN) / 2,
                    GetSystemMetrics(SM_CYSCREEN) / 2,
                )
            };

            with_motion(|motion| motion.set_capture(Some(park)));
            CAPTURING.store(true, Ordering::SeqCst);

            // SAFETY: plain Win32 call.
            unsafe {
                SetCursorPos(park.0, park.1);
            }
        } else {
            CAPTURING.store(false, Ordering::SeqCst);
            with_motion(|motion| motion.set_capture(None));
        }

        Ok(())
    }
}
