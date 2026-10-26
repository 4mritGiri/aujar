//! Injects input through `/dev/uinput`.
//!
//! Two virtual devices: a keyboard-and-relative-mouse for normal input, and a separate
//! absolute pointer used only to place the pointer when control arrives (mixing absolute and
//! relative axes in one device confuses libinput). The absolute range is fixed at 0..=65535
//! and scaled to the desktop size, as tablets do.
//!
//! Needs write access to `/dev/uinput` (see `packaging/udev`).

use aujar_core::{Position, Size};
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, EventType, InputEvent, KeyCode, RelativeAxisCode,
    UinputAbsSetup, uinput::VirtualDevice,
};

use crate::{
    backend::InputSink,
    error::{Error, Result},
    input::Button,
    keymap,
    keys::KEY_F15,
};

/// Name prefix of our virtual devices; the capture side skips anything starting with it.
pub const VIRTUAL_PREFIX: &str = "Aujar MWB";

const ABS_RANGE: i32 = 65_535;

pub struct UinputSink {
    main: VirtualDevice,
    absolute: VirtualDevice,
    screen: Size,
    wheel_remainder: (i32, i32),
}

fn io_error(what: &str, error: std::io::Error) -> Error {
    let hint = match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            " (no permission for /dev/uinput; install packaging/udev/70-aujar-uinput.rules \
             and add your user to the `input` group)"
        }
        std::io::ErrorKind::NotFound => " (is the `uinput` kernel module loaded?)",
        _ => "",
    };

    Error::unsupported("input injection", format!("{what}: {error}{hint}"))
}

impl UinputSink {
    pub fn new(screen: Size) -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();

        // Only the keys and buttons we can forward need to exist on the virtual device.
        for code in keymap::KEYS.iter().filter_map(|key| key.evdev) {
            keys.insert(KeyCode::new(code));
        }

        for button in Button::ALL {
            keys.insert(KeyCode::new(super::button_to_evdev(button)));
        }

        let mut relative = AttributeSet::<RelativeAxisCode>::new();

        for axis in [
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
        ] {
            relative.insert(axis);
        }

        let main = VirtualDevice::builder()
            .and_then(|builder| builder.name("Aujar MWB virtual input").with_keys(&keys))
            .and_then(|builder| builder.with_relative_axes(&relative))
            .and_then(|builder| builder.build())
            .map_err(|error| io_error("cannot create the virtual keyboard and mouse", error))?;

        let mut buttons = AttributeSet::<KeyCode>::new();

        for button in [KeyCode::BTN_LEFT, KeyCode::BTN_RIGHT, KeyCode::BTN_MIDDLE] {
            buttons.insert(button);
        }

        let axis = |code| UinputAbsSetup::new(code, AbsInfo::new(0, 0, ABS_RANGE, 0, 0, 0));

        let absolute = VirtualDevice::builder()
            .and_then(|builder| {
                builder
                    .name("Aujar MWB virtual pointer")
                    .with_keys(&buttons)
            })
            .and_then(|builder| builder.with_absolute_axis(&axis(AbsoluteAxisCode::ABS_X)))
            .and_then(|builder| builder.with_absolute_axis(&axis(AbsoluteAxisCode::ABS_Y)))
            .and_then(|builder| builder.build())
            .map_err(|error| io_error("cannot create the virtual pointer", error))?;

        Ok(Self {
            main,
            absolute,
            screen,
            wheel_remainder: (0, 0),
        })
    }

    fn emit_main(&mut self, events: &[InputEvent]) -> Result<()> {
        self.main
            .emit(events)
            .map_err(|error| io_error("cannot inject input", error))
    }

    fn scale(value: i32, extent: u32) -> i32 {
        let max = i64::from(extent.saturating_sub(1).max(1));

        (i64::from(value).clamp(0, max) * i64::from(ABS_RANGE) / max) as i32
    }
}

