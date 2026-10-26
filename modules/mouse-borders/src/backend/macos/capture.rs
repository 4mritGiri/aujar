//! Captures keyboard and mouse input with a Quartz event tap.
//!
//! The tap runs on a dedicated thread with its own run loop. While input is *captured* the
//! tap callback drops events, so the rest of the desktop never sees them; mouse motion is
//! still reported because the raw deltas travel with the events. The decisions live in
//! [`super::super::macinput`]; this file is the Quartz plumbing.
//!
//! Needs *Input Monitoring* and *Accessibility* permission. macOS also stops delivering
//! keystrokes to taps while a password field has "secure input" focus: those never cross.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread::JoinHandle,
    time::Duration,
};

use async_trait::async_trait;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes, kCFRunLoopDefaultMode};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use tokio::sync::mpsc;

use super::super::macinput as logic;
use crate::{
    backend::{CapturedInput, InputSource},
    error::{Error, Result},
    keymap,
};

// The pure module repeats these Quartz values; fail the build if any is wrong.
macro_rules! same {
    ($($name:ident = $variant:ident),+ $(,)?) => {
        $(const _: () = assert!(logic::$name == CGEventType::$variant as u32);)+
    };
}

same!(
    LEFT_MOUSE_DOWN = LeftMouseDown,
    LEFT_MOUSE_UP = LeftMouseUp,
    RIGHT_MOUSE_DOWN = RightMouseDown,
    RIGHT_MOUSE_UP = RightMouseUp,
    MOUSE_MOVED = MouseMoved,
    LEFT_MOUSE_DRAGGED = LeftMouseDragged,
    RIGHT_MOUSE_DRAGGED = RightMouseDragged,
    KEY_DOWN = KeyDown,
    KEY_UP = KeyUp,
    FLAGS_CHANGED = FlagsChanged,
    SCROLL_WHEEL = ScrollWheel,
    OTHER_MOUSE_DOWN = OtherMouseDown,
    OTHER_MOUSE_UP = OtherMouseUp,
    OTHER_MOUSE_DRAGGED = OtherMouseDragged,
);

/// Whether the tap drops events instead of passing them on.
static CAPTURING: AtomicBool = AtomicBool::new(false);
/// macOS disabled the tap (it was too slow, or the user did something); turn it back on.
static REENABLE: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);

const TAP_DISABLED_BY_TIMEOUT: u32 = CGEventType::TapDisabledByTimeout as u32;
const TAP_DISABLED_BY_USER_INPUT: u32 = CGEventType::TapDisabledByUserInput as u32;

fn forward(sender: &mpsc::UnboundedSender<CapturedInput>, input: CapturedInput) {
    let _ = sender.send(input);
}

fn handle(
    sender: &mpsc::UnboundedSender<CapturedInput>,
    kind: u32,
    event: &core_graphics::event::CGEvent,
) {
    match kind {
        k if logic::is_motion(k) => {
            let dx = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_X) as i32;
            let dy = event.get_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y) as i32;

            if (dx, dy) != (0, 0) {
                forward(sender, CapturedInput::PointerMotion { dx, dy });
            }
        }

        logic::LEFT_MOUSE_DOWN
        | logic::LEFT_MOUSE_UP
        | logic::RIGHT_MOUSE_DOWN
        | logic::RIGHT_MOUSE_UP
        | logic::OTHER_MOUSE_DOWN
        | logic::OTHER_MOUSE_UP => {
            let number = event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER);

            if let Some((button, pressed)) = logic::translate_button(kind, number) {
                forward(sender, CapturedInput::Button { button, pressed });
            }
        }

        logic::SCROLL_WHEEL => {
            let vertical =
                event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1);
            let horizontal =
                event.get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_2);

            if (vertical, horizontal) != (0, 0) {
                forward(
                    sender,
                    CapturedInput::Wheel {
                        horizontal: horizontal as i32 * 120,
                        vertical: vertical as i32 * 120,
                    },
                );
            }
        }

        logic::KEY_DOWN | logic::KEY_UP => {
            // Auto-repeat: the machine we control repeats on its own.
            if event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0 {
                return;
            }

            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;

            if let Some(key) = logic::translate_key(keycode) {
                forward(
                    sender,
                    CapturedInput::Key {
                        key,
                        pressed: kind == logic::KEY_DOWN,
                    },
                );
            }
        }

        logic::FLAGS_CHANGED => {
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;

            if keycode == logic::CAPS_LOCK_KEYCODE {
                // Caps Lock reports its state, not press and release: send a tap.
                if let Some(key) = keymap::from_mac(keycode) {
                    forward(sender, CapturedInput::Key { key, pressed: true });
                    forward(
                        sender,
                        CapturedInput::Key {
                            key,
                            pressed: false,
                        },
                    );
                }
            } else if let Some((key, pressed)) =
                logic::modifier_change(keycode, event.get_flags().bits())
            {
                forward(sender, CapturedInput::Key { key, pressed });
            }
        }

        _ => {}
    }
}

