//! Aujar Snip: Aujar's own snipping overlay.
//!
//! The daemon takes a picture of the whole screen without asking anything (`CaptureScreen`),
//! with the places of the visible windows, and this window shows it frozen, full screen.
//!
//! A toolbar at the bottom picks how to select: **Selection** (drag an area), **Screen** (the
//! whole monitor) or **Window** (hover a window, click to take exactly it); **OCR Extract** makes
//! the capture copy text instead of the picture; the round button captures. Under a selection,
//! a bar offers Copy, Extract Text, Auto-Redact (cover secrets and personal data, shown in place
//! before you copy) and Save.
//!
//! | Key | Action |
//! |---|---|
//! | Enter, Ctrl+C, double-click | copy the picture (or its text with OCR Extract on) |
//! | T | copy the text in it |
//! | R | Auto-Redact (again: undo) |
//! | Ctrl+S | save a PNG (`~/Pictures/Screenshots` by default) |
//! | 1 / 2 / 3, O | Selection / Screen / Window, OCR Extract |
//! | Ctrl+A | select the whole screen |
//! | Arrows (Shift: 10 px), Ctrl+Arrows | move, resize |
//! | Right-click, Esc | clear the selection, close |
//!
//! The screen picture lives in memory only; its file is deleted as soon as it is read.

use crate::{UiMessage, color_picker::send_to_ui};
use aujar_ipc::{ScreenMonitor, ScreenWindow};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, FocusHandle, Focusable, Hsla,
    ImageFormat, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Pixels, RenderImage, SharedString, Stateful, StyledImage, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle, WindowOptions, div,
    img, prelude::*, px, rgb, rgba, size, svg,
};
use image::RgbaImage;
use std::{path::PathBuf, sync::Arc, time::Duration};

/// Longest wait for the daemon's picture of the screen.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
/// Half the side of the magnifier, in screen pixels (the magnifier shows 2n+1 squared).
const LOUPE_RADIUS: i64 = 6;
/// Side of one magnified pixel, in window pixels.
const LOUPE_CELL: f32 = 8.0;
/// How close (window pixels) the pointer must be to a handle to grab it.
const HANDLE_REACH: f32 = 10.0;
/// A drag shorter than this is a click, not a selection.
const MIN_DRAG: f32 = 4.0;

const ACCENT: u32 = 0x3b82f6;
const PANEL: u32 = 0x18181bf2;
const PANEL_BORDER: u32 = 0xffffff24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Select, then choose: copy, text, redact, save.
    Picture,
    /// Started for text (`aujar text`): OCR Extract is on, and the text is copied on release.
    Text,
}

/// How an area is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Selection,
    Screen,
    Window,
}

/// A picture of the whole screen, ready for the overlay.
pub(crate) struct Shot {
    pub mode: Mode,
    pub pixels: Arc<RgbaImage>,
    pub windows: Vec<ScreenWindow>,
    /// Where each monitor is in `pixels` (Windows, macOS); empty on Linux.
    pub monitors: Vec<ScreenMonitor>,
}

// ---- Geometry (window pixels; pure, tested) ----------------------------------------------

/// A rectangle in window pixels, normalized (left <= right, top <= bottom).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn spanning(a: (f32, f32), b: (f32, f32)) -> Self {
        Self {
            left: a.0.min(b.0),
            top: a.1.min(b.1),
            right: a.0.max(b.0),
            bottom: a.1.max(b.1),
        }
    }

    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }

    pub fn contains(&self, (x, y): (f32, f32)) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }

    /// Moved by `(dx, dy)` but kept inside `0..w, 0..h` with its size.
    pub fn moved(&self, dx: f32, dy: f32, (w, h): (f32, f32)) -> Self {
        let dx = dx.clamp(-self.left, (w - self.right).max(-self.left));
        let dy = dy.clamp(-self.top, (h - self.bottom).max(-self.top));

        Self {
            left: self.left + dx,
            top: self.top + dy,
            right: self.right + dx,
            bottom: self.bottom + dy,
        }
    }

    /// Clamped into `0..w, 0..h`.
    pub fn clamped(&self, (w, h): (f32, f32)) -> Self {
        Self {
            left: self.left.clamp(0.0, w),
            top: self.top.clamp(0.0, h),
            right: self.right.clamp(0.0, w),
            bottom: self.bottom.clamp(0.0, h),
        }
    }
}

/// One of the eight resize handles: which edges it moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Handle {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Handle {
    /// Corners first (they are drawn), then the edge middles (grabbable, not drawn).
    pub const ALL: [Handle; 8] = [
        Handle::at(true, true, false, false),
        Handle::at(false, true, true, false),
        Handle::at(false, false, true, true),
        Handle::at(true, false, false, true),
        Handle::at(false, true, false, false),
        Handle::at(false, false, true, false),
        Handle::at(false, false, false, true),
        Handle::at(true, false, false, false),
    ];

    const fn at(left: bool, top: bool, right: bool, bottom: bool) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub const BOTTOM_RIGHT: Handle = Handle::ALL[2];

    /// Where this handle sits on `rect`.
    pub fn position(self, rect: &Rect) -> (f32, f32) {
        let x = if self.left {
            rect.left
        } else if self.right {
            rect.right
        } else {
            (rect.left + rect.right) / 2.0
        };
        let y = if self.top {
            rect.top
        } else if self.bottom {
            rect.bottom
        } else {
            (rect.top + rect.bottom) / 2.0
        };

        (x, y)
    }

    /// `rect` with this handle's edges dragged to `to` (may flip; normalized).
    pub fn drag(self, rect: &Rect, to: (f32, f32)) -> Rect {
        let mut a = (rect.left, rect.top);
        let mut b = (rect.right, rect.bottom);

        if self.left {
            a.0 = to.0;
        }
        if self.right {
            b.0 = to.0;
        }
        if self.top {
            a.1 = to.1;
        }
        if self.bottom {
            b.1 = to.1;
        }

        Rect::spanning(a, b)
    }

    pub fn cursor(self) -> CursorStyle {
        match (self.left || self.right, self.top || self.bottom) {
            (true, false) => CursorStyle::ResizeLeftRight,
            (false, true) => CursorStyle::ResizeUpDown,
            _ if (self.left && self.top) || (self.right && self.bottom) => {
                CursorStyle::ResizeUpLeftDownRight
            }
            _ => CursorStyle::ResizeUpRightDownLeft,
        }
    }
}

/// The handle within reach of `at`, if any.
pub(crate) fn handle_at(rect: &Rect, at: (f32, f32)) -> Option<Handle> {
    Handle::ALL.into_iter().find(|handle| {
        let (x, y) = handle.position(rect);

        (x - at.0).abs() <= HANDLE_REACH && (y - at.1).abs() <= HANDLE_REACH
    })
}

