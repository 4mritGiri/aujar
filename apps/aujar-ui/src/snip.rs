//! Aujar Snip: Aujar's own snipping overlay.
//!
//! The daemon takes a picture of the whole screen without asking anything (`CaptureScreen`),
//! and this window shows it frozen, full screen. You drag an area (with a pixel magnifier, the
//! size in pixels, handles to resize and arrow keys to nudge), then:
//!
//! | Key | Action |
//! |---|---|
//! | Enter, Ctrl+C, double-click | copy the picture |
//! | T | copy the text in it (Tesseract, on this computer) |
//! | Ctrl+S | save a PNG (`~/Pictures/Screenshots` by default) |
//! | Ctrl+A | select the whole screen |
//! | Arrows (Shift: 10 px), Ctrl+Arrows | move, resize |
//! | Right-click, Esc | clear the selection, close |
//!
//! In text mode (`aujar text`) the text of the area is copied as soon as you let go.
//! The screen picture lives in memory only; its file is deleted as soon as it is read.

use crate::{UiMessage, color_picker::send_to_ui};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, FocusHandle, Focusable,
    ImageFormat, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Pixels, RenderImage, StyledImage, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowHandle, WindowOptions, div, img, prelude::*, px, rgb, rgba, size,
};
use image::RgbaImage;
use std::{path::PathBuf, sync::Arc, time::Duration};

/// Longest wait for the daemon's picture of the screen.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
/// Half the side of the magnifier, in screen pixels (the magnifier shows 2n+1 squared).
const LOUPE_RADIUS: i64 = 6;
/// Side of one magnified pixel, in window pixels.
const LOUPE_CELL: f32 = 9.0;
/// How close (window pixels) the pointer must be to a handle to grab it.
const HANDLE_REACH: f32 = 10.0;
/// A drag shorter than this is a click, not a selection.
const MIN_DRAG: f32 = 4.0;

const ACCENT: u32 = 0x4c8dff;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Select, then choose: copy, text, save.
    Picture,
    /// The text of the selected area is copied when the mouse is let go.
    Text,
}

/// A picture of the whole screen, ready for the overlay.
pub(crate) struct Shot {
    pub mode: Mode,
    pub pixels: Arc<RgbaImage>,
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
        let dx = dx.clamp(-self.left, w - self.right);
        let dy = dy.clamp(-self.top, h - self.bottom);

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
    pub const ALL: [Handle; 8] = [
        Handle::at(true, true, false, false),
        Handle::at(false, true, false, false),
        Handle::at(false, true, true, false),
        Handle::at(false, false, true, false),
        Handle::at(false, false, true, true),
        Handle::at(false, false, false, true),
        Handle::at(true, false, false, true),
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
            .spawn(async move { capture().map(|pixels| Shot { mode, pixels }) })
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

fn capture() -> Result<Arc<RgbaImage>, String> {
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

    let path = match answer {
        Err(_) => return Err("The screen capture took too long.".to_owned()),
        Ok(Err(_)) => {
            return Err(
                "The Aujar daemon is not running. Start it with `systemctl --user start aujar`."
                    .to_owned(),
            );
        }
        Ok(Ok(aujar_ipc::Response::Screen { path })) => PathBuf::from(path),
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
        .map(|picture| Arc::new(picture.to_rgba8()))
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

    cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| SnipView::new(shot, cx));
        let focus = view.read(cx).focus_handle.clone();

        window.focus(&focus, cx);

        view
    })
    .ok()
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

pub(crate) struct SnipView {
    mode: Mode,
    shot: Arc<RgbaImage>,
    focus_handle: FocusHandle,
    /// The monitor's part of the screen picture and its image (made on the first frame, once
    /// the window knows its display).
    view: Option<(Crop, Arc<RenderImage>)>,
    pointer: (f32, f32),
    selection: Option<Rect>,
    drag: Drag,
    /// Recognising text: the overlay waits and ignores input.
    busy: bool,
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

    let Ok(time) = libc::time_t::try_from(secs) else {
        return secs.to_string();
    };

    // SAFETY: `localtime_r` only reads `time` and fills the zeroed `tm` we pass.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();