fn tap_thread(
    sender: mpsc::UnboundedSender<CapturedInput>,
    ready: std::sync::mpsc::Sender<std::result::Result<(), String>>,
) {
    let events = vec![
        CGEventType::LeftMouseDown,
        CGEventType::LeftMouseUp,
        CGEventType::RightMouseDown,
        CGEventType::RightMouseUp,
        CGEventType::MouseMoved,
        CGEventType::LeftMouseDragged,
        CGEventType::RightMouseDragged,
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
        CGEventType::ScrollWheel,
        CGEventType::OtherMouseDown,
        CGEventType::OtherMouseUp,
        CGEventType::OtherMouseDragged,
    ];

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        events,
        move |_proxy, event_type, event| {
            let kind = event_type as u32;

            if kind == TAP_DISABLED_BY_TIMEOUT || kind == TAP_DISABLED_BY_USER_INPUT {
                REENABLE.store(true, Ordering::SeqCst);

                return CallbackResult::Keep;
            }

            // Our own injected events: neither forwarded nor dropped.
            if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA)
                == logic::OUR_EVENT_MARKER
            {
                return CallbackResult::Keep;
            }

            handle(&sender, kind, event);

            if CAPTURING.load(Ordering::Relaxed) {
                CallbackResult::Drop
            } else {
                CallbackResult::Keep
            }
        },
    );

    let Ok(tap) = tap else {
        let _ = ready.send(Err(
            "macOS refused to create the event tap. Allow this program under System Settings → \
             Privacy & Security → Input Monitoring and Accessibility, then start it again"
                .to_owned(),
        ));

        return;
    };

    let Ok(source) = tap.mach_port().create_runloop_source(0) else {
        let _ = ready.send(Err("cannot attach the event tap to a run loop".to_owned()));

        return;
    };

    // SAFETY: `kCFRunLoopCommonModes` is a constant string provided by CoreFoundation.
    CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
    tap.enable();

    let _ = ready.send(Ok(()));

    while !STOP.load(Ordering::SeqCst) {
        // SAFETY: `kCFRunLoopDefaultMode` is a constant string provided by CoreFoundation.
        CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopDefaultMode },
            Duration::from_millis(200),
            false,
        );

        if REENABLE.swap(false, Ordering::SeqCst) {
            tap.enable();
        }
    }
}

pub struct TapSource {
    events: mpsc::UnboundedReceiver<CapturedInput>,
    thread: Option<JoinHandle<()>>,
}

impl TapSource {
    pub fn new() -> Result<Self> {
        STOP.store(false, Ordering::SeqCst);
        CAPTURING.store(false, Ordering::SeqCst);

        let (sender, events) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("aujar-mwb-tap".into())
            .spawn(move || tap_thread(sender, ready_tx))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                events,
                thread: Some(thread),
            }),
            Ok(Err(reason)) => Err(Error::unsupported("input capture", reason)),
            Err(_) => Err(Error::unsupported(
                "input capture",
                "the event tap thread stopped",
            )),
        }
    }
}

impl Drop for TapSource {
    fn drop(&mut self) {
        CAPTURING.store(false, Ordering::SeqCst);
        STOP.store(true, Ordering::SeqCst);

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[async_trait]
impl InputSource for TapSource {
    async fn next(&mut self) -> Result<Option<CapturedInput>> {
        Ok(self.events.recv().await)
    }

    fn set_grab(&mut self, grabbed: bool) -> Result<()> {
        CAPTURING.store(grabbed, Ordering::SeqCst);

        Ok(())
    }
}
