//! Captures keyboards, mice and touchpads from `/dev/input/event*`.
//!
//! Devices are read passively until the engine asks for a grab; then `EVIOCGRAB` makes the
//! kernel deliver their events only to us, so the local desktop stops seeing them. The
//! kernel drops a grab when the file descriptor closes, so even a crash cannot leave the
//! keyboard captured.
//!
//! Needs read access to the event nodes (the `input` group).

use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    task::Poll,
    time::Duration,
};

use async_trait::async_trait;
use evdev::{
    AbsoluteAxisCode, EventType, KeyCode, PropType, RelativeAxisCode, SynchronizationCode,
    raw_stream::RawDevice,
};

use super::uinput_sink::VIRTUAL_PREFIX;
use crate::{
    backend::{CapturedInput, InputSource},
    error::{Error, Result},
    keymap,
};

const INPUT_DIR: &str = "/dev/input";
const RESCAN: Duration = Duration::from_secs(2);

/// Pointer pixels per millimetre of finger travel on a touchpad.
const TOUCHPAD_PX_PER_MM: f32 = 35.0;

/// Units per millimetre assumed when a touchpad does not report its resolution.
const DEFAULT_TOUCHPAD_RESOLUTION: f32 = 12.0;

const BTN_MOUSE_FIRST: u16 = 0x110;
const BTN_MOUSE_LAST: u16 = 0x117;
const BTN_TOUCH: u16 = 0x14a;
const BTN_MISC: u16 = 0x100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Keyboard,
    Mouse,
    Touchpad,
}

fn classify(device: &RawDevice) -> Option<Kind> {
    if device
        .name()
        .is_some_and(|name| name.starts_with(VIRTUAL_PREFIX))
    {
        return None;
    }

    let keys = device.supported_keys();
    let has_key = |key: KeyCode| keys.is_some_and(|keys| keys.contains(key));
    let rel = device.supported_relative_axes();
    let abs = device.supported_absolute_axes();

    if rel.is_some_and(|rel| {
        rel.contains(RelativeAxisCode::REL_X) && rel.contains(RelativeAxisCode::REL_Y)
    }) && has_key(KeyCode::BTN_LEFT)
    {
        return Some(Kind::Mouse);
    }

    if abs.is_some_and(|abs| {
        abs.contains(AbsoluteAxisCode::ABS_X) && abs.contains(AbsoluteAxisCode::ABS_Y)
    }) && has_key(KeyCode::BTN_TOOL_FINGER)
        && device.properties().contains(PropType::POINTER)
        && !device.properties().contains(PropType::DIRECT)
    {
        return Some(Kind::Touchpad);
    }

    if has_key(KeyCode::KEY_A) && has_key(KeyCode::KEY_ENTER) {
        return Some(Kind::Keyboard);
    }

    None
}

/// Turns a touchpad's absolute finger position into relative pointer motion.
#[derive(Debug, Default)]
struct TouchpadMotion {
    x: Option<i32>,
    y: Option<i32>,
    last: Option<(i32, i32)>,
    resolution: (f32, f32),
    remainder: (f32, f32),
}

impl TouchpadMotion {
    fn new(resolution: (f32, f32)) -> Self {
        Self {
            resolution,
            ..Self::default()
        }
    }

    /// The finger lifted: the next touch starts a new stroke instead of jumping.
    fn lift(&mut self) {
        self.last = None;
        self.x = None;
        self.y = None;
        self.remainder = (0.0, 0.0);
    }

    /// Called at the end of an event frame.
    fn frame(&mut self) -> Option<(i32, i32)> {
        let (x, y) = (self.x.take(), self.y.take());
        let (Some(x), Some(y)) = (x.or(self.last.map(|l| l.0)), y.or(self.last.map(|l| l.1)))
        else {
            return None;
        };

        let previous = self.last.replace((x, y))?;
        let scale = |delta: i32, resolution: f32, remainder: &mut f32| {
            let exact = delta as f32 / resolution.max(1.0) * TOUCHPAD_PX_PER_MM + *remainder;
            let whole = exact.trunc();

            *remainder = exact - whole;

            whole as i32
        };

        let dx = scale(x - previous.0, self.resolution.0, &mut self.remainder.0);
        let dy = scale(y - previous.1, self.resolution.1, &mut self.remainder.1);

        (dx != 0 || dy != 0).then_some((dx, dy))
    }
}

struct Tracked {
    path: PathBuf,
    stream: evdev::raw_stream::EventStream,
    kind: Kind,
    hi_res_wheel: bool,
    motion: (i32, i32),
    wheel: (i32, i32),
    touchpad: TouchpadMotion,
}