        if libc::localtime_r(&time, &mut tm).is_null() {
            return secs.to_string();
        }

        tm
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

impl SnipView {
    fn new(shot: Shot, cx: &mut Context<Self>) -> Self {
        Self {
            mode: shot.mode,
            shot: shot.pixels,
            focus_handle: cx.focus_handle(),
            view: None,
            pointer: (-1000.0, -1000.0),
            selection: None,
            drag: Drag::None,
            busy: false,
            message: None,
        }
    }

    /// Work out which part of the screen picture this window shows.
    fn ensure_view(&mut self, window: &Window, cx: &App) {
        if self.view.is_some() {
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
        let viewport = window.viewport_size();
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));

        // The window's display: the one GPUI names, else the only one this size.
        let named = window.display(cx).map(|display| rect(display.bounds()));
        let sized: Vec<_> = displays
            .iter()
            .copied()
            .filter(|d| (d.2 - vw).abs() < 2.0 && (d.3 - vh).abs() < 2.0)
            .collect();
        let display = named.or_else(|| (sized.len() == 1).then(|| sized[0]));

        let crop = crop_for_display((self.shot.width(), self.shot.height()), &displays, display);

        // GPUI draws BGRA frames.
        let mut frame =
            image::imageops::crop_imm(self.shot.as_ref(), crop.x, crop.y, crop.width, crop.height)
                .to_image();

        for pixel in frame.pixels_mut() {
            pixel.0.swap(0, 2);
        }

        let render = RenderImage::new([image::Frame::new(frame)]);

        self.view = Some((crop, Arc::new(render)));
    }

    fn window_size(window: &Window) -> (f32, f32) {
        let viewport = window.viewport_size();

        (f32::from(viewport.width), f32::from(viewport.height))
    }

    fn close(&mut self, window: &mut Window) {
        window.remove_window();
    }

    /// The selection as picture pixels.
    fn selected_picture(&self, window: &Window) -> Option<RgbaImage> {
        let selection = self.selection?;
        let (crop, _) = self.view.as_ref()?;
        let area = to_picture(&selection, Self::window_size(window), crop);

        Some(
            image::imageops::crop_imm(self.shot.as_ref(), area.x, area.y, area.width, area.height)
                .to_image(),
        )
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

        self.busy = true;
        self.message = None;
        cx.notify();

        let settings = aujar_config::Settings::load_or_default().text_extractor;
        let options = aujar_text_extractor::Options {
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
                this.busy = false;

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

    // ---- Input ----

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }

        let at = (f32::from(event.position.x), f32::from(event.position.y));
        self.message = None;

        if event.button == MouseButton::Right {
            if self.selection.take().is_none() {
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
                self.copy_picture(window, cx);
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
            self.selection = Some(Rect::spanning(origin, origin));
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
        self.selection = match self.drag {
            Drag::None => self.selection,
            Drag::New(origin) => Some(Rect::spanning(origin, at).clamped(bounds)),
            Drag::Move { from, original } => {
                Some(original.moved(at.0 - from.0, at.1 - from.1, bounds))
            }
            Drag::Resize { handle, original } => Some(handle.drag(&original, at).clamped(bounds)),
        };

        cx.notify();
    }

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let finished_new = matches!(self.drag, Drag::New(_));

        self.drag = Drag::None;

        // A click is not a selection.
        if self
            .selection
            .is_some_and(|s| s.width() < MIN_DRAG || s.height() < MIN_DRAG)
        {
            self.selection = None;
        }

        if finished_new && self.mode == Mode::Text && self.selection.is_some() {
            self.copy_text(window, cx);
        }

        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();

        if self.busy {
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
            "enter" => self.copy_picture(window, cx),
            "c" if ctrl => self.copy_picture(window, cx),
            "s" if ctrl => self.save(window, cx),
            "t" if !ctrl => self.copy_text(window, cx),
            "a" if ctrl => {
                self.selection = Some(Rect::spanning((0.0, 0.0), bounds));
                cx.notify();
            }
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

                self.selection = Some(if ctrl {
                    // Ctrl: grow or shrink from the bottom-right corner.
                    let corner = (selection.right + dx, selection.bottom + dy);
                    let resized = Handle::ALL[4].drag(&selection, corner).clamped(bounds);

                    if resized.width() >= 1.0 && resized.height() >= 1.0 {
                        resized
                    } else {
                        selection
                    }
                } else {
                    selection.moved(dx, dy, bounds)
                });
                cx.notify();
            }
            _ => {}
        }
    }