/// The part of the screen picture this window shows, in picture pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Which part of a picture of all monitors (`picture` pixels, covering the union of `displays`)
/// belongs to `display`. Display bounds are in logical pixels, as the desktop lays them out.
pub(crate) fn crop_for_display(
    picture: (u32, u32),
    displays: &[(f32, f32, f32, f32)],
    display: Option<(f32, f32, f32, f32)>,
) -> Crop {
    let whole = Crop {
        x: 0,
        y: 0,
        width: picture.0,
        height: picture.1,
    };

    let Some((dx, dy, dw, dh)) = display.filter(|_| displays.len() > 1) else {
        return whole;
    };

    let left = displays.iter().map(|d| d.0).fold(f32::MAX, f32::min);
    let top = displays.iter().map(|d| d.1).fold(f32::MAX, f32::min);
    let right = displays.iter().map(|d| d.0 + d.2).fold(f32::MIN, f32::max);
    let bottom = displays.iter().map(|d| d.1 + d.3).fold(f32::MIN, f32::max);

    if right <= left || bottom <= top {
        return whole;
    }

    let sx = picture.0 as f32 / (right - left);
    let sy = picture.1 as f32 / (bottom - top);
    let x = (((dx - left) * sx).round() as u32).min(picture.0.saturating_sub(1));
    let y = (((dy - top) * sy).round() as u32).min(picture.1.saturating_sub(1));

    Crop {
        x,
        y,
        width: ((dw * sx).round() as u32).clamp(1, picture.0 - x),
        height: ((dh * sy).round() as u32).clamp(1, picture.1 - y),
    }
}

/// `rect` (window pixels, window of `window` size) as picture pixels inside `crop`.
pub(crate) fn to_picture(rect: &Rect, window: (f32, f32), crop: &Crop) -> Crop {
    let kx = crop.width as f32 / window.0.max(1.0);
    let ky = crop.height as f32 / window.1.max(1.0);
    let x = (rect.left * kx).round().max(0.0) as u32;
    let y = (rect.top * ky).round().max(0.0) as u32;
    let x = x.min(crop.width.saturating_sub(1));
    let y = y.min(crop.height.saturating_sub(1));
    let width = ((rect.width() * kx).round() as u32).clamp(1, crop.width - x);
    let height = ((rect.height() * ky).round() as u32).clamp(1, crop.height - y);

    Crop {
        x: crop.x + x,
        y: crop.y + y,
        width,
        height,
    }
}

/// Which of the daemon's monitors this overlay covers: the one at the display GPUI names, else
/// the only one of the overlay's size, else the first of that size.
pub(crate) fn pick_monitor(
    monitors: &[ScreenMonitor],
    display: Option<(f32, f32, f32, f32)>,
    window: (f32, f32),
) -> Option<usize> {
    let near = |a: f32, b: f32| (a - b).abs() < 2.0;

    if let Some((x, y, w, h)) = display
        && let Some(index) = monitors
            .iter()
            .position(|m| near(m.x, x) && near(m.y, y) && near(m.width, w) && near(m.height, h))
    {
        return Some(index);
    }

    let sized: Vec<usize> = monitors
        .iter()
        .enumerate()
        .filter(|(_, m)| near(m.width, window.0) && near(m.height, window.1))
        .map(|(index, _)| index)
        .collect();

    match sized.as_slice() {
        [] if monitors.len() == 1 => Some(0),
        [] => None,
        [first, ..] => Some(*first),
    }
}

/// A window the pointer can pick, in overlay pixels, topmost first.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Target {
    pub rect: Rect,
    pub label: String,
}

/// The desktop's windows (logical pixels, topmost first) as overlay rects: relative to the
/// display at `origin` of logical size `display`, scaled to the overlay's `window` size, clipped
/// to it; windows entirely off this display are dropped.
pub(crate) fn targets(
    windows: &[ScreenWindow],
    origin: (f32, f32),
    display: (f32, f32),
    window: (f32, f32),
) -> Vec<Target> {
    let kx = window.0 / display.0.max(1.0);
    let ky = window.1 / display.1.max(1.0);

    windows
        .iter()
        .filter_map(|w| {
            let rect = Rect {
                left: (w.x as f32 - origin.0) * kx,
                top: (w.y as f32 - origin.1) * ky,
                right: (w.x as f32 + w.width as f32 - origin.0) * kx,
                bottom: (w.y as f32 + w.height as f32 - origin.1) * ky,
            }
            .clamped(window);

            (rect.width() >= 8.0 && rect.height() >= 8.0).then(|| Target {
                rect,
                label: match (w.app.trim(), w.title.trim()) {
                    ("", title) => title.to_owned(),
                    (app, "") => app.to_owned(),
                    (app, title) if title.contains(app) => title.to_owned(),
                    (app, title) => format!("{app} \u{2014} {title}"),
                },
            })
        })
        .collect()
}

/// The topmost target under `at`.
pub(crate) fn target_at(targets: &[Target], at: (f32, f32)) -> Option<usize> {
    targets.iter().position(|target| target.rect.contains(at))
}

/// Top-left corner for a box of `size` next to `anchor`, flipped to stay inside `window`.
pub(crate) fn place_beside(anchor: (f32, f32), size: (f32, f32), window: (f32, f32)) -> (f32, f32) {
    let gap = 24.0;
    let x = if anchor.0 + gap + size.0 <= window.0 {
        anchor.0 + gap
    } else {
        (anchor.0 - gap - size.0).max(0.0)
    };
    let y = if anchor.1 + gap + size.1 <= window.1 {
        anchor.1 + gap
    } else {
        (anchor.1 - gap - size.1).max(0.0)
    };

    (x, y)
}

/// Top-left corner for the actions bar: under the selection, else above it, else inside it.
pub(crate) fn place_bar(selection: &Rect, bar: (f32, f32), window: (f32, f32)) -> (f32, f32) {
    let gap = 10.0;
    let x = (selection.right - bar.0).clamp(0.0, (window.0 - bar.0).max(0.0));

    let y = if selection.bottom + gap + bar.1 <= window.1 {
        selection.bottom + gap
    } else if selection.top - gap - bar.1 >= 0.0 {
        selection.top - gap - bar.1
    } else {
        (selection.bottom - gap - bar.1).max(0.0)
    };

    (x, y)
}

// ---- Starting: capture, then open the window ---------------------------------------------

/// Ask the daemon for the screen, then open the overlay (on the UI thread, via a message).
pub(crate) fn start(cx: &mut App, mode: Mode) {
    cx.spawn(async move |cx| {
        let shot = cx
            .background_executor()
            .spawn(async move {
                capture().map(|(pixels, windows, monitors)| Shot {
                    mode,
                    pixels,
                    windows,
                    monitors,
                })
            })
            .await;

        match shot {
            Ok(shot) => send_to_ui(UiMessage::SnipReady(shot)),
            Err(error) => {
                crate::debug(&format!("snip: {error}"));
                notify("Aujar Snip", &error);
            }
        }
    })
    .detach();
}

