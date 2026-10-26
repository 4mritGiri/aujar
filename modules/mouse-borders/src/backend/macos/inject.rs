//! Injects input by posting Quartz events.
//!
//! Every event we post is stamped with a marker the event tap recognises, so injection never
//! feeds back into capture. The decisions (event types, click counting, modifier flags,
//! scroll accumulation) live in [`super::super::macinput`].

use std::{collections::BTreeSet, time::Instant};

use aujar_core::Position;
use core_graphics::{
    display::CGDisplay,
    event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, CGMouseButton, EventField,
        ScrollEventUnit,
    },
    event_source::{CGEventSource, CGEventSourceStateID},
    geometry::CGPoint,
};

use super::desktop::virtual_bounds;
use crate::{
    backend::{InputSink, macinput as logic},
    error::{Error, Result},
    input::Button,
    keymap,
};

pub struct QuartzSink {
    pressed: BTreeSet<Button>,
    modifiers: logic::HeldModifiers,
    scroll: logic::ScrollAccumulator,
    clicks: logic::ClickTracker,
    last_click_count: i64,
    started: Instant,
}

impl QuartzSink {
    pub fn new() -> Self {
        Self {
            pressed: BTreeSet::new(),
            modifiers: logic::HeldModifiers::default(),
            scroll: logic::ScrollAccumulator::default(),
            clicks: logic::ClickTracker::default(),
            last_click_count: 1,
            started: Instant::now(),
        }
    }
}

impl Default for QuartzSink {
    fn default() -> Self {
        Self::new()
    }
}

fn source() -> Result<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| Error::Other("cannot create an event source".into()))
}

fn event_type(raw: u32) -> CGEventType {
    match raw {
        logic::LEFT_MOUSE_DOWN => CGEventType::LeftMouseDown,
        logic::LEFT_MOUSE_UP => CGEventType::LeftMouseUp,
        logic::RIGHT_MOUSE_DOWN => CGEventType::RightMouseDown,
        logic::RIGHT_MOUSE_UP => CGEventType::RightMouseUp,
        logic::LEFT_MOUSE_DRAGGED => CGEventType::LeftMouseDragged,
        logic::RIGHT_MOUSE_DRAGGED => CGEventType::RightMouseDragged,
        logic::OTHER_MOUSE_DOWN => CGEventType::OtherMouseDown,
        logic::OTHER_MOUSE_UP => CGEventType::OtherMouseUp,
        logic::OTHER_MOUSE_DRAGGED => CGEventType::OtherMouseDragged,
        _ => CGEventType::MouseMoved,
    }
}

fn cg_button(button: Button) -> CGMouseButton {
    match button {
        Button::Left => CGMouseButton::Left,
        Button::Right => CGMouseButton::Right,
        _ => CGMouseButton::Center,
    }
}

fn current_position() -> Result<CGPoint> {
    let location = CGEvent::new(source()?)
        .map_err(|()| Error::Other("cannot read the pointer position".into()))?
        .location();

    Ok(location)
}

impl QuartzSink {
    /// Stamp our marker and the held modifiers on an event and post it.
    fn post(&self, event: &CGEvent) {
        event.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, logic::OUR_EVENT_MARKER);
        event.set_flags(CGEventFlags::from_bits_truncate(self.modifiers.flags()));
        event.post(CGEventTapLocation::HID);
    }

    fn motion(&self, point: CGPoint, delta: (i32, i32)) -> Result<()> {
        let raw = logic::motion_event_type(&self.pressed);
        let button = self
            .pressed
            .iter()
            .next()
            .copied()
            .map_or(CGMouseButton::Left, cg_button);
        let event = CGEvent::new_mouse_event(source()?, event_type(raw), point, button)
            .map_err(|()| Error::Other("cannot create a mouse event".into()))?;

        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_X, i64::from(delta.0));
        event.set_integer_value_field(EventField::MOUSE_EVENT_DELTA_Y, i64::from(delta.1));
        self.post(&event);

        Ok(())
    }
}

impl InputSink for QuartzSink {
    fn pointer_abs(&mut self, position: Position) -> Result<()> {
        let bounds = virtual_bounds();
        let point = CGPoint::new(
            bounds.origin.x + f64::from(position.x),
            bounds.origin.y + f64::from(position.y),
        );

        CGDisplay::warp_mouse_cursor_position(point)
            .map_err(|code| Error::Other(format!("cannot move the pointer (error {code})")))?;
        // Warping freezes the pointer for a moment unless it is re-associated.
        let _ = CGDisplay::associate_mouse_and_mouse_cursor_position(true);

        // Applications only see hover and drag events that are actually posted.
        self.motion(point, (0, 0))
    }

    fn pointer_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        let bounds = virtual_bounds();
        let here = current_position()?;
        let point = CGPoint::new(
            (here.x + f64::from(dx))
                .clamp(bounds.origin.x, bounds.origin.x + bounds.size.width - 1.0),
            (here.y + f64::from(dy))
                .clamp(bounds.origin.y, bounds.origin.y + bounds.size.height - 1.0),
        );

        self.motion(point, (dx, dy))
    }

    fn button(&mut self, button: Button, pressed: bool) -> Result<()> {
        let point = current_position()?;
        let raw = logic::button_event_type(button, pressed);
        let event = CGEvent::new_mouse_event(source()?, event_type(raw), point, cg_button(button))
            .map_err(|()| Error::Other("cannot create a mouse event".into()))?;

        if pressed {
            self.last_click_count =
                self.clicks
                    .press(button, (point.x, point.y), self.started.elapsed());
            self.pressed.insert(button);
        } else {
            self.pressed.remove(&button);
        }

        event.set_integer_value_field(EventField::MOUSE_EVENT_CLICK_STATE, self.last_click_count);
        event.set_integer_value_field(
            EventField::MOUSE_EVENT_BUTTON_NUMBER,
            logic::button_number(button),
        );
        self.post(&event);

        Ok(())
    }

    fn wheel(&mut self, horizontal: i32, vertical: i32) -> Result<()> {
        let (lines_h, lines_v) = self.scroll.add(horizontal, vertical);

        if (lines_h, lines_v) == (0, 0) {
            return Ok(());
        }

        let event =
            CGEvent::new_scroll_event(source()?, ScrollEventUnit::LINE, 2, lines_v, lines_h, 0)
                .map_err(|()| Error::Other("cannot create a scroll event".into()))?;

        self.post(&event);

        Ok(())
    }

    fn key(&mut self, key: u16, pressed: bool) -> Result<()> {
        let code = keymap::to_mac(key).ok_or_else(|| {
            Error::unsupported(
                "input injection",
                format!("the key {} has no macOS equivalent", keymap::name(key)),
            )
        })?;

        // Flags first: a modifier's own event must already reflect its new state.
        self.modifiers.update(key, pressed);

        let event = CGEvent::new_keyboard_event(source()?, code, pressed)
            .map_err(|()| Error::Other("cannot create a key event".into()))?;

        self.post(&event);

        Ok(())
    }

    fn nudge(&mut self) -> Result<()> {
        // A key press would be seen by applications; `caffeinate -u` declares user activity to
        // the power manager instead, which resets the display-sleep and screen-saver timers.
        std::process::Command::new("caffeinate")
            .args(["-u", "-t", "2"])
            .spawn()
            .map(|_| ())
            .map_err(|error| Error::Other(format!("cannot run caffeinate: {error}")))
    }
}