    // ---- Drawing ----

    fn loupe(&self, window: &Window) -> Option<AnyElement> {
        let (crop, _) = self.view.as_ref()?;
        let bounds = Self::window_size(window);

        if !Rect::spanning((0.0, 0.0), bounds).contains(self.pointer) {
            return None;
        }

        let center = to_picture(&Rect::spanning(self.pointer, self.pointer), bounds, crop);
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

        let size_line = self
            .selection
            .filter(|_| self.drag != Drag::None)
            .map(|selection| {
                let area = to_picture(&selection, bounds, crop);

                format!("{} \u{d7} {}", area.width, area.height)
            });
        let info = div()
            .flex()
            .flex_col()
            .px_2()
            .py_1()
            .text_xs()
            .text_color(rgb(0xffffff))
            .child(format!(
                "{}, {}   {hex}",
                center.x - crop.x,
                center.y - crop.y
            ))
            .children(size_line);

        let (x, y) = place_beside(self.pointer, (side, side + 40.0), bounds);

        Some(
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .rounded_md()
                .overflow_hidden()
                .border_1()
                .border_color(rgb(0xffffff))
                .bg(rgba(0x000000e0))
                .shadow_lg()
                .child(div().flex().flex_col().children(rows))
                .child(info)
                .into_any_element(),
        )
    }

    fn actions_bar(&self, selection: &Rect, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let button = |id: &'static str, label: &'static str, key: &'static str, primary: bool| {
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor(CursorStyle::PointingHand)
                .text_sm()
                .text_color(rgb(0xffffff))
                .when(primary, |b| b.bg(rgb(ACCENT)))
                .when(!primary, |b| b.hover(|b| b.bg(rgba(0xffffff26))))
                .child(label)
                .child(div().text_xs().opacity(0.7).child(key))
                // Clicking a button must not start a new selection.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        };

        let bar_size = (420.0, 40.0);
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
            .bg(rgba(0x1c1c1ef2))
            .border_1()
            .border_color(rgba(0xffffff33))
            .shadow_lg()
            .child(
                button("snip-copy", "Copy", "\u{23ce}", true)
                    .on_click(cx.listener(|this, _, window, cx| this.copy_picture(window, cx))),
            )
            .child(
                button("snip-text", "Copy text", "T", false)
                    .on_click(cx.listener(|this, _, window, cx| this.copy_text(window, cx))),
            )
            .child(
                button("snip-save", "Save", "Ctrl+S", false)
                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
            )
            .child(
                button("snip-close", "\u{2715}", "Esc", false)
                    .on_click(cx.listener(|this, _, window, _| this.close(window))),
            )
            .into_any_element()
    }
}

impl Render for SnipView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_view(window, cx);

        let bounds = Self::window_size(window);
        let dim = rgba(0x00000080);
        let picture = self.view.as_ref().map(|(_, render)| render.clone());

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

