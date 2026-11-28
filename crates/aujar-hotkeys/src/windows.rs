//! Windows: `RegisterHotKey` on a dedicated thread, whose message loop receives `WM_HOTKEY`.
//! No window and no permission are needed. Windows refuses a combination another program (or
//! Windows itself, such as Win+Shift+S) already registered; that shortcut is reported as failed.

use crate::{HotkeyEvent, ShortcutSpec, Trigger};
use tokio::sync::mpsc::UnboundedSender;
use windows_sys::Win32::UI::{
    Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey, UnregisterHotKey,
    },
    WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY},
};

fn modifiers(trigger: &Trigger) -> u32 {
    // Holding the keys must not fire again and again.
    let mut flags = MOD_NOREPEAT;

    if trigger.ctrl {
        flags |= MOD_CONTROL;
    }
    if trigger.alt {
        flags |= MOD_ALT;
    }
    if trigger.shift {
        flags |= MOD_SHIFT;
    }
    if trigger.super_key {
        flags |= MOD_WIN;
    }

    flags
}

pub(crate) fn start(shortcuts: Vec<(ShortcutSpec, Trigger)>, tx: UnboundedSender<HotkeyEvent>) {
    // For reporting a thread that could not start; the thread owns `tx`.
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("aujar-hotkeys".into())
        .spawn(move || {
            // Hotkey ids are 1-based indexes into `registered`.
            let mut registered: Vec<String> = Vec::new();

            for (spec, trigger) in &shortcuts {
                let id = registered.len() as i32 + 1;

                // SAFETY: a thread-level hotkey (no window); its id is unique on this thread.
                let ok = unsafe {
                    RegisterHotKey(
                        std::ptr::null_mut(),
                        id,
                        modifiers(trigger),
                        trigger.windows_vk(),
                    )
                } != 0;

                if ok {
                    registered.push(spec.id.clone());
                    let _ = tx.send(HotkeyEvent::Bound {
                        id: spec.id.clone(),
                        trigger: trigger.to_string(),
                    });
                } else {
                    let _ = tx.send(HotkeyEvent::Failed {
                        id: spec.id.clone(),
                        reason: format!(
                            "{trigger} is already used by another program (or by Windows); \
                             choose another key in the settings"
                        ),
                    });
                }
            }

            if registered.is_empty() {
                let _ = tx.send(HotkeyEvent::Unavailable(
                    "no global shortcut could be registered".to_owned(),
                ));
                return;
            }

            // SAFETY: an all-zero MSG is valid; GetMessageW fills it.
            let mut message: MSG = unsafe { std::mem::zeroed() };

            // GetMessageW returns 0 for WM_QUIT and -1 on error.
            while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
                if message.message != WM_HOTKEY {
                    continue;
                }

                let index = message.wParam.wrapping_sub(1);

                if let Some(id) = registered.get(index)
                    && tx.send(HotkeyEvent::Activated(id.clone())).is_err()
                {
                    break;
                }
            }

            for id in 1..=registered.len() as i32 {
                // SAFETY: unregisters this thread's own hotkeys.
                unsafe { UnregisterHotKey(std::ptr::null_mut(), id) };
            }
        });

    if let Err(error) = spawned {
        let _ = fallback.send(HotkeyEvent::Unavailable(error.to_string()));
    }
}