#[allow(clippy::type_complexity)]
fn capture() -> Result<(Arc<RgbaImage>, Vec<ScreenWindow>, Vec<ScreenMonitor>), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;

    let answer = runtime.block_on(async {
        tokio::time::timeout(
            CAPTURE_TIMEOUT,
            aujar_ipc::send(crate::socket_path(), aujar_ipc::Request::CaptureScreen),
        )
        .await
    });

    let (path, windows, monitors) = match answer {
        Err(_) => return Err("The screen capture took too long.".to_owned()),
        Ok(Err(_)) => {
            return Err(
                "The Aujar daemon is not running. Start it with `systemctl --user start aujar`."
                    .to_owned(),
            );
        }
        Ok(Ok(aujar_ipc::Response::Screen {
            path,
            windows,
            monitors,
        })) => (PathBuf::from(path), windows, monitors),
        Ok(Ok(aujar_ipc::Response::Error(message))) => return Err(message),
        Ok(Ok(_)) => {
            return Err(
                "The daemon is older than the UI; restart it (`systemctl --user restart aujar`)."
                    .to_owned(),
            );
        }
    };

    let bytes = std::fs::read(&path);
    // A picture of the whole screen must not stay on disk.
    let _ = std::fs::remove_file(&path);

    let bytes = bytes.map_err(|error| format!("Could not read the screen picture: {error}"))?;

    image::load_from_memory(&bytes)
        .map(|picture| (Arc::new(picture.to_rgba8()), windows, monitors))
        .map_err(|error| format!("Could not read the screen picture: {error}"))
}

/// Open the overlay full screen. The desktop puts it on the monitor you are using.
pub(crate) fn open(cx: &mut App, shot: Shot) -> Option<WindowHandle<SnipView>> {
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Fullscreen(bounds)),
        titlebar: None,
        window_decorations: Some(WindowDecorations::Client),
        window_background: WindowBackgroundAppearance::Opaque,
        app_id: Some("aujar".into()),
        focus: true,
        show: true,
        ..Default::default()
    };

    let handle = cx
        .open_window(options, |window, cx| {
            let view = cx.new(|cx| SnipView::new(shot, cx));
            let focus = view.read(cx).focus_handle.clone();

            window.focus(&focus, cx);

            view
        })
        .ok()?;

    // The desktop may ignore a fullscreen request made before the window was shown; ask again
    // once it is up, so the overlay never sits in a window of its own.
    cx.spawn(async move |cx| {
        for _ in 0..3 {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;

            let done = handle
                .update(cx, |_, window, _| {
                    if !window.is_fullscreen() {
                        window.toggle_fullscreen();
                    }

                    window.is_fullscreen()
                })
                .unwrap_or(true);

            if done {
                break;
            }
        }
    })
    .detach();

    Some(handle)
}