impl Tracked {
    /// Handle one raw event, queueing captured input in `out`.
    fn handle(&mut self, event: &evdev::InputEvent, out: &mut VecDeque<CapturedInput>) {
        let code = event.code();
        let value = event.value();

        match event.event_type() {
            EventType::KEY => {
                // 2 is kernel auto-repeat; the receiving desktop repeats on its own.
                if value == 2 {
                    return;
                }

                let pressed = value == 1;

                match code {
                    BTN_TOUCH if self.kind == Kind::Touchpad => {
                        if !pressed {
                            self.touchpad.lift();
                        }
                    }
                    BTN_MOUSE_FIRST..=BTN_MOUSE_LAST => {
                        if let Some(button) = super::button_from_evdev(code) {
                            out.push_back(CapturedInput::Button { button, pressed });
                        }
                    }
                    code if code < BTN_MISC => {
                        // Keys with no neutral equivalent are not forwarded.
                        if let Some(key) = keymap::from_evdev(code) {
                            out.push_back(CapturedInput::Key { key, pressed });
                        }
                    }
                    _ => {}
                }
            }

            EventType::RELATIVE => match code {
                c if c == RelativeAxisCode::REL_X.0 => self.motion.0 += value,
                c if c == RelativeAxisCode::REL_Y.0 => self.motion.1 += value,
                c if c == RelativeAxisCode::REL_WHEEL_HI_RES.0 && self.hi_res_wheel => {
                    self.wheel.1 += value;
                }
                c if c == RelativeAxisCode::REL_HWHEEL_HI_RES.0 && self.hi_res_wheel => {
                    self.wheel.0 += value;
                }
                c if c == RelativeAxisCode::REL_WHEEL.0 && !self.hi_res_wheel => {
                    self.wheel.1 += value * 120;
                }
                c if c == RelativeAxisCode::REL_HWHEEL.0 && !self.hi_res_wheel => {
                    self.wheel.0 += value * 120;
                }
                _ => {}
            },

            EventType::ABSOLUTE if self.kind == Kind::Touchpad => {
                if code == AbsoluteAxisCode::ABS_X.0 {
                    self.touchpad.x = Some(value);
                } else if code == AbsoluteAxisCode::ABS_Y.0 {
                    self.touchpad.y = Some(value);
                }
            }

            EventType::SYNCHRONIZATION if code == SynchronizationCode::SYN_REPORT.0 => {
                if self.kind == Kind::Touchpad
                    && let Some((dx, dy)) = self.touchpad.frame()
                {
                    self.motion = (self.motion.0 + dx, self.motion.1 + dy);
                }

                // One event per frame, however many axes moved.
                if self.motion != (0, 0) {
                    out.push_back(CapturedInput::PointerMotion {
                        dx: self.motion.0,
                        dy: self.motion.1,
                    });
                    self.motion = (0, 0);
                }

                if self.wheel != (0, 0) {
                    out.push_back(CapturedInput::Wheel {
                        horizontal: self.wheel.0,
                        vertical: self.wheel.1,
                    });
                    self.wheel = (0, 0);
                }
            }

            _ => {}
        }
    }
}

pub struct EvdevSource {
    devices: Vec<Tracked>,
    /// Event nodes already looked at (opened, or judged irrelevant).
    seen: HashSet<PathBuf>,
    pending: VecDeque<CapturedInput>,
    grabbed: bool,
    rescan: tokio::time::Interval,
    /// Where the next poll round starts, so one chatty device cannot starve the others.
    start: usize,
}

impl EvdevSource {
    pub fn new() -> Result<Self> {
        let mut source = Self {
            devices: Vec::new(),
            seen: HashSet::new(),
            pending: VecDeque::new(),
            grabbed: false,
            rescan: tokio::time::interval(RESCAN),
            start: 0,
        };

        let denied = source.scan(Path::new(INPUT_DIR))?;

        if source.devices.is_empty() && denied > 0 {
            return Err(Error::unsupported(
                "input capture",
                format!(
                    "no permission to read {INPUT_DIR}/event*. Add your user to the `input` \
                     group and log in again, or run Aujar's system service"
                ),
            ));
        }

        Ok(source)
    }