        match self.selection {
            Some(s) => {
                // Dim everything but the selection.
                layers.push(shade(0.0, 0.0, bounds.0, s.top));
                layers.push(shade(0.0, s.bottom, bounds.0, bounds.1 - s.bottom));
                layers.push(shade(0.0, s.top, s.left, s.height()));
                layers.push(shade(s.right, s.top, bounds.0 - s.right, s.height()));

                layers.push(
                    div()
                        .absolute()
                        .left(px(s.left - 1.0))
                        .top(px(s.top - 1.0))
                        .w(px(s.width() + 2.0))
                        .h(px(s.height() + 2.0))
                        .border_1()
                        .border_color(rgb(ACCENT))
                        .into_any_element(),
                );

                if self.drag == Drag::None {
                    for handle in Handle::ALL {
                        let (x, y) = handle.position(&s);

                        layers.push(
                            div()
                                .absolute()
                                .left(px(x - 4.0))
                                .top(px(y - 4.0))
                                .size(px(8.0))
                                .rounded_sm()
                                .bg(rgb(0xffffff))
                                .border_1()
                                .border_color(rgb(ACCENT))
                                .into_any_element(),
                        );
                    }
                }

                // The size, above the top-left corner (inside it at the top of the screen).
                if let Some((crop, _)) = &self.view {
                    let area = to_picture(&s, bounds, crop);
                    let label_y = if s.top >= 26.0 {
                        s.top - 24.0
                    } else {
                        s.top + 4.0
                    };

                    layers.push(
                        div()
                            .absolute()
                            .left(px(s.left))
                            .top(px(label_y))
                            .px_2()
                            .py_0p5()
                            .rounded_sm()
                            .bg(rgba(0x000000c0))
                            .text_xs()
                            .text_color(rgb(0xffffff))
                            .child(format!("{} \u{d7} {}", area.width, area.height))
                            .into_any_element(),
                    );
                }
            }
            None => {
                layers.push(shade(0.0, 0.0, bounds.0, bounds.1));

                // Crosshair guides through the pointer.
                let (x, y) = self.pointer;
                let guide = rgba(0xffffff59);

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

        let selecting =
            self.selection.is_none() || matches!(self.drag, Drag::New(_) | Drag::Resize { .. });

        if selecting && !self.busy {
            layers.extend(self.loupe(window));
        }

        if let (Some(selection), Drag::None, false, Mode::Picture) =
            (self.selection, self.drag, self.busy, self.mode)
        {
            layers.push(self.actions_bar(&selection, window, cx));
        }

        // What to do, at the top; or what happened.
        let banner = if self.busy {
            Some("Reading text\u{2026}".to_owned())
        } else if let Some(message) = &self.message {
            Some(message.clone())
        } else if self.selection.is_none() {
            Some(match self.mode {
                Mode::Picture => {
                    "Drag to select an area  \u{b7}  Ctrl+A whole screen  \u{b7}  Esc cancels"
                        .to_owned()
                }
                Mode::Text => "Drag over text to copy it  \u{b7}  Esc cancels".to_owned(),
            })
        } else {
            None
        };

        if let Some(banner) = banner {
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
                            .bg(rgba(0x1c1c1ef2))
                            .border_1()
                            .border_color(rgba(0xffffff33))
                            .text_sm()
                            .text_color(rgb(0xffffff))
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
    }

    #[test]
    fn handles_resize_their_edges_and_can_flip() {
        let r = rect(10.0, 10.0, 110.0, 60.0);
        let bottom_right = Handle::ALL[4];
        let left = Handle::ALL[7];

        assert_eq!(
            bottom_right.drag(&r, (150.0, 80.0)),
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
        assert_eq!(handle_at(&r, (60.0, 61.0)), Some(Handle::ALL[5]));
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
        // Two 1920x1080 monitors side by side, captured at scale 1.
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

        // The same at 200 %: the picture has twice the pixels.
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

        // 200 %: a 100x50 logical rect is 200x100 picture pixels.
        assert_eq!(
            to_picture(&rect(10.0, 20.0, 110.0, 70.0), (1920.0, 1080.0), &crop),
            Crop {
                x: 3860,
                y: 40,
                width: 200,
                height: 100
            }
        );

        // Never empty, never past the edge.
        let edge = to_picture(
            &rect(1919.9, 1079.9, 1920.0, 1080.0),
            (1920.0, 1080.0),
            &crop,
        );
        assert!(edge.width >= 1 && edge.height >= 1);
        assert!(edge.x + edge.width <= crop.x + crop.width);
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
        // Right-aligned with the selection, but kept on screen.
        assert_eq!(place_bar(&rect(0.0, 0.0, 100.0, 100.0), bar, window).0, 0.0);
    }

    #[test]
    fn previews_are_short() {
        assert_eq!(preview("a\n b", 10), "a b");
        assert_eq!(preview("abcdefghij", 4), "abcd\u{2026}");
    }
}