/// A desktop notification (`notify-send`); best effort, off the UI thread.
fn notify(summary: &str, body: &str) {
    let (summary, body) = (summary.to_owned(), body.to_owned());

    std::thread::spawn(move || {
        let _ = std::process::Command::new("notify-send")
            .args([
                "--app-name=Aujar",
                "--icon=applets-screenshooter",
                "--",
                &summary,
                &body,
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// A frame GPUI can draw (it wants BGRA).
fn render_image(picture: &RgbaImage) -> Arc<RenderImage> {
    let mut frame = picture.clone();

    for pixel in frame.pixels_mut() {
        pixel.0.swap(0, 2);
    }

    Arc::new(RenderImage::new([image::Frame::new(frame)]))
}

// ---- The overlay -------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    None,
    /// Drawing a new selection from this corner.
    New((f32, f32)),
    /// Moving the selection; the pointer was here when it started.
    Move {
        from: (f32, f32),
        original: Rect,
    },
    Resize {
        handle: Handle,
        original: Rect,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Busy {
    Text,
    Redact,
}

/// Auto-Redact's result, shown over the selection until it changes.
struct Redacted {
    rect: Rect,
    picture: RgbaImage,
    render: Arc<RenderImage>,
    count: usize,
}

/// What the overlay shows of the screen, worked out for one window size.
struct View {
    size: (f32, f32),
    crop: Crop,
    render: Arc<RenderImage>,
    targets: Vec<Target>,
}

pub(crate) struct SnipView {
    shot: Arc<RgbaImage>,
    windows: Vec<ScreenWindow>,
    monitors: Vec<ScreenMonitor>,
    focus_handle: FocusHandle,
    view: Option<View>,
    tool: Tool,
    /// OCR Extract: capturing copies the text instead of the picture.
    ocr: bool,
    /// Started for text: copy the text as soon as an area is chosen.
    text_mode: bool,
    pointer: (f32, f32),
    selection: Option<Rect>,
    drag: Drag,
    redacted: Option<Redacted>,
    busy: Option<Busy>,
    /// A message over the overlay (no text found, an error).
    message: Option<String>,
}

impl Focusable for SnipView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// The local time as `2026-11-28 12-03-04`, for file names.
fn file_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());

    let Some(tm) = crate::local_tm(secs) else {
        return secs.to_string();
    };

    format!(
        "{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

fn save_folder() -> PathBuf {
    let configured = aujar_config::Settings::load_or_default().snip.folder;

    if !configured.trim().is_empty() {
        let configured = configured.trim();

        return match configured.strip_prefix("~/") {
            Some(rest) => aujar_config::platform::home_dir()
                .unwrap_or_default()
                .join(rest),
            None => PathBuf::from(configured),
        };
    }

    // The desktop's Pictures folder (it may be translated), else ~/Pictures.
    let pictures = std::process::Command::new("xdg-user-dir")
        .arg("PICTURES")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
        .filter(|path| path.is_absolute() && path.is_dir())
        .or_else(|| aujar_config::platform::home_dir().map(|home| home.join("Pictures")))
        .unwrap_or_else(std::env::temp_dir);

    pictures.join("Screenshots")
}

fn png(picture: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();

    image::DynamicImage::ImageRgba8(picture.clone())
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|error| format!("Could not make the PNG: {error}"))?;

    Ok(bytes)
}

/// How an action button is colored.
#[derive(Clone, Copy)]
enum Tint {
    Primary,
    Green,
    Amber,
    Plain,
}

impl SnipView {
    fn new(shot: Shot, cx: &mut Context<Self>) -> Self {
        let text_mode = shot.mode == Mode::Text;

        Self {
            shot: shot.pixels,
            windows: shot.windows,
            monitors: shot.monitors,
            focus_handle: cx.focus_handle(),
            view: None,
            tool: Tool::Selection,
            ocr: text_mode,
            text_mode,
            pointer: (-1000.0, -1000.0),
            selection: None,
            drag: Drag::None,
            redacted: None,
            busy: None,
            message: None,
        }
    }

    fn window_size(window: &Window) -> (f32, f32) {
        let viewport = window.viewport_size();

        (f32::from(viewport.width), f32::from(viewport.height))
    }

    /// Work out which part of the screen picture this window shows, and where the windows are
    /// in it; again whenever the window size changes (it becomes full screen after opening).
    fn ensure_view(&mut self, window: &Window, cx: &App) {
        let size = Self::window_size(window);

        if self.view.as_ref().is_some_and(|view| view.size == size) {
            return;
        }

        let rect = |bounds: Bounds<Pixels>| {
            (
                f32::from(bounds.origin.x),
                f32::from(bounds.origin.y),
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
            )
        };
        let displays: Vec<_> = cx
            .displays()
            .iter()
            .map(|display| rect(display.bounds()))
            .collect();

        // The window's display: the one GPUI names, else the only one this size.
        let named = window.display(cx).map(|display| rect(display.bounds()));
        let sized: Vec<_> = displays
            .iter()
            .copied()
            .filter(|d| (d.2 - size.0).abs() < 2.0 && (d.3 - size.1).abs() < 2.0)
            .collect();
        let display = named
            .or_else(|| (sized.len() == 1).then(|| sized[0]))
            .or_else(|| (displays.len() == 1).then(|| displays[0]));

        // Windows and macOS say exactly where each monitor is in the picture (right also with
        // mixed scale factors); on Linux the picture is matched to the display layout.
        let (crop, origin, logical) = match pick_monitor(&self.monitors, display, size) {
            Some(index) => {
                let m = &self.monitors[index];

                (
                    Crop {
                        x: m.picture_x.min(self.shot.width().saturating_sub(1)),
                        y: m.picture_y.min(self.shot.height().saturating_sub(1)),
                        width: m
                            .picture_width
                            .clamp(1, self.shot.width().saturating_sub(m.picture_x).max(1)),
                        height: m
                            .picture_height
                            .clamp(1, self.shot.height().saturating_sub(m.picture_y).max(1)),
                    },
                    (m.x, m.y),
                    (m.width, m.height),
                )
            }
            None => (
                crop_for_display((self.shot.width(), self.shot.height()), &displays, display),
                display.map_or((0.0, 0.0), |d| (d.0, d.1)),
                display.map_or(size, |d| (d.2, d.3)),
            ),
        };
        let frame =
            image::imageops::crop_imm(self.shot.as_ref(), crop.x, crop.y, crop.width, crop.height)
                .to_image();

        self.view = Some(View {
            size,
            crop,
            render: render_image(&frame),
            targets: targets(&self.windows, origin, logical, size),
        });

        // A selection made for another size no longer lines up.
        self.selection = None;
        self.redacted = None;
    }

    fn close(&mut self, window: &mut Window) {
        window.remove_window();
    }

    /// The selection changed: an earlier redaction no longer applies.
    fn set_selection(&mut self, selection: Option<Rect>) {
        if self.redacted.as_ref().map(|r| r.rect) != selection {
            self.redacted = None;
        }

        self.selection = selection;
    }

    fn hovered_target(&self) -> Option<&Target> {
        let view = self.view.as_ref()?;

        (self.tool == Tool::Window && self.drag == Drag::None)
            .then(|| target_at(&view.targets, self.pointer))
            .flatten()
            .map(|index| &view.targets[index])
    }

    fn set_tool(&mut self, tool: Tool, window: &Window, cx: &mut Context<Self>) {
        self.tool = tool;
        self.message = None;

        match tool {
            Tool::Screen => {
                let size = Self::window_size(window);

                self.set_selection(Some(Rect::spanning((0.0, 0.0), size)));
            }
            Tool::Window | Tool::Selection => self.set_selection(None),
        }

        cx.notify();
    }

    /// The selection as picture pixels (the redacted copy when it was redacted).
    fn selected_picture(&self, window: &Window) -> Option<RgbaImage> {
        let selection = self.selection?;

        if let Some(redacted) = self.redacted.as_ref().filter(|r| r.rect == selection) {
            return Some(redacted.picture.clone());
        }

        let view = self.view.as_ref()?;
        let area = to_picture(&selection, Self::window_size(window), &view.crop);

        Some(
            image::imageops::crop_imm(self.shot.as_ref(), area.x, area.y, area.width, area.height)
                .to_image(),
        )
    }

    /// The round button, Enter, double-click: copy text with OCR Extract on, else the picture.
    fn capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_none() && self.tool == Tool::Screen {
            let size = Self::window_size(window);

            self.set_selection(Some(Rect::spanning((0.0, 0.0), size)));
        }

        if self.selection.is_none() {
            self.message = Some("Select an area first".to_owned());
            cx.notify();
            return;
        }

        if self.ocr {
            self.copy_text(window, cx);
        } else {
            self.copy_picture(window, cx);
        }
    }

    fn copy_picture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picture) = self.selected_picture(window) else {
            return;
        };

        match png(&picture) {
            Ok(bytes) => {
                let image = gpui::Image::from_bytes(ImageFormat::Png, bytes);

                // Written while the window still has focus: Wayland wants that.
                cx.write_to_clipboard(ClipboardItem::new_image(&image));
                notify(
                    "Snip copied",
                    &format!(
                        "{} \u{d7} {} picture on the clipboard",
                        picture.width(),
                        picture.height()
                    ),
                );
                self.close(window);
            }
            Err(error) => {
                self.message = Some(error);
                cx.notify();
            }
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picture) = self.selected_picture(window) else {
            return;
        };

        let folder = save_folder();
        let path = folder.join(format!("Snip {}.png", file_stamp()));
        let saved = std::fs::create_dir_all(&folder)
            .map_err(|error| error.to_string())
            .and_then(|()| png(&picture))
            .and_then(|bytes| std::fs::write(&path, bytes).map_err(|error| error.to_string()));

        match saved {
            Ok(()) => {
                notify("Snip saved", &path.display().to_string());
                self.close(window);
            }
            Err(error) => {
                self.message = Some(format!("Could not save in {}: {error}", folder.display()));
                cx.notify();
            }
        }
    }

    fn copy_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(picture) = self.selected_picture(window) else {
            return;
        };

        self.busy = Some(Busy::Text);
        self.message = None;
        cx.notify();

        let settings = aujar_config::Settings::load_or_default().text_extractor;
        let options = aujar_text_extractor::Options {
            engine: settings.engine.parse().unwrap_or_default(),
            languages: settings.languages,
            single_line: settings.single_line,
            backend: aujar_text_extractor::Backend::Auto,
        };

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    aujar_text_extractor::extract_from_image(
                        &image::DynamicImage::ImageRgba8(picture),
                        &options,
                    )
                    .map_err(|error| format!("{error:#}"))
                })
                .await;

            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;

                match result {
                    Ok(extracted) if !extracted.text.is_empty() => {
                        cx.write_to_clipboard(ClipboardItem::new_string(extracted.text.clone()));
                        notify("Text copied", &preview(&extracted.text, 120));
                        this.close(window);
                    }
                    Ok(extracted) => {
                        this.message = Some(format!(
                            "No text found ({}). Select another area, or Esc to close.",
                            extracted.languages.join("+")
                        ));
                        cx.notify();
                    }
                    Err(error) => {
                        this.message = Some(error);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Cover secrets and personal data in the selection; again to undo.
    fn toggle_redact(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.redacted.take().is_some() {
            self.message = None;
            cx.notify();
            return;
        }

        let (Some(rect), Some(picture)) = (self.selection, self.selected_picture(window)) else {
            return;
        };

        self.busy = Some(Busy::Redact);
        self.message = None;
        cx.notify();

        let settings = aujar_config::Settings::load_or_default().text_extractor;
        let (languages, engine) = (
            settings.languages,
            settings.engine.parse().unwrap_or_default(),
        );

        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    aujar_text_extractor::redact_image(&picture, &languages, engine)
                        .map_err(|error| format!("{error:#}"))
                })
                .await;

            let _ = this.update_in(cx, |this, _window, cx| {
                this.busy = None;

                match result {
                    // Only if the selection did not change meanwhile.
                    Ok((covered, count)) if this.selection == Some(rect) => {
                        this.message = Some(if count == 0 {
                            "Nothing sensitive found. Check before sharing.".to_owned()
                        } else {
                            format!(
                                "Covered {count} item{}. Check before sharing; R undoes.",
                                if count == 1 { "" } else { "s" }
                            )
                        });
                        this.redacted = Some(Redacted {
                            rect,
                            render: render_image(&covered),
                            picture: covered,
                            count,
                        });
                    }
                    Ok(_) => {}
                    Err(error) => this.message = Some(error),
                }

                cx.notify();
            });
        })
        .detach();
    }

    // ---- Input ----

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.is_some() {
            return;
        }

        let at = (f32::from(event.position.x), f32::from(event.position.y));
        self.message = None;

        if event.button == MouseButton::Right {
            if self.selection.is_some() {
                self.set_selection(None);
            } else {
                self.close(window);
            }

            self.drag = Drag::None;
            cx.notify();
            return;
        }

        if event.button != MouseButton::Left {
            return;
        }

        self.drag = match self.selection {
            Some(selection) if event.click_count >= 2 && selection.contains(at) => {
                self.capture(window, cx);
                return;
            }
            Some(selection) => match handle_at(&selection, at) {
                Some(handle) => Drag::Resize {
                    handle,
                    original: selection,
                },
                None if selection.contains(at) => Drag::Move {
                    from: at,
                    original: selection,
                },
                None => Drag::New(at),
            },
            None => Drag::New(at),
        };

        if let Drag::New(origin) = self.drag {
            self.set_selection(Some(Rect::spanning(origin, origin)));
        }

        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = (f32::from(event.position.x), f32::from(event.position.y));
        let bounds = Self::window_size(window);

        self.pointer = at;

        let selection = match self.drag {
            Drag::None => self.selection,
            Drag::New(origin) => Some(Rect::spanning(origin, at).clamped(bounds)),
            Drag::Move { from, original } => {
                Some(original.moved(at.0 - from.0, at.1 - from.1, bounds))
            }
            Drag::Resize { handle, original } => Some(handle.drag(&original, at).clamped(bounds)),
        };

        self.set_selection(selection);
        cx.notify();
    }

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let drag = self.drag;

        self.drag = Drag::None;

        let clicked = self
            .selection
            .is_some_and(|s| s.width() < MIN_DRAG || s.height() < MIN_DRAG);

        if clicked {
            // In window mode a click takes the window under the pointer; elsewhere it is not
            // a selection.
            let picked = match (self.tool, self.view.as_ref()) {
                (Tool::Window, Some(view)) => {
                    target_at(&view.targets, self.pointer).map(|index| view.targets[index].rect)
                }
                _ => None,
            };

            self.set_selection(picked);
        }

        let chosen = matches!(drag, Drag::New(_)) && self.selection.is_some();

        if chosen && self.text_mode {
            self.copy_text(window, cx);
        }

        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();

        if self.busy.is_some() {
            if event.keystroke.key == "escape" {
                self.close(window);
            }

            return;
        }

        let modifiers = &event.keystroke.modifiers;
        let (ctrl, shift) = (modifiers.control || modifiers.platform, modifiers.shift);
        let bounds = Self::window_size(window);
        let step = if shift { 10.0 } else { 1.0 };

        match event.keystroke.key.as_str() {
            "escape" => self.close(window),
            "enter" => self.capture(window, cx),
            "c" if ctrl => self.copy_picture(window, cx),
            "s" if ctrl => self.save(window, cx),
            "a" if ctrl => self.set_tool(Tool::Screen, window, cx),
            "t" if !ctrl => self.copy_text(window, cx),
            "r" if !ctrl => self.toggle_redact(window, cx),
            "o" if !ctrl => {
                self.ocr = !self.ocr;
                cx.notify();
            }
            "1" => self.set_tool(Tool::Selection, window, cx),
            "2" => self.set_tool(Tool::Screen, window, cx),
            "3" => self.set_tool(Tool::Window, window, cx),
            key @ ("left" | "right" | "up" | "down") => {
                let Some(selection) = self.selection else {
                    return;
                };
                let (dx, dy) = match key {
                    "left" => (-step, 0.0),
                    "right" => (step, 0.0),
                    "up" => (0.0, -step),
                    _ => (0.0, step),
                };

                let next = if ctrl {
                    // Ctrl: grow or shrink from the bottom-right corner.
                    let corner = (selection.right + dx, selection.bottom + dy);
                    let resized = Handle::BOTTOM_RIGHT
                        .drag(&selection, corner)
                        .clamped(bounds);

                    if resized.width() >= 1.0 && resized.height() >= 1.0 {
                        resized
                    } else {
                        selection
                    }
                } else {
                    selection.moved(dx, dy, bounds)
                };

                self.set_selection(Some(next));
                cx.notify();
            }
            _ => {}
        }
    }

    // ---- Drawing ----

    fn loupe(&self, window: &Window) -> Option<AnyElement> {
        let view = self.view.as_ref()?;
        let bounds = Self::window_size(window);

        if !Rect::spanning((0.0, 0.0), bounds).contains(self.pointer) {
            return None;
        }

        let center = to_picture(
            &Rect::spanning(self.pointer, self.pointer),
            bounds,
            &view.crop,
        );
        let (cx0, cy0) = (i64::from(center.x), i64::from(center.y));
        let side = (2 * LOUPE_RADIUS + 1) as f32 * LOUPE_CELL;

        let mut rows = Vec::new();
        let mut hex = String::new();

        for dy in -LOUPE_RADIUS..=LOUPE_RADIUS {
            let mut cells = Vec::new();

            for dx in -LOUPE_RADIUS..=LOUPE_RADIUS {
                let (x, y) = (cx0 + dx, cy0 + dy);
                let color = (x >= 0
                    && y >= 0
                    && x < i64::from(self.shot.width())
                    && y < i64::from(self.shot.height()))
                .then(|| self.shot.get_pixel(x as u32, y as u32).0)
                .map_or(0x101010, |[r, g, b, _]| {
                    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
                });

                if dx == 0 && dy == 0 {
                    hex = format!("#{color:06X}");
                }

                cells.push(
                    div()
                        .w(px(LOUPE_CELL))
                        .h(px(LOUPE_CELL))
                        .bg(rgb(color))
                        .when(dx == 0 && dy == 0, |cell| {
                            cell.border_1().border_color(rgb(0xffffff))
                        })
                        .into_any_element(),
                );
            }

            rows.push(div().flex().flex_row().children(cells).into_any_element());
        }

        let info = div()
            .px_2()
            .py_1()
            .text_xs()
            .text_color(rgb(0xe4e4e7))
            .child(format!(
                "{}, {}  {hex}",
                center.x - view.crop.x,
                center.y - view.crop.y
            ));

        let (x, y) = place_beside(self.pointer, (side, side + 24.0), bounds);

        Some(
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .rounded_lg()
                .overflow_hidden()
                .border_1()
                .border_color(rgba(PANEL_BORDER))
                .bg(rgba(PANEL))
                .shadow_lg()
                .child(div().flex().flex_col().children(rows))
                .child(info)
                .into_any_element(),
        )
    }

    fn icon(path: &'static str, color: impl Into<Hsla>) -> impl IntoElement {
        svg()
            .path(path)
            .size(px(15.))
            .flex_shrink_0()
            .text_color(color)
    }

    fn action_button(
        &self,
        id: &'static str,
        icon: &'static str,
        label: &str,
        tint: Tint,
    ) -> Stateful<gpui::Div> {
        let (bg, hover, fg): (u32, u32, u32) = match tint {
            Tint::Primary => (0x2563ebff, 0x3b82f6ff, 0xffffff),
            Tint::Green => (0x10b98126, 0x10b98140, 0x6ee7b7),
            Tint::Amber => (0xf59e0b26, 0xf59e0b40, 0xfcd34d),
            Tint::Plain => (0x00000000, 0xffffff1f, 0xe4e4e7),
        };

        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1p5()
            .rounded_md()
            .cursor(CursorStyle::PointingHand)
            .text_sm()
            .text_color(rgb(fg))
            .bg(rgba(bg))
            .hover(move |b| b.bg(rgba(hover)))
            .child(Self::icon(icon, rgb(fg)))
            .when(!label.is_empty(), |b| {
                b.child(SharedString::from(label.to_owned()))
            })
            // Clicking a button must not start a new selection.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    }

    fn actions_bar(&self, selection: &Rect, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let redacted = self.redacted.is_some();
        let bar_size = (470.0, 40.0);
        let (x, y) = place_bar(selection, bar_size, Self::window_size(window));

        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .p_1()
            .rounded_lg()
            .bg(rgba(PANEL))
            .border_1()
            .border_color(rgba(PANEL_BORDER))
            .shadow_lg()
            .child(
                self.action_button("snip-copy", "icons/copy.svg", "Copy", Tint::Primary)
                    .on_click(cx.listener(|this, _, window, cx| this.copy_picture(window, cx))),
            )
            .child(
                self.action_button("snip-text", "icons/type.svg", "Extract Text", Tint::Green)
                    .on_click(cx.listener(|this, _, window, cx| this.copy_text(window, cx))),
            )
            .child(
                self.action_button(
                    "snip-redact",
                    "icons/shield.svg",
                    if redacted {
                        "Undo Redact"
                    } else {
                        "Auto-Redact"
                    },
                    Tint::Amber,
                )
                .on_click(cx.listener(|this, _, window, cx| this.toggle_redact(window, cx))),
            )
            .child(
                self.action_button("snip-save", "icons/download.svg", "Save", Tint::Plain)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
            .child(
                self.action_button("snip-close", "icons/x.svg", "", Tint::Plain)
                    .on_click(cx.listener(|this, _, window, _| this.close(window))),
            )
            .into_any_element()
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let tool_button = |id: &'static str,
                           icon: &'static str,
                           label: &'static str,
                           tool: Tool| {
            let active = self.tool == tool;

            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap_1p5()
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor(CursorStyle::PointingHand)
                .text_sm()
                .text_color(rgb(if active { 0xffffff } else { 0xa1a1aa }))
                .when(active, |b| b.bg(rgba(0xffffff26)))
                .when(!active, |b| b.hover(|b| b.bg(rgba(0xffffff14))))
                .child(Self::icon(
                    icon,
                    rgb(if active { 0xffffff } else { 0xa1a1aa }),
                ))
                .child(label)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| this.set_tool(tool, window, cx)))
        };

        let divider = || div().w(px(1.)).h(px(22.)).mx_1().bg(rgba(0xffffff26));

        let ocr = self.ocr;
        let ocr_button = div()
            .id("snip-ocr")
            .flex()
            .flex_row()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_1p5()
            .rounded_md()
            .cursor(CursorStyle::PointingHand)
            .text_sm()
            .text_color(rgb(if ocr { 0x6ee7b7 } else { 0xa1a1aa }))
            .when(ocr, |b| b.bg(rgba(0x10b98126)))
            .when(!ocr, |b| b.hover(|b| b.bg(rgba(0xffffff14))))
            .child(Self::icon(
                "icons/scan-text.svg",
                rgb(if ocr { 0x6ee7b7 } else { 0xa1a1aa }),
            ))
            .child("OCR Extract")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| {
                this.ocr = !this.ocr;
                cx.notify();
            }));

        // The round capture button: copies the picture, or its text with OCR Extract on.
        let shutter = div()
            .id("snip-capture")
            .mx_1()
            .size(px(34.))
            .rounded_full()
            .border_2()
            .border_color(rgb(0xffffff))
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::PointingHand)
            .child(div().size(px(24.)).rounded_full().bg(rgb(if ocr {
                0x6ee7b7
            } else {
                0xffffff
            })))
            .hover(|b| b.opacity(0.85))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| this.capture(window, cx)));

        let close = div()
            .id("snip-exit")
            .size(px(30.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0xffffff1a))
            .hover(|b| b.bg(rgba(0xffffff33)))
            .cursor(CursorStyle::PointingHand)
            .child(Self::icon("icons/x.svg", rgb(0xe4e4e7)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, _| this.close(window)));

        div()
            .absolute()
            .bottom(px(36.))
            .left_0()
            .w_full()
            .flex()
            .justify_center()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .p_1p5()
                    .rounded_xl()
                    .bg(rgba(PANEL))
                    .border_1()
                    .border_color(rgba(PANEL_BORDER))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .p_0p5()
                            .rounded_lg()
                            .bg(rgba(0xffffff0d))
                            .child(tool_button(
                                "snip-tool-selection",
                                "icons/selection.svg",
                                "Selection",
                                Tool::Selection,
                            ))
                            .child(tool_button(
                                "snip-tool-screen",
                                "icons/monitor.svg",
                                "Screen",
                                Tool::Screen,
                            ))
                            .child(tool_button(
                                "snip-tool-window",
                                "icons/window.svg",
                                "Window",
                                Tool::Window,
                            )),
                    )
                    .child(divider())
                    .child(ocr_button)
                    .child(divider())
                    .child(shutter)
                    .child(close),
            )
            .into_any_element()
    }

    fn banner(&self) -> Option<String> {
        if let Some(busy) = self.busy {
            return Some(
                match busy {
                    Busy::Text => "Reading text\u{2026}",
                    Busy::Redact => "Looking for secrets and personal data\u{2026}",
                }
                .to_owned(),
            );
        }

        if let Some(message) = &self.message {
            return Some(message.clone());
        }

        if self.selection.is_some() {
            return None;
        }

        Some(match self.tool {
            Tool::Window if self.view.as_ref().is_some_and(|v| v.targets.is_empty()) => {
                "No windows to pick: the Aujar Shell extension is missing or older. Run \
                 `aujar gnome-extension install`, then log out and back in."
                    .to_owned()
            }
            Tool::Window => "Click a window to take it  \u{b7}  Esc cancels".to_owned(),
            _ if self.text_mode => "Drag over text to copy it  \u{b7}  Esc cancels".to_owned(),
            _ => "Drag to select  \u{b7}  Enter copies  \u{b7}  Esc cancels".to_owned(),
        })
    }
}