    /// Open any new event nodes. Returns how many were refused for lack of permission.
    fn scan(&mut self, dir: &Path) -> Result<usize> {
        let present: HashSet<PathBuf> = std::fs::read_dir(dir)
            .map_err(|error| {
                Error::unsupported(
                    "input capture",
                    format!("cannot read {}: {error}", dir.display()),
                )
            })?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("event"))
            })
            .collect();

        // Forget nodes that went away so a replugged device is picked up again.
        self.seen.retain(|path| present.contains(path));

        let mut denied = 0;

        for path in present {
            if self.seen.contains(&path) {
                continue;
            }

            match RawDevice::open(&path) {
                Ok(device) => {
                    self.seen.insert(path.clone());
                    self.attach(path, device);
                }
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => denied += 1,
                Err(_) => {
                    self.seen.insert(path);
                }
            }
        }

        Ok(denied)
    }

    fn attach(&mut self, path: PathBuf, mut device: RawDevice) {
        let Some(kind) = classify(&device) else {
            return;
        };

        let hi_res_wheel = device
            .supported_relative_axes()
            .is_some_and(|axes| axes.contains(RelativeAxisCode::REL_WHEEL_HI_RES));

        let resolution = device
            .get_abs_state()
            .map(|state| {
                let of = |axis: AbsoluteAxisCode| {
                    let value = state[usize::from(axis.0)].resolution as f32;

                    if value > 0.0 {
                        value
                    } else {
                        DEFAULT_TOUCHPAD_RESOLUTION
                    }
                };

                (of(AbsoluteAxisCode::ABS_X), of(AbsoluteAxisCode::ABS_Y))
            })
            .unwrap_or((DEFAULT_TOUCHPAD_RESOLUTION, DEFAULT_TOUCHPAD_RESOLUTION));

        // A device plugged in while input is captured must be captured too.
        if self.grabbed && device.grab().is_err() {
            tracing::warn!(path = %path.display(), "cannot grab a newly attached device");
        }

        match device.into_event_stream() {
            Ok(stream) => {
                tracing::debug!(path = %path.display(), ?kind, "capturing input device");

                self.devices.push(Tracked {
                    path,
                    stream,
                    kind,
                    hi_res_wheel,
                    motion: (0, 0),
                    wheel: (0, 0),
                    touchpad: TouchpadMotion::new(resolution),
                });
            }
            Err(error) => tracing::warn!(%error, "cannot read an input device"),
        }
    }
}

/// Wait for the next raw event from any device, starting the round at `start`.
async fn next_raw(
    devices: &mut [Tracked],
    start: usize,
) -> (usize, std::io::Result<evdev::InputEvent>) {
    std::future::poll_fn(|cx| {
        let count = devices.len();

        for offset in 0..count {
            let index = (start + offset) % count;

            if let Poll::Ready(result) = devices[index].stream.poll_event(cx) {
                return Poll::Ready((index, result));
            }
        }

        Poll::Pending
    })
    .await
}

#[async_trait]
impl InputSource for EvdevSource {
    async fn next(&mut self) -> Result<Option<CapturedInput>> {
        loop {
            if let Some(input) = self.pending.pop_front() {
                return Ok(Some(input));
            }

            tokio::select! {
                (index, result) = next_raw(&mut self.devices, self.start) => {
                    self.start = index + 1;

                    match result {
                        Ok(event) => {
                            let device = &mut self.devices[index];

                            device.handle(&event, &mut self.pending);
                        }
                        Err(error) => {
                            // Unplugged (or otherwise gone): stop reading it.
                            let gone = self.devices.swap_remove(index);

                            tracing::debug!(path = %gone.path.display(), %error, "input device removed");
                            self.seen.remove(&gone.path);
                        }
                    }
                }

                _ = self.rescan.tick() => {
                    let _ = self.scan(Path::new(INPUT_DIR));
                }
            }
        }
    }

    fn set_grab(&mut self, grabbed: bool) -> Result<()> {
        let mut failure = None;

        for device in &mut self.devices {
            let raw = device.stream.device_mut();
            let result = if grabbed { raw.grab() } else { raw.ungrab() };

            if let Err(error) = result {
                failure = Some(format!("{}: {error}", device.path.display()));

                break;
            }
        }

        if let Some(failure) = failure {
            if grabbed {
                // All or nothing: a half-grabbed keyboard is worse than none.
                for device in &mut self.devices {
                    let _ = device.stream.device_mut().ungrab();
                }
            }

            return Err(Error::Other(format!(
                "cannot grab input devices ({failure})"
            )));
        }

        self.grabbed = grabbed;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touchpad_strokes_become_relative_motion() {
        let mut pad = TouchpadMotion::new((10.0, 10.0));
        let mut stroke = |x: i32, y: i32| {
            pad.x = Some(x);
            pad.y = Some(y);

            pad.frame()
        };

        // First contact only records the start.
        assert_eq!(stroke(1000, 500), None);
        // 10 units at 10 units/mm is 1 mm: 35 px.
        assert_eq!(stroke(1010, 500), Some((35, 0)));
        assert_eq!(stroke(1010, 480), Some((0, -70)));
    }

    #[test]
    fn lifting_a_finger_does_not_make_the_pointer_jump() {
        let mut pad = TouchpadMotion::new((10.0, 10.0));

        pad.x = Some(0);
        pad.y = Some(0);
        pad.frame();
        pad.lift();

        // A touch far away starts a fresh stroke.
        pad.x = Some(5000);
        pad.y = Some(5000);
        assert_eq!(pad.frame(), None);
    }

    #[test]
    fn sub_pixel_movement_accumulates() {
        let mut pad = TouchpadMotion::new((100.0, 100.0));
        let mut total = 0;

        pad.x = Some(0);
        pad.y = Some(0);
        pad.frame();

        // 1 unit at 100 units/mm is 0.35 px; three of them must eventually move a pixel.
        for step in 1..=10 {
            pad.x = Some(step);
            pad.y = Some(0);

            if let Some((dx, _)) = pad.frame() {
                total += dx;
            }
        }

        assert_eq!(total, 3, "10 x 0.35 px = 3.5 px, 3 whole pixels so far");
    }
}