fn event(kind: EventType, code: u16, value: i32) -> InputEvent {
    InputEvent::new(kind.0, code, value)
}

impl InputSink for UinputSink {
    fn pointer_abs(&mut self, position: Position) -> Result<()> {
        let events = [
            event(
                EventType::ABSOLUTE,
                AbsoluteAxisCode::ABS_X.0,
                Self::scale(position.x, self.screen.width),
            ),
            event(
                EventType::ABSOLUTE,
                AbsoluteAxisCode::ABS_Y.0,
                Self::scale(position.y, self.screen.height),
            ),
        ];

        self.absolute
            .emit(&events)
            .map_err(|error| io_error("cannot move the pointer", error))
    }

    fn pointer_rel(&mut self, dx: i32, dy: i32) -> Result<()> {
        let mut events = Vec::with_capacity(2);

        if dx != 0 {
            events.push(event(EventType::RELATIVE, RelativeAxisCode::REL_X.0, dx));
        }

        if dy != 0 {
            events.push(event(EventType::RELATIVE, RelativeAxisCode::REL_Y.0, dy));
        }

        if events.is_empty() {
            return Ok(());
        }

        self.emit_main(&events)
    }

    fn button(&mut self, button: Button, pressed: bool) -> Result<()> {
        self.emit_main(&[event(
            EventType::KEY,
            super::button_to_evdev(button),
            i32::from(pressed),
        )])
    }

    fn wheel(&mut self, horizontal: i32, vertical: i32) -> Result<()> {
        let mut events = Vec::with_capacity(4);

        // Clients that ignore the high-resolution axes still need whole notches.
        let mut axis = |hi_res: RelativeAxisCode,
                        notch: RelativeAxisCode,
                        amount: i32,
                        remainder: &mut i32| {
            if amount == 0 {
                return;
            }

            events.push(event(EventType::RELATIVE, hi_res.0, amount));

            *remainder += amount;
            let notches = *remainder / 120;
            *remainder %= 120;

            if notches != 0 {
                events.push(event(EventType::RELATIVE, notch.0, notches));
            }
        };

        axis(
            RelativeAxisCode::REL_HWHEEL_HI_RES,
            RelativeAxisCode::REL_HWHEEL,
            horizontal,
            &mut self.wheel_remainder.0,
        );
        axis(
            RelativeAxisCode::REL_WHEEL_HI_RES,
            RelativeAxisCode::REL_WHEEL,
            vertical,
            &mut self.wheel_remainder.1,
        );

        if events.is_empty() {
            return Ok(());
        }

        self.emit_main(&events)
    }

    fn key(&mut self, key: u16, pressed: bool) -> Result<()> {
        let code = keymap::to_evdev(key).ok_or_else(|| {
            Error::unsupported(
                "input injection",
                format!("the key {} has no Linux equivalent", keymap::name(key)),
            )
        })?;

        self.emit_main(&[event(EventType::KEY, code, i32::from(pressed))])
    }

    fn nudge(&mut self) -> Result<()> {
        // F15 is bound to nothing by default, but counts as activity to every idle timer.
        self.key(KEY_F15, true)?;
        self.key(KEY_F15, false)
    }

    fn set_screen(&mut self, size: Size) {
        self.screen = size;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_positions_scale_to_the_tablet_range() {
        assert_eq!(UinputSink::scale(0, 1920), 0);
        assert_eq!(UinputSink::scale(1919, 1920), ABS_RANGE);
        assert_eq!(UinputSink::scale(960, 1920), 960 * ABS_RANGE / 1919);
        // Out-of-range positions are clamped, degenerate screens do not divide by zero.
        assert_eq!(UinputSink::scale(-50, 1920), 0);
        assert_eq!(UinputSink::scale(99_999, 1920), ABS_RANGE);
        assert_eq!(UinputSink::scale(5, 0), ABS_RANGE);
    }
}