impl Render for SnipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_view(window, cx);

        let bounds = Self::window_size(window);
        let dim = rgba(0x0000008f);
        let picture = self.view.as_ref().map(|view| view.render.clone());

        let mut layers: Vec<AnyElement> = Vec::new();

        if let Some(picture) = picture {
            layers.push(
                img(picture)
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full()
                    .object_fit(ObjectFit::Fill)
                    .into_any_element(),
            );
        }

        let shade = |left: f32, top: f32, width: f32, height: f32| {
            div()
                .absolute()
                .left(px(left))
                .top(px(top))
                .w(px(width.max(0.0)))
                .h(px(height.max(0.0)))
                .bg(dim)
                .into_any_element()
        };
        let cut_out = |area: &Rect, layers: &mut Vec<AnyElement>| {
            layers.push(shade(0.0, 0.0, bounds.0, area.top));
            layers.push(shade(0.0, area.bottom, bounds.0, bounds.1 - area.bottom));
            layers.push(shade(0.0, area.top, area.left, area.height()));
            layers.push(shade(
                area.right,
                area.top,
                bounds.0 - area.right,
                area.height(),
            ));
        };

        let hovered = self.hovered_target().cloned();

        match (self.selection, &hovered) {
            (Some(s), _) => {
                cut_out(&s, &mut layers);

                if let Some(redacted) = self.redacted.as_ref().filter(|r| r.rect == s) {
                    layers.push(
                        img(redacted.render.clone())
                            .absolute()
                            .left(px(s.left))
                            .top(px(s.top))
                            .w(px(s.width()))
                            .h(px(s.height()))
                            .object_fit(ObjectFit::Fill)
                            .into_any_element(),
                    );
                }

                layers.push(
                    div()
                        .absolute()
                        .left(px(s.left - 1.0))
                        .top(px(s.top - 1.0))
                        .w(px(s.width() + 2.0))
                        .h(px(s.height() + 2.0))
                        .border_1()
                        .border_color(rgb(0xffffff))
                        .into_any_element(),
                );

                if self.drag == Drag::None {
                    for handle in &Handle::ALL[..4] {
                        let (x, y) = handle.position(&s);

                        layers.push(
                            div()
                                .absolute()
                                .left(px(x - 4.5))
                                .top(px(y - 4.5))
                                .size(px(9.0))
                                .rounded_full()
                                .bg(rgb(0xffffff))
                                .shadow_sm()
                                .into_any_element(),
                        );
                    }
                }

                // The size, above the top-left corner (inside it at the top of the screen).
                if let Some(view) = &self.view {
                    let area = to_picture(&s, bounds, &view.crop);
                    let label_y = if s.top >= 30.0 {
                        s.top - 28.0
                    } else {
                        s.top + 6.0
                    };
                    let redacted = self
                        .redacted
                        .as_ref()
                        .filter(|r| r.rect == s)
                        .map(|r| format!("  \u{b7}  {} covered", r.count));

                    layers.push(
                        div()
                            .absolute()
                            .left(px(s.left))
                            .top(px(label_y))
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(rgba(PANEL))
                            .border_1()
                            .border_color(rgba(PANEL_BORDER))
                            .text_xs()
                            .text_color(rgb(0xe4e4e7))
                            .child(format!(
                                "{} \u{d7} {} px{}",
                                area.width,
                                area.height,
                                redacted.unwrap_or_default()
                            ))
                            .into_any_element(),
                    );
                }
            }
            (None, Some(target)) => {
                // Window mode: the window under the pointer, lit up and named.
                cut_out(&target.rect, &mut layers);

                let r = target.rect;
                layers.push(
                    div()
                        .absolute()
                        .left(px(r.left))
                        .top(px(r.top))
                        .w(px(r.width()))
                        .h(px(r.height()))
                        .border_2()
                        .border_color(rgb(ACCENT))
                        .bg(rgba(0x3b82f612))
                        .into_any_element(),
                );
                layers.push(
                    div()
                        .absolute()
                        .left(px(r.left + 8.0))
                        .top(px(r.top + 8.0))
                        .max_w(px((r.width() - 16.0).max(40.0)))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .bg(rgba(0x1d4ed8e6))
                        .text_xs()
                        .text_color(rgb(0xffffff))
                        .child(SharedString::from(target.label.clone()))
                        .into_any_element(),
                );
            }
            (None, None) => {
                layers.push(shade(0.0, 0.0, bounds.0, bounds.1));

                if self.tool == Tool::Selection {
                    // Crosshair guides through the pointer.
                    let (x, y) = self.pointer;
                    let guide = rgba(0xffffff40);

                    layers.push(
                        div()
                            .absolute()
                            .left(px(x))
                            .top_0()
                            .w(px(1.0))
                            .h_full()
                            .bg(guide)
                            .into_any_element(),
                    );
                    layers.push(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(y))
                            .w_full()
                            .h(px(1.0))
                            .bg(guide)
                            .into_any_element(),
                    );
                }
            }
        }

        let selecting = self.tool == Tool::Selection
            && (self.selection.is_none()
                || matches!(self.drag, Drag::New(_) | Drag::Resize { .. }));

        if selecting && self.busy.is_none() {
            layers.extend(self.loupe(window));
        }

        if let (Some(selection), Drag::None, None, false) =
            (self.selection, self.drag, self.busy, self.text_mode)
        {
            layers.push(self.actions_bar(&selection, window, cx));
        }

        if self.drag == Drag::None && self.busy.is_none() {
            layers.push(self.toolbar(cx));
        }

        if let Some(banner) = self.banner() {
            layers.push(
                div()
                    .absolute()
                    .top(px(18.0))
                    .left_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .bg(rgba(PANEL))
                            .border_1()
                            .border_color(rgba(PANEL_BORDER))
                            .text_sm()
                            .text_color(rgb(0xf4f4f5))
                            .child(banner),
                    )
                    .into_any_element(),
            );
        }

        let cursor = match (self.drag, self.selection) {
            (Drag::Move { .. }, _) => CursorStyle::ClosedHand,
            (Drag::Resize { handle, .. }, _) => handle.cursor(),
            (Drag::None, Some(selection)) => match handle_at(&selection, self.pointer) {
                Some(handle) => handle.cursor(),
                None if selection.contains(self.pointer) => CursorStyle::OpenHand,
                None => CursorStyle::Crosshair,
            },
            (Drag::None, None) if hovered.is_some() => CursorStyle::PointingHand,
            _ => CursorStyle::Crosshair,
        };

        div()
            .id("snip")
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .bg(rgb(0x000000))
            .cursor(cursor)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .children(layers)
    }
}

