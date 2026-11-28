//! macOS: a Quartz event tap on a dedicated thread with its own run loop. Key presses that match
//! a shortcut are reported and swallowed, so the focused app does not also receive them.
//!
//! Swallowing keys needs the *Accessibility* permission (System Settings > Privacy & Security >
//! Accessibility). Without it macOS refuses the tap; the reason is reported and the system's
//! prompt is shown once.

use crate::{HotkeyEvent, ShortcutSpec, Trigger};
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, CallbackResult, EventField,
};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// macOS turns a tap off when it is too slow or the user is entering secure input.
const TAP_DISABLED_BY_TIMEOUT: u32 = 0xFFFF_FFFE;
const TAP_DISABLED_BY_USER_INPUT: u32 = 0xFFFF_FFFF;

/// Whether the modifiers held match the trigger exactly (Caps Lock and Fn are ignored).
fn matches(trigger: &Trigger, flags: CGEventFlags, keycode: u16) -> bool {
    keycode == trigger.mac_keycode()
        && flags.contains(CGEventFlags::CGEventFlagControl) == trigger.ctrl
        && flags.contains(CGEventFlags::CGEventFlagAlternate) == trigger.alt
        && flags.contains(CGEventFlags::CGEventFlagShift) == trigger.shift
        && flags.contains(CGEventFlags::CGEventFlagCommand) == trigger.super_key
}

pub(crate) fn start(shortcuts: Vec<(ShortcutSpec, Trigger)>, tx: UnboundedSender<HotkeyEvent>) {
    // SAFETY: a documented, argument-free query.
    if !unsafe { AXIsProcessTrusted() } {
        let _ = tx.send(HotkeyEvent::Unavailable(
            "allow Aujar under System Settings > Privacy & Security > Accessibility, then \
             restart it"
                .to_owned(),
        ));
        return;
    }

    // For reporting a thread that could not start; the thread owns `tx`.
    let fallback = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("aujar-hotkeys".into())
        .spawn(move || {
            let callback_tx = tx.clone();
            let triggers: Vec<(String, Trigger)> = shortcuts
                .iter()
                .map(|(spec, trigger)| (spec.id.clone(), *trigger))
                .collect();
            let reenable = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flag = reenable.clone();

            let tap = CGEventTap::new(
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::Default,
                vec![CGEventType::KeyDown],
                move |_proxy, event_type, event| {
                    let kind = event_type as u32;

                    if kind == TAP_DISABLED_BY_TIMEOUT || kind == TAP_DISABLED_BY_USER_INPUT {
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        return CallbackResult::Keep;
                    }

                    // Held keys repeat; a shortcut fires once per press.
                    if event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0 {
                        return CallbackResult::Keep;
                    }

                    let keycode =
                        event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                    let flags = event.get_flags();

                    match triggers
                        .iter()
                        .find(|(_, trigger)| matches(trigger, flags, keycode))
                    {
                        Some((id, _)) => {
                            let _ = callback_tx.send(HotkeyEvent::Activated(id.clone()));
                            CallbackResult::Drop
                        }
                        None => CallbackResult::Keep,
                    }
                },
            );

            let Ok(tap) = tap else {
                let _ = tx.send(HotkeyEvent::Unavailable(
                    "macOS refused the keyboard tap; allow Aujar under Privacy & Security > \
                     Accessibility"
                        .to_owned(),
                ));
                return;
            };

            let Ok(source) = tap.mach_port().create_runloop_source(0) else {
                let _ = tx.send(HotkeyEvent::Unavailable(
                    "cannot attach the keyboard tap to a run loop".to_owned(),
                ));
                return;
            };

            // SAFETY: `kCFRunLoopCommonModes` is a constant string provided by CoreFoundation.
            CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
            tap.enable();

            for (spec, trigger) in &shortcuts {
                let _ = tx.send(HotkeyEvent::Bound {
                    id: spec.id.clone(),
                    trigger: trigger.to_string(),
                });
            }

            // Until the daemon stops listening.
            while !tx.is_closed() {
                // SAFETY: `kCFRunLoopDefaultMode` is a constant string provided by CoreFoundation.
                CFRunLoop::run_in_mode(
                    unsafe { kCFRunLoopDefaultMode },
                    Duration::from_millis(500),
                    false,
                );

                if reenable.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    tap.enable();
                }
            }
        });

    if let Err(error) = spawned {
        let _ = fallback.send(HotkeyEvent::Unavailable(error.to_string()));
    }
}