/// The first characters of `text` on one line, for a notification.
fn preview(text: &str, max_chars: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");

    if flat.chars().count() <= max_chars {
        return flat;
    }

    let cut: String = flat.chars().take(max_chars).collect();

    format!("{}\u{2026}", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(l: f32, t: f32, r: f32, b: f32) -> Rect {
        Rect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        }
    }

    fn screen_window(app: &str, title: &str, x: i32, y: i32, w: i32, h: i32) -> ScreenWindow {
        ScreenWindow {
            title: title.into(),
            app: app.into(),
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn a_drag_in_any_direction_makes_a_normal_rect() {
        assert_eq!(
            Rect::spanning((50.0, 40.0), (10.0, 90.0)),
            rect(10.0, 40.0, 50.0, 90.0)
        );
    }

    #[test]
    fn moving_stays_on_screen_and_keeps_the_size() {
        let r = rect(10.0, 10.0, 110.0, 60.0);

        assert_eq!(
            r.moved(-50.0, 0.0, (200.0, 100.0)),
            rect(0.0, 10.0, 100.0, 60.0)
        );
        assert_eq!(
            r.moved(500.0, 500.0, (200.0, 100.0)),
            rect(100.0, 50.0, 200.0, 100.0)
        );
        // A full-screen selection cannot move (and must not panic).
        let full = rect(0.0, 0.0, 200.0, 100.0);
        assert_eq!(full.moved(5.0, 5.0, (200.0, 100.0)), full);
    }

    #[test]
    fn handles_resize_their_edges_and_can_flip() {
        let r = rect(10.0, 10.0, 110.0, 60.0);
        let left = Handle::ALL[7];

        assert_eq!(
            Handle::BOTTOM_RIGHT.drag(&r, (150.0, 80.0)),
            rect(10.0, 10.0, 150.0, 80.0)
        );
        assert_eq!(left.drag(&r, (40.0, 999.0)), rect(40.0, 10.0, 110.0, 60.0));
        // Dragging the left edge past the right one flips the rect.
        assert_eq!(left.drag(&r, (130.0, 0.0)), rect(110.0, 10.0, 130.0, 60.0));
    }

    #[test]
    fn handles_are_found_near_corners_and_edge_middles() {
        let r = rect(10.0, 10.0, 110.0, 60.0);

        assert_eq!(handle_at(&r, (12.0, 8.0)), Some(Handle::ALL[0]));
        assert_eq!(handle_at(&r, (60.0, 61.0)), Some(Handle::ALL[6]));
        assert_eq!(handle_at(&r, (60.0, 35.0)), None);
    }

    #[test]
    fn one_display_shows_the_whole_picture() {
        let whole = Crop {
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
        };

        assert_eq!(
            crop_for_display((3840, 2160), &[(0.0, 0.0, 1920.0, 1080.0)], None),
            whole
        );
        assert_eq!(
            crop_for_display(
                (3840, 2160),
                &[(0.0, 0.0, 1920.0, 1080.0)],
                Some((0.0, 0.0, 1920.0, 1080.0))
            ),
            whole
        );
    }

    #[test]
    fn two_displays_crop_to_the_windows_one() {
        let displays = [(0.0, 0.0, 1920.0, 1080.0), (1920.0, 0.0, 1920.0, 1080.0)];

        assert_eq!(
            crop_for_display((3840, 1080), &displays, Some(displays[1])),
            Crop {
                x: 1920,
                y: 0,
                width: 1920,
                height: 1080
            }
        );
        assert_eq!(
            crop_for_display((7680, 2160), &displays, Some(displays[1])),
            Crop {
                x: 3840,
                y: 0,
                width: 3840,
                height: 2160
            }
        );
    }

    #[test]
    fn window_rects_map_to_picture_pixels() {
        let crop = Crop {
            x: 3840,
            y: 0,
            width: 3840,
            height: 2160,
        };

        assert_eq!(
            to_picture(&rect(10.0, 20.0, 110.0, 70.0), (1920.0, 1080.0), &crop),
            Crop {
                x: 3860,
                y: 40,
                width: 200,
                height: 100
            }
        );

        let edge = to_picture(
            &rect(1919.9, 1079.9, 1920.0, 1080.0),
            (1920.0, 1080.0),
            &crop,
        );
        assert!(edge.width >= 1 && edge.height >= 1);
        assert!(edge.x + edge.width <= crop.x + crop.width);
    }

    #[test]
    fn windows_become_overlay_targets_on_this_display_only() {
        let windows = [
            screen_window(
                "Ptyxis",
                "prod-cluster-admin \u{2014} zsh",
                2000,
                100,
                800,
                400,
            ),
            screen_window("Files", "Home", 100, 100, 500, 300),
            screen_window("Zed", "", 1900, 0, 300, 200),
        ];

        // This display is the right one of two 1920-wide monitors.
        let found = targets(&windows, (1920.0, 0.0), (1920.0, 1080.0), (1920.0, 1080.0));

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].rect, rect(80.0, 100.0, 880.0, 500.0));
        assert_eq!(
            found[0].label,
            "Ptyxis \u{2014} prod-cluster-admin \u{2014} zsh"
        );
        // Partly on this display: clipped.
        assert_eq!(found[1].rect, rect(0.0, 0.0, 280.0, 200.0));
        assert_eq!(found[1].label, "Zed");
    }

    fn monitor(x: f32, w: f32, h: f32, px: u32, pw: u32, ph: u32) -> ScreenMonitor {
        ScreenMonitor {
            x,
            y: 0.0,
            width: w,
            height: h,
            picture_x: px,
            picture_y: 0,
            picture_width: pw,
            picture_height: ph,
        }
    }

    #[test]
    fn the_overlay_finds_its_monitor_also_with_mixed_scales() {
        // A 4K laptop at 200 % (1920x1080 logical) and a 1440p monitor at 100 %.
        let monitors = [
            monitor(0.0, 1920.0, 1080.0, 0, 3840, 2160),
            monitor(1920.0, 2560.0, 1440.0, 3840, 2560, 1440),
        ];

        assert_eq!(
            pick_monitor(
                &monitors,
                Some((1920.0, 0.0, 2560.0, 1440.0)),
                (2560.0, 1440.0)
            ),
            Some(1)
        );
        // GPUI did not name the display: the size decides.
        assert_eq!(pick_monitor(&monitors, None, (1920.0, 1080.0)), Some(0));
        // Nothing matches and there are several: let the layout heuristic decide.
        assert_eq!(pick_monitor(&monitors, None, (800.0, 600.0)), None);
        // Linux: no monitors from the daemon.
        assert_eq!(pick_monitor(&[], None, (1920.0, 1080.0)), None);
    }

    #[test]
    fn the_topmost_window_under_the_pointer_wins() {
        let found = targets(
            &[
                screen_window("Top", "", 100, 100, 200, 200),
                screen_window("Below", "", 0, 0, 1000, 800),
            ],
            (0.0, 0.0),
            (1920.0, 1080.0),
            (1920.0, 1080.0),
        );

        assert_eq!(target_at(&found, (150.0, 150.0)), Some(0));
        assert_eq!(target_at(&found, (50.0, 50.0)), Some(1));
        assert_eq!(target_at(&found, (1500.0, 900.0)), None);
    }

    #[test]
    fn boxes_flip_to_stay_on_screen() {
        assert_eq!(
            place_beside((10.0, 10.0), (100.0, 100.0), (800.0, 600.0)),
            (34.0, 34.0)
        );
        assert_eq!(
            place_beside((790.0, 590.0), (100.0, 100.0), (800.0, 600.0)),
            (666.0, 466.0)
        );
    }

    #[test]
    fn the_bar_goes_below_then_above_then_inside() {
        let window = (800.0, 600.0);
        let bar = (400.0, 40.0);

        assert_eq!(
            place_bar(&rect(100.0, 100.0, 600.0, 300.0), bar, window).1,
            310.0
        );
        assert_eq!(
            place_bar(&rect(100.0, 100.0, 600.0, 590.0), bar, window).1,
            50.0
        );
        assert_eq!(
            place_bar(&rect(0.0, 0.0, 800.0, 600.0), bar, window).1,
            550.0
        );
        assert_eq!(place_bar(&rect(0.0, 0.0, 100.0, 100.0), bar, window).0, 0.0);
    }

    #[test]
    fn previews_are_short() {
        assert_eq!(preview("a\n b", 10), "a b");
        assert_eq!(preview("abcdefghij", 4), "abcd\u{2026}");
    }
}
