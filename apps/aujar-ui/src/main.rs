//! Aujar launcher UI (GPUI): a Spotlight-style search window with PowerToys-style
//! plugins, and a settings window.
//!
//! Design: the UI holds **no search logic**. A background thread (tokio) talks to the
//! daemon over IPC; the GPUI thread only renders state and forwards keystrokes.
//!
//! Keys: type to search, Left/Right/Home/End to edit, Up/Down/Tab to select, Enter to
//! launch, Ctrl+Enter to show a file in its folder, Ctrl+C to copy, Ctrl+L to look a word up,
//! Ctrl+K (or right-click) for the actions menu, Alt+1..9 to launch a row, Esc to clear/close.
//! Files can be dragged out of the window into other apps (Wayland). Limitation: plain key
//! events only (no IME / composition yet).

mod assets;
mod model;
mod settings_view;
mod theme;

use anyhow::{Result, bail};
use aujar_config::{History, LauncherPosition, Settings};
use aujar_ipc::{
    Event, ExecuteRequest, LauncherCommand, Request, Response, SearchHit, SearchRequest, send,
    subscribe,
};
use gpui::{
    AnyElement, AnyView, App, Bounds, ClipboardItem, Context, ExternalDragPayload, FileDragPaths,
    FocusHandle, Focusable, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    ScrollHandle, SharedString, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowHandle, WindowOptions, div, img, point, prelude::*, px, rgb, rgba,
    size, svg,
};
use gpui_platform::application;
use model::{
    Action, MenuItem, SECTION_COUNT, Section, define_query, definition_parts, has_loading,
    icon_for_kind, is_card, is_dangerous, is_inert, kind_meta, menu_items, price_direction,
    scoreboard, section_of, section_title, stock_rows, tile_color, weather_parts,
};
use settings_view::SettingsView;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use theme::{Palette, rounded_px};

/// Window height; the card inside is content-sized and the rest is transparent.
const WINDOW_H: f32 = 640.0;
/// Wait before sending a search, so a burst of keystrokes becomes one request.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(30);
/// Wait before asking again about a lookup that is still loading.
const POLL_DELAY: Duration = Duration::from_millis(450);
/// Re-asks per search (about 20 s), after which the placeholder stays.
const MAX_POLLS: u32 = 44;
/// Tallest the results list gets before it scrolls.
/// Height of the search row (the pill and the round buttons), and the gap below it.
const ROW_H: f32 = 52.0;
const ROW_GAP: f32 = 8.0;

/// Height of the results card: tall when it lists something or shows the tips, short for a
/// notice or a few examples. The window is resized to fit exactly, so no empty strip of window
/// (and the shadow the compositor draws around it) is left below the card.
const CARD_FULL_H: f32 = 500.0;
const CARD_SHORT_H: f32 = 150.0;

/// Bridge thread -> UI thread.
enum UiMessage {
    Launcher(LauncherCommand),
    Results {
        query: String,
        results: Vec<SearchHit>,
        /// 0 for a fresh search; counts the re-asks made while a lookup is loading.
        attempt: u32,
    },
    Failed(String),
    OpenSettings,
    /// New text on the system clipboard (from the watcher thread).
    Clipboard(String),
    /// "Clear history" in the clipboard view.
    ClearClips,
    /// Recently used files (file history) for the empty launcher.
    RecentFiles(Vec<SearchHit>),
    /// A PDF thumbnail finished rendering; redraw the preview.
    ThumbReady,
}

/// UI thread -> bridge thread.
enum UiRequest {
    /// `attempt` > 0 is a re-ask for a lookup that was still loading: it waits a moment first.
    Search {
        query: String,
        attempt: u32,
    },
    Execute(String),
    OpenSettings,
    /// Ask the daemon for the file history (`kind:recent`).
    Recents,
    /// Render the first page of a PDF into the thumbnail cache.
    Thumbnail(PathBuf),
}

// ---- Web helpers ------------------------------------------------------------------------

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());

    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }

    out
}

/// `github.com/rust-lang`, `https://example.org/x` -> a full URL; anything else -> `None`.
fn url_from_query(query: &str) -> Option<String> {
    let query = query.trim();

    if query.contains(char::is_whitespace) || query.len() < 4 {
        return None;
    }

    if query.starts_with("http://") || query.starts_with("https://") {
        return Some(query.to_owned());
    }

    let host = query.split('/').next()?;
    let (name, tld) = host.rsplit_once('.')?;

    let looks_like_host = !name.is_empty()
        && tld.len() >= 2
        && tld.chars().all(|c| c.is_ascii_alphabetic())
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');

    looks_like_host.then(|| format!("https://{query}"))
}

fn is_web_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

fn web_hit(title: String, subtitle: String, url: String) -> SearchHit {
    SearchHit {
        id: format!("url:{url}"),
        title,
        subtitle: Some(subtitle),
        kind: "web".to_owned(),
        score: 0,
        icon: None,
        detail: None,
    }
}

// ---- Preview ----------------------------------------------------------------------------

/// Details shown in the right-hand panel for the selected result.
struct Preview {
    title: String,
    /// What it is, under the title: "PDF document", "Folder", "Application", ...
    badge: String,
    rows: Vec<(&'static str, String)>,
    /// A few names from inside a folder.
    entries: Vec<String>,
    /// First lines of a text file, or the full text of a clipboard entry.
    text: Option<String>,
    image: Option<PathBuf>,
    /// A PDF whose thumbnail is not cached yet; the view asks the bridge to render it.
    thumb_request: Option<PathBuf>,
}

/// What Enter does for a result, in words (NEPSE results are not stock quotes to open).
fn action_label_for(hit: &SearchHit) -> &'static str {
    if hit.id.starts_with("nepse:alert:") {
        "Set alert"
    } else if hit.id.starts_with("nepse:unalert:") {
        "Remove alert"
    } else if hit.id.starts_with("nepse:") {
        "Open NEPSE site"
    } else {
        kind_meta(&hit.kind).1
    }
}

fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("aujar")
}

fn modified_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Cache file stem for a PDF thumbnail; changes when the file is modified.
fn thumb_prefix(path: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    path.hash(&mut hasher);
    modified_secs(path).hash(&mut hasher);

    cache_dir()
        .join("thumbs")
        .join(format!("{:016x}", hasher.finish()))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// Render page 1 of a PDF to `<cache>/thumbs/<hash>.png` with poppler's `pdftoppm`.
/// Returns whether a thumbnail exists afterwards.
fn generate_pdf_thumb(path: &Path) -> bool {
    let prefix = thumb_prefix(path);
    let png = prefix.with_extension("png");

    if png.is_file() {
        return true;
    }

    if let Some(parent) = prefix.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let status = Command::new("pdftoppm")
        .args([
            "-png",
            "-f",
            "1",
            "-l",
            "1",
            "-singlefile",
            "-scale-to",
            "560",
        ])
        .arg(path)
        .arg(&prefix)
        .status();

    matches!(status, Ok(status) if status.success()) && png.is_file()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

    let mut value = bytes as f64;
    let mut unit = 0;

    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn age_text(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();

    match seconds {
        0..=59 => "just now".to_owned(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

fn first_chars(text: &str, limit: usize) -> String {
    let mut out: String = text.chars().take(limit).collect();

    if text.chars().count() > limit {
        out.push('\u{2026}');
    }

    out
}

const TEXT_EXTENSIONS: [&str; 24] = [
    "txt", "md", "rs", "py", "js", "ts", "json", "toml", "yaml", "yml", "sh", "c", "h", "cpp",
    "css", "html", "csv", "log", "conf", "ini", "xml", "java", "go", "sql",
];
const IMAGE_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// First lines of a small text file; `None` for binary or unreadable files.
fn read_text_preview(path: &Path) -> Option<String> {
    use std::io::Read;

    let mut buffer = Vec::new();

    fs::File::open(path)
        .ok()?
        .take(4096)
        .read_to_end(&mut buffer)
        .ok()?;

    // A cut multi-byte character at the 4 KiB boundary is fine; NUL bytes mean binary.
    if buffer.contains(&0) {
        return None;
    }

    let text = String::from_utf8_lossy(&buffer);
    let lines: Vec<String> = text
        .lines()
        .take(14)
        .map(|line| first_chars(line, 70))
        .collect();

    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// A short, human description of a file by its extension (Spotlight's "Kind").
fn file_kind(path: &Path, is_dir: bool) -> String {
    if is_dir {
        return "Folder".to_owned();
    }

    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tif" | "tiff" | "avif" => {
            format!("{} image", extension.to_uppercase())
        }
        "pdf" => "PDF document".to_owned(),
        "md" | "markdown" => "Markdown document".to_owned(),
        "txt" | "log" => "Plain text".to_owned(),
        "doc" | "docx" | "odt" | "rtf" | "" => "Document".to_owned(),
        "xls" | "xlsx" | "ods" | "csv" => "Spreadsheet".to_owned(),
        "ppt" | "pptx" | "odp" => "Presentation".to_owned(),
        "zip" | "tar" | "gz" | "xz" | "bz2" | "zst" | "7z" | "rar" => "Archive".to_owned(),
        "mp3" | "flac" | "ogg" | "wav" | "m4a" | "opus" => "Audio".to_owned(),
        "mp4" | "mkv" | "webm" | "mov" | "avi" => "Video".to_owned(),
        "rs" | "py" | "js" | "ts" | "c" | "h" | "cpp" | "java" | "go" | "sh" | "css" | "html"
        | "sql" | "toml" | "yaml" | "yml" | "json" | "xml" => {
            format!("{} source", extension.to_uppercase())
        }
        other => format!("{} file", other.to_uppercase()),
    }
}

/// `~/Downloads` for `/home/me/Downloads`.
fn pretty_folder(dir: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);

    pretty_folder_in(dir, home.as_deref())
}

fn pretty_folder_in(dir: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| dir.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    }
}

/// `meta:<size>:<modified>` from the daemon: a file's size in bytes and modification time.
fn parse_file_meta(detail: &str) -> Option<(u64, u64)> {
    let (size, modified) = detail.strip_prefix("meta:")?.split_once(':')?;

    Some((size.parse().ok()?, modified.parse().ok()?))
}

/// Word the daemon's file details for the row: "PDF document \u{b7} 57 KB \u{b7} yesterday 9:05 PM
/// \u{b7} ~/Downloads". Results from a daemon that sends no details stay as they are.
fn describe_files(hits: &mut [SearchHit]) {
    let now = now_secs();
    let now_stamp = local_stamp(now);

    for hit in hits
        .iter_mut()
        .filter(|hit| matches!(hit.kind.as_str(), "file" | "folder"))
    {
        let Some((size, modified)) = hit.detail.as_deref().and_then(parse_file_meta) else {
            continue;
        };

        let is_dir = hit.kind == "folder";
        let mut parts = vec![file_kind(Path::new(&hit.title), is_dir)];

        if !is_dir {
            parts.push(human_size(size));
        }

        if modified > 0 {
            let age = Duration::from_secs(now.saturating_sub(modified));

            parts.push(copied_text(now_stamp, local_stamp(modified), age));
        }

        if let Some(folder) = hit.subtitle.take().filter(|folder| !folder.is_empty()) {
            parts.push(folder);
        }

        hit.subtitle = Some(parts.join(" \u{b7} "));
        hit.detail = None;
    }
}

/// One clipboard history entry.
#[derive(Debug, Clone)]
struct ClipEntry {
    text: String,
    /// Unix time in seconds.
    copied_at: u64,
}

fn now_secs() -> u64 {
    now_ms() / 1000
}

/// A moment on the local wall clock, for "Copied 4:19 PM".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    year: i32,
    /// Day of the year, 0 to 365.
    yday: i32,
    /// 0 is Sunday.
    weekday: usize,
    hour: u32,
    minute: u32,
}

/// The local time of a Unix time, or `None` when the C library cannot say.
fn local_stamp(secs: u64) -> Option<Stamp> {
    let time = libc::time_t::try_from(secs).ok()?;

    // SAFETY: `localtime_r` only reads `time` and fills in the `tm` we give it; an all-zero
    // `tm` is a valid value.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();

        if libc::localtime_r(&time, &mut tm).is_null() {
            return None;
        }

        tm
    };

    Some(Stamp {
        year: tm.tm_year + 1900,
        yday: tm.tm_yday,
        weekday: usize::try_from(tm.tm_wday).ok()? % 7,
        hour: u32::try_from(tm.tm_hour).ok()?,
        minute: u32::try_from(tm.tm_min).ok()?,
    })
}

/// `4:19 PM`.
fn clock_text(hour: u32, minute: u32) -> String {
    let hour12 = match hour % 12 {
        0 => 12,
        other => other,
    };

    format!(
        "{hour12}:{minute:02} {}",
        if hour < 12 { "AM" } else { "PM" }
    )
}

/// When something was copied, as a person says it: `4:19 PM` today, `yesterday 4:19 PM`,
/// `Mon 4:19 PM` this week, `3 days ago` after that. Falls back to the age when the local
/// time is unknown.
fn copied_text(now: Option<Stamp>, then: Option<Stamp>, age: Duration) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

    if age.as_secs() < 60 {
        return "just now".to_owned();
    }

    let (Some(now), Some(then)) = (now, then) else {
        return age_text(age);
    };

    let clock = clock_text(then.hour, then.minute);
    let same_year = now.year == then.year;

    if same_year && now.yday == then.yday {
        clock
    } else if (same_year && now.yday == then.yday + 1)
        || (now.year == then.year + 1 && now.yday == 0 && then.yday >= 364)
    {
        format!("yesterday {clock}")
    } else if age.as_secs() < 6 * 86_400 {
        format!("{} {clock}", WEEKDAYS[then.weekday % 7])
    } else {
        age_text(age)
    }
}

/// `en.wikipedia.org` from `https://en.wikipedia.org/wiki/X`.
fn url_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host: String = rest
        .chars()
        .take_while(|c| !matches!(c, '/' | '?' | '#'))
        .collect();

    (!host.is_empty()).then_some(host)
}

fn build_preview(hit: &SearchHit, clips: &[ClipEntry]) -> Preview {
    let (badge, _) = kind_meta(&hit.kind);

    let mut preview = Preview {
        title: hit.title.clone(),
        badge: badge.to_owned(),
        rows: Vec::new(),
        entries: Vec::new(),
        text: None,
        image: None,
        thumb_request: None,
    };

    match hit.kind.as_str() {
        "file" | "folder" => {
            let Some(path) = hit.id.strip_prefix("files:open:").map(PathBuf::from) else {
                return preview;
            };

            if let Ok(meta) = fs::metadata(&path) {
                preview.badge = file_kind(&path, meta.is_dir());

                if meta.is_dir() {
                    if let Ok(entries) = fs::read_dir(&path) {
                        // Bounded: a huge directory must not stall the UI thread.
                        let mut count = 0usize;
                        let mut names: Vec<String> = Vec::new();

                        for entry in entries.take(5001).flatten() {
                            count += 1;

                            let name = entry.file_name().to_string_lossy().into_owned();

                            if names.len() < 60 && !name.starts_with('.') {
                                names.push(name);
                            }
                        }

                        names.sort_by_key(|name| name.to_lowercase());
                        names.truncate(8);
                        preview.entries = names;

                        preview.rows.push((
                            "Contains",
                            if count > 5000 {
                                "5,000+ items".to_owned()
                            } else {
                                format!("{count} items")
                            },
                        ));
                    }
                } else {
                    preview.rows.push(("Size", human_size(meta.len())));
                }

                let since = |time: std::io::Result<SystemTime>| {
                    time.and_then(|time| {
                        SystemTime::now()
                            .duration_since(time)
                            .map_err(std::io::Error::other)
                    })
                    .ok()
                };

                if let Some(elapsed) = since(meta.modified()) {
                    preview.rows.push(("Modified", age_text(elapsed)));
                }

                if let Some(elapsed) = since(meta.created()) {
                    preview.rows.push(("Created", age_text(elapsed)));
                }

                if meta.is_file() {
                    let extension = path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(str::to_ascii_lowercase)
                        .unwrap_or_default();

                    if extension == "pdf" {
                        let png = thumb_prefix(&path).with_extension("png");

                        if png.is_file() {
                            preview.image = Some(png);
                        } else if on_path("pdftoppm") {
                            preview.thumb_request = Some(path.clone());
                        }
                    } else if IMAGE_EXTENSIONS.contains(&extension.as_str())
                        && meta.len() < 15_000_000
                    {
                        preview.image = Some(path.clone());
                    } else if TEXT_EXTENSIONS.contains(&extension.as_str())
                        && meta.len() < 1_000_000
                    {
                        preview.text = read_text_preview(&path);
                    }
                }
            }

            // The row's subtitle now says kind, size and date; the location is the folder.
            let folder = path
                .parent()
                .map(pretty_folder)
                .filter(|folder| !folder.is_empty());

            if let Some(parent) = folder.or_else(|| hit.subtitle.clone()) {
                preview.rows.push(("Location", parent));
            }
        }
        "clip" => {
            let clip = hit
                .id
                .strip_prefix("clip:")
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| clips.get(index));

            if let Some(clip) = clip {
                preview.rows.push((
                    "Length",
                    format!("{} characters", clip.text.chars().count()),
                ));
                preview.text = Some(first_chars(&clip.text, 700));
            }
        }
        "calculation" => {
            if let Some(expression) = &hit.subtitle {
                preview
                    .rows
                    .push(("Input", expression.trim_start_matches("= ").to_owned()));
            }

            preview.rows.push(("Result", hit.title.clone()));
        }
        "web" => {
            if let Some(url) = hit.id.strip_prefix("url:") {
                preview.rows.push(("Address", url.to_owned()));
            }
        }
        // Drawn from `hit.detail` by `render_preview`.
        "definition" | "weather" | "stock" | "sports" => {}
        _ => {
            if let Some(subtitle) = &hit.subtitle {
                preview.rows.push(("About", subtitle.clone()));
            }
        }
    }

    preview
}

// ---- Drag and drop out -------------------------------------------------------------------

/// A file result being dragged. Dropping it on another app (file manager, chat, mail) copies
/// or moves the real file; GPUI offers it to the compositor when the pointer leaves the window
/// (Wayland; elsewhere the drag stays inside the window and does nothing).
#[derive(Clone)]
struct DraggedFile {
    path: PathBuf,
    is_dir: bool,
    title: String,
}

/// What follows the pointer while dragging inside the window.
struct DragGhost {
    title: String,
    accent: u32,
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .rounded_lg()
            .bg(rgb(self.accent))
            .text_color(rgb(0xffffff))
            .text_sm()
            .child(self.title.clone())
    }
}

/// Set while a file drag has been handed to the compositor: losing focus is expected then,
/// and must not close the window (that would cancel the drag).
static DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Bumped each time the launcher opens, so a stale auto-close timer leaves a newer window alone.
static LAUNCHER_GENERATION: AtomicU64 = AtomicU64::new(0);
static UI_SENDER: OnceLock<async_channel::Sender<UiMessage>> = OnceLock::new();
/// True while a launcher window exists (the clipboard watcher pauses then: `wl-paste` can steal
/// focus on GNOME/Wayland, which made the launcher close itself right after opening).
static LAUNCHER_ALIVE: AtomicBool = AtomicBool::new(false);
/// Until this time (ms since the epoch) focus loss is expected and must not close the launcher:
/// set while the user drags the launcher around, and just after a background clipboard read.
static BLUR_IGNORE_UNTIL: AtomicU64 = AtomicU64::new(0);

/// Diagnostics on stderr when `AUJAR_DEBUG` is set (run `AUJAR_DEBUG=1 aujar-ui`).
fn debug(message: &str) {
    if std::env::var_os("AUJAR_DEBUG").is_some() {
        eprintln!("aujar-ui[{}ms]: {message}", now_ms() % 100_000);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ignore_blur_for(duration: Duration) {
    BLUR_IGNORE_UNTIL.fetch_max(now_ms() + duration.as_millis() as u64, Ordering::SeqCst);
}

/// Start moving the launcher with the compositor. The compositor grabs the pointer, so the
/// window briefly loses focus: that must not count as a click-away.
fn begin_window_move(window: &mut Window) {
    ignore_blur_for(Duration::from_secs(5));
    window.start_window_move();
}

/// How long the window stays alive after a drag leaves it, so the receiving app can read the
/// file list. Then it closes by itself.
const DRAG_LINGER: Duration = Duration::from_secs(30);

fn begin_external_drag() {
    DRAG_ACTIVE.store(true, Ordering::SeqCst);

    let generation = LAUNCHER_GENERATION.load(Ordering::SeqCst);

    std::thread::spawn(move || {
        std::thread::sleep(DRAG_LINGER);

        DRAG_ACTIVE.store(false, Ordering::SeqCst);

        if LAUNCHER_GENERATION.load(Ordering::SeqCst) == generation {
            if let Some(sender) = UI_SENDER.get() {
                let _ = sender.send_blocking(UiMessage::Launcher(LauncherCommand::Hide));
            }
        }
    });
}

// ---- Context menu -------------------------------------------------------------------------

struct ContextMenu {
    items: Vec<MenuItem>,
    selected: usize,
    /// Top-left corner in window coordinates.
    x: f32,
    y: f32,
}

const MENU_W: f32 = 232.0;
const MENU_ROW_H: f32 = 32.0;

// ---- Launcher view ----------------------------------------------------------------------

/// The round buttons next to the search field (macOS Spotlight's Applications, Files, Actions
/// and Clipboard). Each one types a query the launcher already understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quick {
    Applications,
    Files,
    Actions,
    Clipboard,
}

impl Quick {
    const ALL: [Quick; 4] = [
        Quick::Applications,
        Quick::Files,
        Quick::Actions,
        Quick::Clipboard,
    ];

    fn icon(self) -> &'static str {
        match self {
            Self::Applications => "icons/app.svg",
            Self::Files => "icons/folder.svg",
            Self::Actions => "icons/layers.svg",
            Self::Clipboard => "icons/clipboard.svg",
        }
    }

    /// What the search field shows while this button's query is active.
    fn title(self) -> &'static str {
        match self {
            Self::Applications => "Applications",
            Self::Files => "Recent Files",
            Self::Actions => "Actions",
            Self::Clipboard => "Clipboard",
        }
    }

    /// What the tooltip says; a button that is on offers to switch itself off.
    fn hint(self, on: bool) -> &'static str {
        match (self, on) {
            (Self::Applications, false) => "Applications",
            (Self::Applications, true) => "Back to all results",
            (Self::Files, false) => "Files",
            (Self::Files, true) => "Back to all results",
            (Self::Actions, false) => "Actions",
            (Self::Actions, true) => "Back to all results",
            (Self::Clipboard, false) => "Clipboard history",
            (Self::Clipboard, true) => "Back to all results",
        }
    }

    /// The query this button types.
    fn query(self) -> &'static str {
        match self {
            Self::Applications => "kind:app",
            Self::Files => "kind:recent",
            Self::Actions => "kind:action",
            Self::Clipboard => "clip",
        }
    }
}

/// Results that the bar's grey " \u{2014} Title" and right-hand icon speak for: not the web-search
/// row, clipboard entries, or inert rows (hints).
fn speaks_in_the_bar(hit: &SearchHit) -> bool {
    !matches!(hit.kind.as_str(), "web" | "clip") && !is_inert(&hit.kind)
}

/// The grey completion after the typed text, like Spotlight's "weather in carmel \u{2014} Weather":
/// the title of the result Enter would open. Nothing when it only repeats what was typed.
fn ghost_text(query: &str, hit: &SearchHit) -> Option<String> {
    let query = query.trim();
    let title = hit.title.trim();

    if query.is_empty() || title.is_empty() || !speaks_in_the_bar(hit) {
        return None;
    }

    if title.to_lowercase() == query.to_lowercase() {
        return None;
    }

    Some(format!(" \u{2014} {}", first_chars(title, 40)))
}

/// A small text bubble shown when the mouse rests on an icon-only button.
struct HoverTip {
    text: SharedString,
    bg: u32,
    fg: u32,
    border: u32,
}

impl Render for HoverTip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        rounded_px(div(), 7.)
            .px_2()
            .py_1()
            .text_xs()
            .border_1()
            .border_color(rgba(self.border))
            .bg(rgba(self.bg))
            .text_color(rgb(self.fg))
            .child(self.text.clone())
    }
}

/// Step through `count` chip positions (0 is "All") from `current`, wrapping at both ends.
fn cycle_index(count: usize, current: usize, delta: isize) -> usize {
    if count == 0 {
        return 0;
    }

    (current as isize + delta).rem_euclid(count as isize) as usize
}

/// "Suggestions" in the Files grid: how many of your most-opened recent files lead it.
const SUGGESTIONS_MAX: usize = 5;

/// Whether a file name is a picture the grid can show as its own thumbnail.
fn is_image_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
            )
        })
}

/// Where each tile of the Files grid sits among the list's children: a "Suggestions" header and
/// its rows (when there are suggestions), then a "Recents" header and its rows.
fn files_grid_rows(suggested: usize, total: usize) -> Vec<usize> {
    let mut rows = Vec::with_capacity(total);
    let mut child = 0;

    if suggested > 0 {
        child += 1;
        rows.extend((0..suggested).map(|index| child + index / GRID_COLS));
        child += suggested.div_ceil(GRID_COLS);
    }

    if total > suggested {
        child += 1;
        rows.extend((0..total - suggested).map(|index| child + index / GRID_COLS));
    }

    rows
}

/// Applications per row in the Applications grid.
const GRID_COLS: usize = 5;

/// "View as List" in the "..." menu. The UI process outlives the launcher window, so the choice
/// lasts until the UI is restarted.
static APPS_AS_LIST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn apps_as_list() -> bool {
    APPS_AS_LIST.load(Ordering::Relaxed)
}

/// The file-type chips of the Files view: label, the `kind:` word the daemon understands, and
/// the icon on the chip and on the token in the search bar.
const FILE_KINDS: [(&str, &str, &str); 8] = [
    ("Documents", "document", "icons/file-text.svg"),
    ("PDFs", "pdf", "icons/pdf.svg"),
    ("Images", "image", "icons/image.svg"),
    ("Videos", "video", "icons/video.svg"),
    ("Audio", "audio", "icons/music.svg"),
    ("Archives", "archive", "icons/archive.svg"),
    ("Code", "code", "icons/code.svg"),
    ("Folders", "folder", "icons/folder.svg"),
];

/// The icon of an Applications group chip.
fn group_icon(group: &str) -> &'static str {
    match group {
        "Internet" => "icons/globe.svg",
        "Productivity" => "icons/briefcase.svg",
        "Media" => "icons/video.svg",
        "Graphics" => "icons/image.svg",
        "Development" => "icons/code.svg",
        "Games" => "icons/gamepad.svg",
        "Utilities" => "icons/wrench.svg",
        "System" => "icons/sliders.svg",
        "Education & Science" => "icons/book.svg",
        _ => "icons/app.svg",
    }
}

/// Something the launcher understands, shown as a click-to-try example.
struct Tip {
    title: &'static str,
    /// What it looks like when typed.
    example: &'static str,
    /// What a click puts in the search field.
    fill: &'static str,
    /// The plugin that must be on for the tip to make sense.
    plugin: Option<&'static str>,
}

const TIPS: [Tip; 12] = [
    Tip {
        title: "Open an app",
        example: "fire",
        fill: "fire",
        plugin: None,
    },
    Tip {
        title: "Find a file",
        example: "report",
        fill: "report",
        plugin: Some("files"),
    },
    Tip {
        title: "Only one kind of file",
        example: "kind:pdf invoice",
        fill: "kind:pdf ",
        plugin: Some("files"),
    },
    Tip {
        title: "Do math",
        example: "2*(3+4)",
        fill: "2*(3+4)",
        plugin: Some("calculator"),
    },
    Tip {
        title: "Convert units",
        example: "10 km to mi",
        fill: "10 km to mi",
        plugin: Some("units"),
    },
    Tip {
        title: "Lock, sleep, restart",
        example: "lock",
        fill: "lock",
        plugin: Some("system"),
    },
    Tip {
        title: "Switch to an open window",
        example: "< firefox",
        fill: "< ",
        plugin: Some("windows"),
    },
    Tip {
        title: "Look up a word",
        example: "define serendipity",
        fill: "define ",
        plugin: Some("dictionary"),
    },
    Tip {
        title: "Check the weather",
        example: "weather kathmandu",
        fill: "weather ",
        plugin: Some("weather"),
    },
    Tip {
        title: "Search the web",
        example: "g rust lifetimes",
        fill: "g ",
        plugin: None,
    },
    Tip {
        title: "Remote desktop",
        example: "rdp office",
        fill: "rdp ",
        plugin: Some("nexdesk"),
    },
    Tip {
        title: "Clipboard history",
        example: "clip",
        fill: "clip",
        plugin: Some("clipboard"),
    },
];

/// The keys worth knowing, for the tips panel.
const SHORTCUTS: [(&str, &str); 9] = [
    ("Enter", "Open"),
    ("Ctrl+K", "More actions"),
    ("Ctrl+C", "Copy"),
    ("Ctrl+L", "Look up a word"),
    ("\u{2191} \u{2193}  Tab", "Move"),
    ("Alt+1 \u{2026} 9", "Open that row"),
    ("Ctrl+B", "Search the web"),
    ("Ctrl+,", "Settings"),
    ("Esc", "Clear, then close"),
];

/// The items of the "..." menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MoreAction {
    Grid,
    List,
    Tips,
    Settings,
}

/// The groups of the Applications grid, in the order their chips appear.
const APP_GROUPS: [&str; 10] = [
    "Internet",
    "Productivity",
    "Media",
    "Graphics",
    "Development",
    "Games",
    "Utilities",
    "System",
    "Education & Science",
    "Other",
];

/// The grid group the daemon put on an application (`kind:app` results only).
fn app_group(hit: &SearchHit) -> Option<&str> {
    hit.detail.as_deref()?.strip_prefix("group:")
}

/// Queries that list everything of one kind. They get a longer list and no web search row.
fn is_listing_query(lower: &str) -> bool {
    matches!(lower, "kind:app" | "kind:action" | "kind:recent")
        || lower.starts_with("kind:app ")
        || lower.starts_with("kind:action ")
}

/// The query for the typed `text` inside a quick button's `scope`: `kind:app fire` for
/// "fire" in Applications, the scope's own listing query while the text is empty, and the text
/// unchanged when no scope is on.
fn scoped_query(scope: Option<Quick>, file_kind: Option<&str>, text: &str) -> String {
    let Some(scope) = scope else {
        return text.to_owned();
    };

    let text = text.trim();

    // A file-type chip ("PDFs") turns the Files view into `kind:pdf` plus whatever is typed.
    if let (Quick::Files, Some(kind)) = (scope, file_kind) {
        return if text.is_empty() {
            format!("kind:{kind}")
        } else {
            format!("kind:{kind} {text}")
        };
    }

    if text.is_empty() {
        return scope.query().to_owned();
    }

    match scope {
        Quick::Applications | Quick::Actions | Quick::Clipboard => {
            format!("{} {text}", scope.query())
        }
        // `kind:file` lets the daemon list up to 40 matches (a plain search stops at 8).
        Quick::Files => format!("kind:file {text}"),
    }
}

/// Which results belong to a quick button's scope. Other providers also answer the same query;
/// only these kinds are shown while the scope is on.
fn scope_keeps(scope: Quick, kind: &str) -> bool {
    match scope {
        Quick::Applications => kind == "application",
        Quick::Files => matches!(kind, "file" | "folder"),
        Quick::Actions => matches!(kind, "system" | "action"),
        Quick::Clipboard => true,
    }
}

/// The chip under the search field for a section, when that section can be filtered to.
fn chip_label(section: Section) -> Option<&'static str> {
    match section {
        Section::Applications => Some("Applications"),
        Section::Windows => Some("Windows"),
        Section::Commands => Some("Actions"),
        Section::Files => Some("Files"),
        Section::Clipboard => Some("Clipboard"),
        Section::Web => Some("Web"),
        _ => None,
    }
}

struct LauncherView {
    query: String,
    /// Selection anchor and caret: byte offsets into `query`, always on character boundaries.
    /// They are equal when nothing is selected.
    anchor: usize,
    cursor: usize,
    /// Undo/redo snapshots of `(query, anchor, cursor)`.
    undo: Vec<(String, usize, usize)>,
    redo: Vec<(String, usize, usize)>,
    /// Set while the left mouse button is held after pressing inside the search field.
    drag_anchor: Option<usize>,
    results: Vec<SearchHit>,
    /// Section of each result, same length as `results`.
    sections: Vec<Section>,
    /// Everything the last search returned, before the chip filter (same lengths as each other).
    all_hits: Vec<SearchHit>,
    all_sections: Vec<Section>,
    /// The chip under the search field that narrows the results; `None` is "All".
    chip: Option<Section>,
    /// The group chip of the Applications grid; `None` is "All".
    group: Option<String>,
    /// The round button whose scope is on. It stays on while you type: the text then searches
    /// inside it (Spotlight's Applications, Files, Actions and Clipboard views).
    scope: Option<Quick>,
    /// The file-type chip of the Files view (`pdf`, `image`, ...); `None` is "All".
    file_kind: Option<&'static str>,
    /// How many tiles at the front of the Files grid are "Suggestions".
    suggested: usize,
    /// The height the window was last resized to.
    window_h: f32,
    /// The "..." menu is open.
    more: bool,
    /// The tips panel replaces the results.
    tips: bool,
    /// Index of each result among the list's child elements (section headers count).
    row_index: Vec<usize>,
    scroll: ScrollHandle,
    /// Scroll position of the preview pane (reset whenever the selection changes).
    preview_scroll: ScrollHandle,
    selected: usize,
    status: Option<String>,
    /// A hint (not an error), shown dimmed.
    notice: Option<String>,
    /// Id of a dangerous result that is waiting for a second Enter.
    confirming: Option<String>,
    preview: Option<Preview>,
    menu: Option<ContextMenu>,
    settings: Settings,
    palette: Palette,
    history: History,
    /// File history from the daemon (`kind:recent`), for the empty launcher.
    recent_files: Vec<SearchHit>,
    /// Snapshot of the clipboard history taken when the launcher opened (newest first).
    clips: Vec<ClipEntry>,
    requests: async_channel::Sender<UiRequest>,
    focus_handle: FocusHandle,
    /// Window position when the launcher opened, and the latest one (whole pixels), used to
    /// remember where you moved it. Only tracked where the app can place windows (not Wayland).
    first_origin: Option<(i32, i32)>,
    last_origin: Option<(i32, i32)>,
}

/// Wayland compositors decide where windows go and never tell the app where they are, so a
/// position can be set and read back on X11 only. Dragging a window works on both.
fn can_place_windows() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_none()
}

impl Drop for LauncherView {
    /// Save the position the launcher was left at, when it should reopen there.
    fn drop(&mut self) {
        let remember = self.settings.appearance.launcher_position == LauncherPosition::Remember;

        if !remember || self.first_origin == self.last_origin {
            return;
        }

        if let Some(origin) = self.last_origin {
            let mut settings = Settings::load_or_default();

            settings.appearance.launcher_origin = Some(origin);

            let _ = settings.save();
        }
    }
}

/// Longest query accepted, in characters.
const MAX_QUERY_CHARS: usize = 300;

impl LauncherView {
    fn new(
        requests: async_channel::Sender<UiRequest>,
        settings: Settings,
        clips: Vec<ClipEntry>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            query: String::new(),
            anchor: 0,
            cursor: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            drag_anchor: None,
            results: Vec::new(),
            sections: Vec::new(),
            all_hits: Vec::new(),
            all_sections: Vec::new(),
            chip: None,
            group: None,
            scope: None,
            file_kind: None,
            suggested: 0,
            window_h: WINDOW_H,
            more: false,
            tips: false,
            row_index: Vec::new(),
            scroll: ScrollHandle::new(),
            preview_scroll: ScrollHandle::new(),
            selected: 0,
            status: None,
            notice: None,
            confirming: None,
            preview: None,
            menu: None,
            palette: Palette::from_settings(&settings),
            history: History::load_or_default(),
            recent_files: Vec::new(),
            settings,
            clips,
            requests,
            focus_handle: cx.focus_handle(),
            first_origin: None,
            last_origin: None,
        };

        view.show_empty_state();

        view
    }

    // ---- Query editing ----

    fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.cursor), self.anchor.max(self.cursor))
    }

    fn has_selection(&self) -> bool {
        self.anchor != self.cursor
    }

    fn selected_text(&self) -> &str {
        let (start, end) = self.selection();

        &self.query[start..end]
    }

    fn prev_boundary(&self, from: usize) -> usize {
        self.query[..from]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_boundary(&self, from: usize) -> usize {
        self.query[from..]
            .chars()
            .next()
            .map_or(from, |c| from + c.len_utf8())
    }

    fn word_left(&self, from: usize) -> usize {
        let head = self.query[..from].trim_end();

        head.rfind(char::is_whitespace).map_or(0, |index| {
            index + head[index..].chars().next().map_or(1, char::len_utf8)
        })
    }

    fn word_right(&self, from: usize) -> usize {
        let rest = &self.query[from..];
        let skipped = rest.len() - rest.trim_start().len();
        let word = &rest[skipped..];
        let length = word.find(char::is_whitespace).unwrap_or(word.len());

        from + skipped + length
    }

    /// Start and end of the word around `index` (for double-click).
    fn word_bounds(&self, index: usize) -> (usize, usize) {
        let index = index.min(self.query.len());

        let start = self.query[..index]
            .rfind(char::is_whitespace)
            .map_or(0, |at| {
                at + self.query[at..].chars().next().map_or(1, char::len_utf8)
            });
        let end = self.query[index..]
            .find(char::is_whitespace)
            .map_or(self.query.len(), |at| index + at);

        (start, end.max(start))
    }

    /// Move the caret; with `extend` the anchor stays, so the text in between is selected.
    fn move_cursor(&mut self, to: usize, extend: bool, cx: &mut Context<Self>) {
        self.cursor = to.min(self.query.len());

        if !extend {
            self.anchor = self.cursor;
        }

        cx.notify();
    }

    fn snapshot(&mut self) {
        self.undo
            .push((self.query.clone(), self.anchor, self.cursor));

        if self.undo.len() > 100 {
            self.undo.remove(0);
        }

        self.redo.clear();
    }

    /// Replace the selection (or insert at the caret) and put the caret after the new text.
    fn replace_selection(&mut self, text: &str) {
        let room = MAX_QUERY_CHARS.saturating_sub(self.query.chars().count());
        let (start, end) = self.selection();
        let removed = self.query[start..end].chars().count();
        let text: String = text.chars().take(room + removed).collect();

        self.snapshot();
        self.query.replace_range(start..end, &text);
        self.cursor = start + text.len();
        self.anchor = self.cursor;
    }

    fn delete_range(&mut self, start: usize, end: usize) {
        self.snapshot();
        self.query.replace_range(start..end, "");
        self.cursor = start;
        self.anchor = start;
    }

    fn restore(&mut self, state: (String, usize, usize)) {
        self.query = state.0;
        self.anchor = state.1.min(self.query.len());
        self.cursor = state.2.min(self.query.len());
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        if let Some(previous) = self.undo.pop() {
            self.redo
                .push((self.query.clone(), self.anchor, self.cursor));
            self.restore(previous);
            self.edited(cx);
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.redo.pop() {
            self.undo
                .push((self.query.clone(), self.anchor, self.cursor));
            self.restore(next);
            self.edited(cx);
        }
    }

    /// Call after the query text changed.
    fn edited(&mut self, cx: &mut Context<Self>) {
        self.more = false;
        self.tips = false;
        self.cursor = self.cursor.min(self.query.len());
        self.anchor = self.anchor.min(self.query.len());
        self.request_search();
        cx.notify();
    }

    // ---- Mouse in the search field ----

    /// `index` is the byte offset the pointer is closest to (left or right half of a character).
    fn mouse_down_at(&mut self, index: usize, clicks: usize, shift: bool, cx: &mut Context<Self>) {
        match clicks {
            0 | 1 => {
                if shift {
                    self.cursor = index;
                } else {
                    self.anchor = index;
                    self.cursor = index;
                }

                self.drag_anchor = Some(self.anchor);
            }
            2 => {
                // Double-click selects the word.
                let (start, end) = self.word_bounds(index);

                self.anchor = start;
                self.cursor = end;
                self.drag_anchor = None;
            }
            _ => {
                // Triple-click selects everything.
                self.anchor = 0;
                self.cursor = self.query.len();
                self.drag_anchor = None;
            }
        }

        cx.notify();
    }

    fn mouse_drag_to(&mut self, index: usize, left_button_down: bool, cx: &mut Context<Self>) {
        if let (true, Some(anchor)) = (left_button_down, self.drag_anchor) {
            self.anchor = anchor;
            self.cursor = index;
            cx.notify();
        }
    }

    // ---- Searching ----

    /// `clip foo` / `cb foo`: the text after the keyword, when clipboard history is on.
    fn clip_filter(&self) -> Option<String> {
        if !self.settings.plugins.clipboard {
            return None;
        }

        let lower = self.search_text().trim().to_lowercase();

        for keyword in ["clip", "cb"] {
            if lower == keyword {
                return Some(String::new());
            }

            if let Some(rest) = lower
                .strip_prefix(keyword)
                .filter(|rest| rest.starts_with(' '))
            {
                return Some(rest.trim().to_owned());
            }
        }

        None
    }

    fn request_search(&mut self) {
        self.selected = 0;
        self.status = None;
        self.notice = None;
        self.confirming = None;
        self.menu = None;

        let query = self.search_text();

        if query.trim().is_empty() {
            self.chip = None;
            self.show_empty_state();
            return;
        }

        if let Some(filter) = self.clip_filter() {
            self.show_clips(&filter);
            return;
        }

        let _ = self
            .requests
            .try_send(UiRequest::Search { query, attempt: 0 });
    }

    fn install(&mut self, hits: Vec<SearchHit>, sections: Vec<Section>) {
        self.all_hits = hits.clone();
        self.all_sections = sections.clone();

        // A chip that the new results no longer have falls back to "All".
        let chip_gone = self.chip.is_some_and(|chip| {
            !self
                .all_hits
                .iter()
                .any(|hit| section_of(&hit.kind) == chip)
        });

        if chip_gone {
            self.chip = None;
        }

        let (hits, sections): (Vec<SearchHit>, Vec<Section>) = match self.chip {
            None => (hits, sections),
            Some(chip) => hits
                .into_iter()
                .zip(sections)
                .filter(|(hit, _)| section_of(&hit.kind) == chip)
                .unzip(),
        };

        // The Applications grid has its own group chips.
        let grid = self.is_grid_list(&self.all_hits);
        let group_gone = !grid
            || self.group.as_ref().is_some_and(|group| {
                !self
                    .all_hits
                    .iter()
                    .any(|hit| app_group(hit) == Some(group.as_str()))
            });

        if group_gone {
            self.group = None;
        }

        let (hits, sections): (Vec<SearchHit>, Vec<Section>) = match &self.group {
            None => (hits, sections),
            Some(group) => hits
                .into_iter()
                .zip(sections)
                .filter(|(hit, _)| app_group(hit) == Some(group.as_str()))
                .unzip(),
        };

        // The Files grid leads with the recent files you open most from here ("Suggestions").
        let files_grid = self.is_files_grid(&self.all_hits);
        let (hits, sections, suggested): (Vec<SearchHit>, Vec<Section>, usize) = if files_grid {
            let count = |index: usize| self.history.count_of(&hits[index].id);
            let mut leading: Vec<usize> =
                (0..hits.len()).filter(|&index| count(index) > 0).collect();

            leading.sort_by_key(|&index| std::cmp::Reverse(count(index)));
            leading.truncate(SUGGESTIONS_MAX);

            let mut order = leading.clone();

            order.extend((0..hits.len()).filter(|index| !leading.contains(index)));

            (
                order.iter().map(|&index| hits[index].clone()).collect(),
                order.iter().map(|&index| sections[index]).collect(),
                leading.len(),
            )
        } else {
            (hits, sections, 0)
        };

        self.suggested = suggested;

        // Where each result sits among the list's children. In the list, a header precedes
        // each section; in the grid, one header precedes rows of `GRID_COLS` tiles.
        let mut row_index = Vec::with_capacity(sections.len());

        if files_grid {
            row_index.extend(files_grid_rows(suggested, sections.len()));
        } else if grid && !apps_as_list() {
            row_index.extend((0..sections.len()).map(|index| 1 + index / GRID_COLS));
        } else {
            let mut headers = 0;
            let mut previous = None;

            for (index, section) in sections.iter().enumerate() {
                if previous != Some(*section) {
                    headers += 1;
                    previous = Some(*section);
                }

                row_index.push(index + headers);
            }
        }

        self.results = hits;
        self.sections = sections;
        self.row_index = row_index;
        self.selected = 0;
        self.confirming = None;
        self.refresh_preview();
        self.scroll_to_selected();
    }

    fn scroll_to_selected(&self) {
        if let Some(index) = self.row_index.get(self.selected) {
            // Show the section header too when the first result of a section is selected.
            let first_of_section = self
                .selected
                .checked_sub(1)
                .is_none_or(|before| self.sections[before] != self.sections[self.selected]);

            self.scroll
                .scroll_to_item(if first_of_section { index - 1 } else { *index });
        }
    }

    /// Scroll the preview pane by `delta` pixels (positive is down), within its content.
    fn scroll_preview(&self, delta: f32, cx: &mut Context<Self>) {
        let offset = self.preview_scroll.offset();
        let furthest = f32::from(self.preview_scroll.max_offset().y).abs();
        let y = (f32::from(offset.y) - delta).clamp(-furthest, 0.0);

        self.preview_scroll.set_offset(point(offset.x, px(y)));
        cx.notify();
    }

    fn refresh_preview(&mut self) {
        self.preview_scroll.set_offset(point(px(0.), px(0.)));

        self.preview = if self.settings.appearance.show_preview {
            self.results
                .get(self.selected)
                .map(|hit| build_preview(hit, &self.clips))
        } else {
            None
        };

        if let Some(path) = self
            .preview
            .as_mut()
            .and_then(|preview| preview.thumb_request.take())
        {
            let _ = self.requests.try_send(UiRequest::Thumbnail(path));
        }
    }

    /// Empty search box: ask for the file history, and show what we already know.
    fn show_empty_state(&mut self) {
        let _ = self.requests.try_send(UiRequest::Recents);
        self.build_empty_state();
    }

    fn build_empty_state(&mut self) {
        if !self.settings.appearance.show_recents {
            self.install(Vec::new(), Vec::new());
            return;
        }

        let as_hit = |entry: &aujar_config::HistoryEntry| SearchHit {
            id: entry.id.clone(),
            title: entry.title.clone(),
            subtitle: entry.subtitle.clone(),
            kind: entry.kind.clone(),
            score: 0,
            icon: entry.icon.clone(),
            detail: None,
        };

        let recent = self.history.recent(5);
        let recent_ids: Vec<String> = recent.iter().map(|entry| entry.id.clone()).collect();
        let skip: Vec<&str> = recent_ids.iter().map(String::as_str).collect();
        let frequent = self.history.frequent(4, &skip);

        let mut hits = Vec::new();
        let mut sections = Vec::new();

        for entry in recent {
            hits.push(as_hit(entry));
            sections.push(Section::Recent);
        }

        // File history from the desktop (includes PDFs opened in any app).
        for hit in self
            .recent_files
            .iter()
            .filter(|hit| !recent_ids.contains(&hit.id))
            .take(5)
        {
            hits.push(hit.clone());
            sections.push(Section::RecentFiles);
        }

        for entry in frequent {
            hits.push(as_hit(entry));
            sections.push(Section::Frequent);
        }

        self.install(hits, sections);
    }

    fn show_clips(&mut self, filter: &str) {
        let filter = filter.to_lowercase();
        let now = now_secs();
        let now_stamp = local_stamp(now);

        let hits: Vec<SearchHit> = self
            .clips
            .iter()
            .enumerate()
            .filter(|(_, clip)| filter.is_empty() || clip.text.to_lowercase().contains(&filter))
            .take(20)
            .map(|(index, clip)| {
                let first = clip.text.lines().next().unwrap_or_default().trim();
                let is_url = is_web_url(first) && !first.contains(char::is_whitespace);
                let age = Duration::from_secs(now.saturating_sub(clip.copied_at));
                let copied = format!(
                    "Copied {}",
                    copied_text(now_stamp, local_stamp(clip.copied_at), age)
                );

                // "URL · en.wikipedia.org · Copied 4:17 PM", "Text · Copied 4:19 PM".
                let subtitle = match (is_url, url_host(first)) {
                    (true, Some(host)) => format!("URL \u{b7} {host} \u{b7} {copied}"),
                    (true, None) => format!("URL \u{b7} {copied}"),
                    (false, _) => format!("Text \u{b7} {copied}"),
                };

                SearchHit {
                    id: format!("clip:{index}"),
                    title: first_chars(first, 90),
                    subtitle: Some(subtitle),
                    kind: "clip".to_owned(),
                    score: 0,
                    icon: None,
                    detail: None,
                }
            })
            .collect();

        if self.clips.is_empty() {
            self.notice = Some(
                "Nothing here yet. Clipboard history records what you copy while Aujar's UI is running."
                    .to_owned(),
            );
        }

        let sections = vec![Section::Clipboard; hits.len()];

        self.install(hits, sections);
    }

    /// Add the UI-side results (settings, web), group into sections and cap each section.
    fn set_results(&mut self, mut hits: Vec<SearchHit>) {
        let query = self.search_text().trim().to_owned();
        let lower = query.to_lowercase();
        let is_shell = query.starts_with('>');

        // Inside a scope the list is the whole point: no web row, no top hit, no section caps.
        let listing = self.scope.is_some() || is_listing_query(&lower);

        if let Some(scope) = self.scope {
            hits.retain(|hit| scope_keeps(scope, &hit.kind));
        }

        if !is_shell && lower.len() >= 3 && "aujar settings preferences".contains(&lower) {
            hits.push(SearchHit {
                id: "ui:settings".to_owned(),
                title: "Aujar Settings".to_owned(),
                subtitle: Some("Appearance, plugins and web search".to_owned()),
                kind: "action".to_owned(),
                score: 0,
                icon: None,
                detail: None,
            });
        }

        if !is_shell && !listing && !query.is_empty() {
            let engine = self.settings.web.engine;
            let mut web = Vec::new();

            if let Some(url) = url_from_query(&query) {
                web.push(web_hit(format!("Open {query}"), url.clone(), url));
            }

            // `g rust lifetimes`, `yt lofi`, ... and `? anything` for the default engine.
            if let Some((keyword, rest)) = query.split_once(char::is_whitespace) {
                let rest = rest.trim();

                if !rest.is_empty() {
                    if keyword == "?" {
                        web.push(web_hit(
                            format!("Search {} for \u{201c}{rest}\u{201d}", engine.name()),
                            "Default search engine".to_owned(),
                            format!("{}{}", engine.url(), percent_encode(rest)),
                        ));
                    } else if let Some(shortcut) = self
                        .settings
                        .web
                        .shortcuts
                        .iter()
                        .find(|shortcut| shortcut.keyword.eq_ignore_ascii_case(keyword))
                    {
                        web.push(web_hit(
                            format!("Search {} for \u{201c}{rest}\u{201d}", shortcut.name),
                            format!("Shortcut `{}`", shortcut.keyword),
                            format!("{}{}", shortcut.url, percent_encode(rest)),
                        ));
                    }
                }
            }

            web.push(web_hit(
                format!("Search {} for \u{201c}{query}\u{201d}", engine.name()),
                "Opens your default browser".to_owned(),
                format!("{}{}", engine.url(), percent_encode(&query)),
            ));

            hits.extend(web);
        }

        // Stable sort: sections in display order; inside a section, things you launch often
        // float up (a nudge, not an override of the daemon's ranking).
        let history = &self.history;

        hits.sort_by_key(|hit| {
            // A listing keeps the daemon's order (A to Z); usage must not reshuffle it.
            let boost = if listing {
                0
            } else {
                150 * history.count_of(&hit.id).min(5)
            };

            (
                section_of(&hit.kind) as usize,
                std::cmp::Reverse(hit.score + boost),
            )
        });

        let apps_cap = self.settings.appearance.max_results as usize;
        let mut counts = [0usize; SECTION_COUNT];

        hits.retain(|hit| {
            let section = section_of(&hit.kind);
            let cap = if listing {
                // A listing is the whole point of the query: the daemon already limits it.
                usize::MAX
            } else {
                match section {
                    Section::Answer => 2,
                    Section::Lookup => 6,
                    Section::Windows => 8,
                    Section::Applications => apps_cap,
                    Section::Commands => 4,
                    Section::Files => apps_cap.min(6),
                    Section::Web => 3,
                    Section::Recent
                    | Section::RecentFiles
                    | Section::Frequent
                    | Section::TopHit
                    | Section::Clipboard => usize::MAX,
                }
            };

            counts[section as usize] += 1;
            counts[section as usize] <= cap
        });

        let mut sections: Vec<Section> = hits.iter().map(|hit| section_of(&hit.kind)).collect();

        // Top Hit (like Spotlight): one strong application/command match, shown first.
        // Skipped when there is an Answer: that already is the top result.
        if !listing && !sections.contains(&Section::Answer) {
            let best = hits
                .iter()
                .enumerate()
                .filter(|(index, hit)| {
                    matches!(sections[*index], Section::Applications | Section::Commands)
                        && hit.score >= 600
                })
                .max_by_key(|(_, hit)| hit.score)
                .map(|(index, _)| index);

            if let Some(position) = best {
                let hit = hits.remove(position);
                sections.remove(position);

                let slot = sections
                    .iter()
                    .take_while(|section| (**section as usize) < Section::TopHit as usize)
                    .count();

                hits.insert(slot, hit);
                sections.insert(slot, Section::TopHit);
            }
        }

        self.install(hits, sections);
        self.status = None;
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.results.len() as isize;

        if len > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
            self.confirming = None;
            self.refresh_preview();
            self.scroll_to_selected();
            cx.notify();
        }
    }

    /// What Ctrl+C copies for a result: a path, a URL, a clipboard entry or the answer.
    fn copy_text(&self, hit: &SearchHit) -> String {
        if let Some(path) = hit.id.strip_prefix("files:open:") {
            return path.to_owned();
        }

        if let Some(url) = hit.id.strip_prefix("url:") {
            return url.to_owned();
        }

        if let Some(clip) = hit
            .id
            .strip_prefix("clip:")
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| self.clips.get(index))
        {
            return clip.text.clone();
        }

        match hit.kind.as_str() {
            "definition" => match hit.detail.as_deref().map(definition_parts) {
                Some((first, _)) if !first.is_empty() => format!("{}: {first}", hit.title),
                _ => hit.title.clone(),
            },
            "weather" | "stock" | "sports" => match &hit.subtitle {
                Some(subtitle) => format!("{} \u{2014} {subtitle}", hit.title),
                None => hit.title.clone(),
            },
            _ => hit.title.clone(),
        }
    }

    /// Ctrl+B (Spotlight's Cmd+B): search the web for the query, whatever is selected.
    fn open_web_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self
            .results
            .iter()
            .rev()
            .find(|hit| hit.kind == "web")
            .and_then(|hit| hit.id.strip_prefix("url:"))
            .filter(|url| is_web_url(url))
            .map(str::to_owned);

        if let Some(url) = url {
            cx.open_url(&url);
            window.remove_window();
        }
    }

    /// Replace the whole query (used by Look Up) and search for it.
    fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let text: String = text.chars().take(MAX_QUERY_CHARS).collect();

        self.snapshot();
        self.query = text;
        self.cursor = self.query.len();
        self.anchor = self.cursor;
        self.edited(cx);
    }

    /// Ctrl+L (Spotlight's Cmd+L): define the selected text, else the query, else the word in
    /// the selected result's title.
    fn look_up(&mut self, cx: &mut Context<Self>) {
        let selected = if self.has_selection() {
            self.selected_text().to_owned()
        } else {
            String::new()
        };
        let title = self
            .results
            .get(self.selected)
            .map(|hit| hit.title.clone())
            .unwrap_or_default();

        match define_query(&[selected.as_str(), self.query.as_str(), title.as_str()]) {
            Some(query) => self.set_query(&query, cx),
            None => {
                self.notice = Some("Select or type a word first, then press Ctrl+L.".to_owned());
                cx.notify();
            }
        }
    }

    /// Whether a dictionary word is available for the selected result (menu entry).
    fn can_look_up(&self) -> bool {
        let title = self
            .results
            .get(self.selected)
            .map(|hit| hit.title.as_str())
            .unwrap_or("");

        self.settings.plugins.dictionary && define_query(&[self.query.as_str(), title]).is_some()
    }

    /// Open the actions menu for result `index` at window position (`x`, `y`), or beside the
    /// selected row when opened from the keyboard.
    fn open_menu(&mut self, index: usize, at: Option<(f32, f32)>, cx: &mut Context<Self>) {
        if index >= self.results.len() {
            return;
        }

        if self.selected != index {
            self.selected = index;
            self.confirming = None;
            self.refresh_preview();
        }

        let items = menu_items(&self.results[index], self.can_look_up());

        if items.is_empty() {
            return;
        }

        let width = self.settings.appearance.width as f32;
        let height = items.len() as f32 * MENU_ROW_H + 12.0;

        // Keyboard: a fixed spot under the search bar, near the left of the list.
        let (x, y) = at.unwrap_or((width - MENU_W - 24.0, 120.0));

        self.menu = Some(ContextMenu {
            items,
            selected: 0,
            x: x.clamp(8.0, (width - MENU_W - 8.0).max(8.0)),
            y: y.clamp(8.0, (self.window_h - height - 8.0).max(8.0)),
        });

        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    /// Run a menu action on the selected result.
    fn perform(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;

        let Some(hit) = self.results.get(self.selected).cloned() else {
            return;
        };

        match action {
            Action::Primary => self.launch_selected(false, window, cx),
            Action::Reveal => self.launch_selected(true, window, cx),
            Action::Copy => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.copy_text(&hit)));
                window.remove_window();
            }
            Action::LookUp => self.look_up(cx),
            Action::WebSearch => self.open_web_search(window, cx),
        }

        cx.notify();
    }

    /// Keys while the actions menu is open. Everything else is swallowed.
    fn on_menu_key(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };

        let count = menu.items.len();

        match key {
            "escape" => self.close_menu(cx),
            "down" | "tab" => {
                menu.selected = (menu.selected + 1) % count;
                cx.notify();
            }
            "up" => {
                menu.selected = (menu.selected + count - 1) % count;
                cx.notify();
            }
            "enter" => {
                let action = menu.items[menu.selected].action;

                self.perform(action, window, cx);
            }
            _ => {}
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        let key = keystroke.key.as_str();
        let (ctrl, shift, alt) = (modifiers.control, modifiers.shift, modifiers.alt);

        if self.menu.is_some() {
            self.on_menu_key(key, window, cx);
            return;
        }

        // The keyboard route to the right-click menu.
        if key == "menu" || (key == "f10" && shift) {
            self.open_menu(self.selected, None, cx);
            return;
        }

        // Backspace on an empty field takes the scope off (Spotlight's behaviour). With text in
        // the field, typing and deleting edit the text and search inside the scope.
        if self.scope.is_some()
            && self.query.is_empty()
            && !ctrl
            && !alt
            && matches!(key, "backspace" | "delete")
        {
            if !self.remove_token(cx) {
                self.clear_scope(cx);
            }

            return;
        }

        // The Applications grid: arrows move across and down the tiles.
        if self.grid_active() && !ctrl && !alt && !shift {
            let step = match key {
                "left" => Some(-1),
                "right" => Some(1),
                "up" => Some(-(GRID_COLS as isize)),
                "down" => Some(GRID_COLS as isize),
                _ => None,
            };

            if let Some(step) = step {
                self.move_grid_selection(step, cx);
                return;
            }
        }

        // Alt+1..9 launches that row directly (Spotlight's Cmd+1..9).
        if alt && !ctrl {
            if let Some(number) = key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                if number <= self.results.len() {
                    self.selected = number - 1;
                    self.launch_selected(false, window, cx);
                }
                return;
            }
        }

        // Alt+Left/Right switch the chips under the bar (All, Applications, Files, ...).
        if alt && !ctrl && matches!(key, "left" | "right") {
            let delta = if key == "left" { -1 } else { 1 };

            if self.cycle_chip(delta, cx) {
                return;
            }
        }

        // Caret movement and deletion. Shift extends the selection, Ctrl works on words, and
        // the two combine (Ctrl+Shift+Left selects the previous word).
        match key {
            "left" => {
                let target = if ctrl {
                    self.word_left(self.cursor)
                } else if !shift && self.has_selection() {
                    self.selection().0
                } else {
                    self.prev_boundary(self.cursor)
                };

                self.move_cursor(target, shift, cx);
                return;
            }
            "right" => {
                let target = if ctrl {
                    self.word_right(self.cursor)
                } else if !shift && self.has_selection() {
                    self.selection().1
                } else {
                    self.next_boundary(self.cursor)
                };

                self.move_cursor(target, shift, cx);
                return;
            }
            "home" => {
                self.move_cursor(0, shift, cx);
                return;
            }
            "end" => {
                self.move_cursor(self.query.len(), shift, cx);
                return;
            }
            "backspace" => {
                if self.has_selection() {
                    let (start, end) = self.selection();
                    self.delete_range(start, end);
                } else if ctrl {
                    let start = self.word_left(self.cursor);
                    self.delete_range(start, self.cursor);
                } else if self.cursor > 0 {
                    let start = self.prev_boundary(self.cursor);
                    self.delete_range(start, self.cursor);
                } else {
                    return;
                }

                self.edited(cx);
                return;
            }
            "delete" => {
                if self.has_selection() {
                    let (start, end) = self.selection();
                    self.delete_range(start, end);
                } else if ctrl {
                    let end = self.word_right(self.cursor);
                    self.delete_range(self.cursor, end);
                } else if self.cursor < self.query.len() {
                    let end = self.next_boundary(self.cursor);
                    self.delete_range(self.cursor, end);
                } else {
                    return;
                }

                self.edited(cx);
                return;
            }
            _ => {}
        }

        // Ctrl chords (with or without Shift: Ctrl+Shift+C/V behave like Ctrl+C/V).
        if ctrl {
            match key {
                "n" | "j" => self.move_selection(1, cx),
                "p" => self.move_selection(-1, cx),
                "k" => self.open_menu(self.selected, None, cx),
                "l" => self.look_up(cx),
                "enter" => self.launch_selected(true, window, cx),
                "a" => {
                    self.anchor = 0;
                    self.cursor = self.query.len();
                    cx.notify();
                }
                "e" => self.move_cursor(self.query.len(), shift, cx),
                "c" => {
                    if self.has_selection() {
                        // Copy the selected text and keep the launcher open.
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            self.selected_text().to_owned(),
                        ));
                    } else if let Some(hit) = self.results.get(self.selected) {
                        cx.write_to_clipboard(ClipboardItem::new_string(self.copy_text(hit)));
                        window.remove_window();
                    }
                }
                "x" => {
                    if self.has_selection() {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            self.selected_text().to_owned(),
                        ));

                        let (start, end) = self.selection();
                        self.delete_range(start, end);
                        self.edited(cx);
                    }
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        let line: String = text
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .chars()
                            .filter(|c| !c.is_control())
                            .collect();

                        self.replace_selection(&line);
                        self.edited(cx);
                    }
                }
                "z" if shift => self.redo(cx),
                "z" => self.undo(cx),
                "y" => self.redo(cx),
                "u" => {
                    // Clear the whole query (readline's kill-line).
                    if !self.query.is_empty() {
                        self.delete_range(0, self.query.len());
                        self.edited(cx);
                    }
                }
                "w" => {
                    let start = self.word_left(self.cursor);
                    self.delete_range(start, self.cursor);
                    self.edited(cx);
                }
                "b" => self.open_web_search(window, cx),
                "/" => {
                    self.more = false;
                    self.tips = !self.tips;
                    cx.notify();
                }
                "," => {
                    let _ = self.requests.try_send(UiRequest::OpenSettings);
                    window.remove_window();
                }
                _ => {}
            }
            return;
        }

        // F1: help, like everywhere else.
        if key == "f1" {
            self.more = false;
            self.tips = !self.tips;
            cx.notify();
            return;
        }

        match key {
            "escape" => {
                // Esc closes the "..." menu, then the tips; then it clears the query (like
                // Spotlight), and last it closes the launcher.
                if self.more || self.tips {
                    self.more = false;
                    self.tips = false;
                    cx.notify();
                    return;
                }

                if self.query.is_empty() {
                    window.remove_window();
                } else {
                    self.delete_range(0, self.query.len());
                    self.edited(cx);
                }
                return;
            }
            "enter" => {
                self.launch_selected(false, window, cx);
                return;
            }
            "down" => {
                self.move_selection(1, cx);
                return;
            }
            "up" => {
                self.move_selection(-1, cx);
                return;
            }
            "tab" => {
                self.move_selection(if shift { -1 } else { 1 }, cx);
                return;
            }
            // Long previews (news, a sector list) scroll from the keyboard too.
            "pagedown" => {
                self.scroll_preview(240.0, cx);
                return;
            }
            "pageup" => {
                self.scroll_preview(-240.0, cx);
                return;
            }
            _ => {}
        }

        // Other chords (Super, ...) are not text.
        if alt || modifiers.platform {
            return;
        }

        if let Some(text) = keystroke
            .key_char
            .as_ref()
            .filter(|text| !text.chars().any(char::is_control))
        {
            self.replace_selection(text);
            self.edited(cx);
        }
    }

    /// Remember a launch so it shows up under Recent and ranks a little higher next time.
    fn record_launch(&mut self, hit: &SearchHit) {
        let recordable = matches!(
            hit.kind.as_str(),
            "application" | "file" | "folder" | "system" | "action"
        );

        if recordable && !is_dangerous(&hit.id) {
            self.history.record(
                &hit.id,
                &hit.title,
                hit.subtitle.as_deref(),
                &hit.kind,
                hit.icon.as_deref(),
            );

            let _ = self.history.save();
        }
    }

    /// Enter (or Ctrl+Enter when `reveal` is set: show a file in its folder).
    fn launch_selected(&mut self, reveal: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hit) = self.results.get(self.selected).cloned() else {
            return;
        };

        if is_inert(&hit.kind) {
            return;
        }

        if is_dangerous(&hit.id) && self.confirming.as_deref() != Some(hit.id.as_str()) {
            self.confirming = Some(hit.id);
            cx.notify();
            return;
        }

        self.record_launch(&hit);

        match hit.kind.as_str() {
            // Calculations are not launchable: copy the answer instead. The UI process stays
            // alive, so it keeps owning the Wayland clipboard after the window closes.
            "calculation" => {
                cx.write_to_clipboard(ClipboardItem::new_string(hit.title.clone()));
            }
            "clip" => {
                cx.write_to_clipboard(ClipboardItem::new_string(self.copy_text(&hit)));
            }
            "web" => {
                if let Some(url) = hit.id.strip_prefix("url:").filter(|url| is_web_url(url)) {
                    cx.open_url(url);
                }
            }
            "action" => {
                let _ = self.requests.try_send(UiRequest::OpenSettings);
            }
            "file" | "folder" if reveal => {
                if let Some(path) = hit.id.strip_prefix("files:open:") {
                    let _ = self
                        .requests
                        .try_send(UiRequest::Execute(format!("files:reveal:{path}")));
                }
            }
            _ => {
                let _ = self.requests.try_send(UiRequest::Execute(hit.id.clone()));
            }
        }

        window.remove_window();
    }

    // ---- Rendering ----

    /// A stroke icon from the embedded set, tinted `color`.
    fn glyph(&self, path: &'static str, size: f32, color: u32) -> impl IntoElement {
        svg()
            .path(path)
            .size(px(size))
            .flex_shrink_0()
            .text_color(rgb(color))
    }

    /// The icon tile of a result: the application's own icon when there is one, otherwise a
    /// colored rounded square with a glyph for its kind.
    fn leading(&self, hit: &SearchHit, tile: f32) -> AnyElement {
        if self.settings.appearance.show_icons {
            if let Some(icon) = hit.icon.as_deref() {
                return img(PathBuf::from(icon))
                    .size(px(tile))
                    .flex_shrink_0()
                    .into_any_element();
            }
        }

        // An application without an icon file: its initial on a stable color.
        if hit.kind == "application" {
            const PALETTE: [u32; 6] = [0x5e5ce6, 0x30a46c, 0xe5732d, 0xd6409f, 0x2f8fd8, 0x8e6bd8];

            let initial = hit
                .title
                .chars()
                .find(|c| c.is_alphanumeric())
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_owned());
            let sum: usize = hit.title.bytes().map(usize::from).sum();

            return div()
                .flex()
                .items_center()
                .justify_center()
                .size(px(tile))
                .flex_shrink_0()
                .rounded_lg()
                .bg(rgb(PALETTE[sum % PALETTE.len()]))
                .text_color(rgb(0xffffff))
                .text_lg()
                .child(initial)
                .into_any_element();
        }

        let color = tile_color(&hit.kind, &hit.id, self.settings.appearance.accent.rgb());

        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(tile))
            .flex_shrink_0()
            .rounded_lg()
            .bg(rgb(color))
            .child(self.glyph(icon_for_kind(&hit.kind, &hit.id), tile * 0.55, 0xffffff))
            .into_any_element()
    }

    fn keycap(&self, label: &str) -> impl IntoElement {
        div()
            .px_1()
            .rounded_md()
            .border_1()
            .border_color(rgba(self.palette.keycap_border))
            .bg(rgba(self.palette.subtle))
            .text_xs()
            .text_color(rgb(self.palette.text_dim))
            .child(label.to_owned())
    }

    fn render_row(&self, index: usize, hit: &SearchHit, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let selected = index == self.selected;
        let dangerous = is_dangerous(&hit.id);
        let confirming = self.confirming.as_deref() == Some(hit.id.as_str());
        let inert = is_inert(&hit.kind);
        let card = is_card(&hit.kind);

        let title_color = if selected {
            rgb(pal.on_accent)
        } else if inert {
            rgb(pal.text_dim)
        } else {
            rgb(pal.text)
        };
        let sub_color = if selected {
            rgba(pal.on_accent_dim)
        } else {
            rgb(pal.text_dim)
        };

        let base = div()
            .id(SharedString::from(format!("row-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .mx_2()
            .px_3()
            .rounded_xl()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.selected = index;
                this.launch_selected(false, window, cx);
            }))
            // Right-click: the actions menu at the pointer.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    let at = (f32::from(event.position.x), f32::from(event.position.y));

                    this.open_menu(index, Some(at), cx);
                }),
            );

        // Files and folders can be dragged out of the window into other applications.
        let base = match hit.id.strip_prefix("files:open:") {
            Some(path) if hit.kind == "file" || hit.kind == "folder" => {
                let dragged = DraggedFile {
                    path: PathBuf::from(path),
                    is_dir: hit.kind == "folder",
                    title: hit.title.clone(),
                };
                let accent = pal.accent;

                base.on_drag(dragged, move |dragged, _offset, _window, cx| {
                    let title = dragged.title.clone();

                    cx.new(|_| DragGhost { title, accent })
                })
                .external_drag_payload::<DraggedFile>(|dragged, _window, _cx| {
                    begin_external_drag();

                    Some(ExternalDragPayload::Files(FileDragPaths::new([(
                        dragged.path.clone(),
                        dragged.is_dir,
                    )])))
                })
            }
            _ => base,
        };

        let base = if selected {
            base.bg(rgb(if dangerous { pal.danger } else { pal.accent }))
        } else {
            base.hover(|style| style.bg(rgba(pal.hover)))
        };

        let (kind_label, _) = kind_meta(&hit.kind);
        let action_label = action_label_for(hit);

        let trailing = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .text_xs()
            .text_color(sub_color)
            .child(if selected {
                if confirming {
                    "Enter again"
                } else {
                    action_label
                }
            } else {
                kind_label
            })
            .when(selected && !inert, |this| {
                this.child(self.glyph(
                    "icons/enter.svg",
                    14.,
                    if selected {
                        pal.on_accent
                    } else {
                        pal.text_dim
                    },
                ))
            });

        if hit.kind == "calculation" {
            // The answer is the star, like Spotlight's calculator.
            return base
                .py_3()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(sub_color)
                                .child(hit.subtitle.clone().unwrap_or_else(|| "Answer".to_owned())),
                        )
                        .child(
                            div()
                                .text_3xl()
                                .text_color(title_color)
                                .child(hit.title.clone()),
                        ),
                )
                .child(trailing)
                .into_any_element();
        }

        if card {
            // Weather, quotes, scores and definitions: a larger row with the key facts.
            let first_definition = (hit.kind == "definition")
                .then(|| {
                    hit.detail
                        .as_deref()
                        .map(|detail| definition_parts(detail).0)
                })
                .flatten()
                .filter(|text| !text.is_empty());

            let subtitle_color = match (
                hit.kind.as_str(),
                hit.subtitle.as_deref().and_then(price_direction),
            ) {
                ("stock", Some(up)) if !selected => {
                    rgb(if up { pal.positive } else { pal.negative })
                }
                _ => sub_color,
            };

            return base
                .py_3()
                .child(self.leading(hit, 44.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .overflow_hidden()
                        .gap_0p5()
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(title_color)
                                .truncate()
                                .child(hit.title.clone()),
                        )
                        .when_some(hit.subtitle.clone(), |this, subtitle| {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(subtitle_color)
                                    .truncate()
                                    .child(subtitle),
                            )
                        })
                        .when_some(first_definition, |this, text| {
                            this.child(
                                div()
                                    .text_sm()
                                    .text_color(sub_color)
                                    .line_clamp(2)
                                    .child(text),
                            )
                        }),
                )
                .child(trailing)
                .into_any_element();
        }

        let subtitle = if confirming {
            Some("Press Enter again to confirm".to_owned())
        } else {
            hit.subtitle.clone()
        };

        // Clipboard rows get a visible copy button, like Spotlight's.
        let copy_button = (hit.kind == "clip").then(|| {
            rounded_px(div(), 14.)
                .id(SharedString::from(format!("clip-copy-{index}")))
                .flex()
                .items_center()
                .justify_center()
                .size(px(28.))
                .flex_shrink_0()
                .bg(rgba(if selected { 0xffffff33 } else { pal.pill }))
                .hover(|style| style.bg(rgba(pal.hover)))
                .child(self.glyph(
                    "icons/clipboard.svg",
                    14.,
                    if selected {
                        pal.on_accent
                    } else {
                        pal.text_dim
                    },
                ))
                .on_click(cx.listener(move |this, _event, window, cx| {
                    cx.stop_propagation();
                    this.selected = index;
                    this.launch_selected(false, window, cx);
                }))
        });

        base.py_2()
            .child(self.leading(hit, 32.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .text_base()
                            .text_color(title_color)
                            .truncate()
                            .child(hit.title.clone()),
                    )
                    .when_some(subtitle, |this, subtitle| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(sub_color)
                                .truncate()
                                .child(subtitle),
                        )
                    }),
            )
            .child(trailing)
            .when_some(copy_button, |this, button| this.child(button))
            .into_any_element()
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let preview = self.preview.as_ref()?;
        let hit = self.results.get(self.selected)?;
        let pal = self.palette;

        if is_inert(&hit.kind) {
            return None;
        }

        let label = |text: &str| {
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(pal.text_faint))
                .child(text.to_owned())
        };

        // Kind-specific body, built from the daemon's `detail` text.
        let mut body: Vec<AnyElement> = Vec::new();

        match (hit.kind.as_str(), hit.detail.as_deref()) {
            ("definition", Some(detail)) => {
                for line in definition_parts(detail).1 {
                    let example = line.starts_with("   ");

                    body.push(
                        div()
                            .text_sm()
                            .text_color(rgb(if example { pal.text_dim } else { pal.text }))
                            .when(example, |this| this.italic())
                            .child(line.trim_start().to_owned())
                            .into_any_element(),
                    );
                }
            }
            ("weather", Some(detail)) => {
                let (facts, days) = weather_parts(detail);

                for fact in facts {
                    body.push(
                        div()
                            .text_sm()
                            .text_color(rgb(pal.text))
                            .child(fact)
                            .into_any_element(),
                    );
                }

                if !days.is_empty() {
                    body.push(label("FORECAST").into_any_element());
                }

                for (name, condition, high, low) in days {
                    body.push(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .w_full()
                            .text_sm()
                            .child(
                                div()
                                    .w(px(42.))
                                    .flex_shrink_0()
                                    .text_color(rgb(pal.text))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .text_color(rgb(pal.text_dim))
                                    .child(condition),
                            )
                            .child(div().flex_shrink_0().text_color(rgb(pal.text)).child(high))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .w(px(34.))
                                    .text_right()
                                    .text_color(rgb(pal.text_faint))
                                    .child(low),
                            )
                            .into_any_element(),
                    );
                }
            }
            ("stock", Some(detail)) => {
                for (name, value) in stock_rows(detail) {
                    body.push(
                        div()
                            .flex()
                            .flex_col()
                            .child(label(&name))
                            .child(div().text_sm().text_color(rgb(pal.text)).child(value))
                            .into_any_element(),
                    );
                }
            }
            ("sports", Some(detail)) => {
                if let Some(board) = scoreboard(detail) {
                    let team = |name: String, score: String, won: bool| {
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .text_sm()
                                    .font_weight(if won {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(rgb(if won { pal.text } else { pal.text_dim }))
                                    .child(name),
                            )
                            .child(div().text_xl().text_color(rgb(pal.text)).child(score))
                    };

                    let away_won = board.winner == "away";
                    let home_won = board.winner == "home";

                    body.push(team(board.away, board.away_score, away_won).into_any_element());
                    body.push(team(board.home, board.home_score, home_won).into_any_element());
                }
            }
            _ => {}
        }

        // Answer cards (definitions, weather, stocks, scores) keep a compact header with their
        // details below; everything else gets a Spotlight-style preview.
        let rich = matches!(
            hit.kind.as_str(),
            "definition" | "weather" | "stock" | "sports"
        );

        let header: AnyElement = if rich {
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(self.leading(hit, 56.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(pal.text))
                                .child(preview.title.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_dim))
                                .child(preview.badge.clone()),
                        ),
                )
                .into_any_element()
        } else {
            // The big preview: the image or PDF page, the start of a text file, or a large icon.
            let hero: AnyElement = if let Some(path) = preview.image.clone() {
                rounded_px(div(), 12.)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w_full()
                    .h(px(176.))
                    .overflow_hidden()
                    .bg(rgba(pal.subtle))
                    .child(img(path).w_full().max_h(px(176.)))
                    .into_any_element()
            } else if let Some(text) = preview.text.clone() {
                rounded_px(div(), 12.)
                    .w_full()
                    .h(px(176.))
                    .p_3()
                    .overflow_hidden()
                    .bg(rgba(pal.subtle))
                    .text_xs()
                    .text_color(rgb(pal.text_dim))
                    .child(text)
                    .into_any_element()
            } else {
                rounded_px(div(), 12.)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w_full()
                    .h(px(132.))
                    .bg(rgba(pal.subtle))
                    .child(self.leading(hit, 84.))
                    .into_any_element()
            };

            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(hero)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap_0p5()
                        .px_2()
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::MEDIUM)
                                .text_center()
                                .text_color(rgb(pal.text))
                                .line_clamp(2)
                                .child(preview.title.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(pal.text_dim))
                                .child(preview.badge.clone()),
                        ),
                )
                .into_any_element()
        };

        // "Information": label on the left, value on the right.
        let info = (!preview.rows.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .pt_3()
                .border_t_1()
                .border_color(rgba(pal.divider))
                .child(label("Information"))
                .children(preview.rows.iter().map(|(name, value)| {
                    div()
                        .flex()
                        .flex_row()
                        .gap_3()
                        .text_sm()
                        .child(
                            div()
                                .w(px(72.))
                                .flex_shrink_0()
                                .text_color(rgb(pal.text_faint))
                                .child((*name).to_owned()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_color(rgb(pal.text))
                                .truncate()
                                .child(value.clone()),
                        )
                }))
        });

        let contents = (!preview.entries.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .pt_3()
                .border_t_1()
                .border_color(rgba(pal.divider))
                .child(label("Contents"))
                .children(preview.entries.iter().map(|name| {
                    div()
                        .text_xs()
                        .text_color(rgb(pal.text_dim))
                        .truncate()
                        .child(name.clone())
                }))
        });

        // Open / Show in Folder, like Quick Look's buttons.
        let button = |id: &'static str,
                      text: &'static str,
                      primary: bool,
                      reveal: bool,
                      cx: &mut Context<Self>| {
            rounded_px(div(), 8.)
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .flex_1()
                .h(px(32.))
                .cursor_pointer()
                .text_sm()
                .when(primary, |this| {
                    this.bg(rgb(pal.accent)).text_color(rgb(pal.on_accent))
                })
                .when(!primary, |this| {
                    this.bg(rgba(pal.subtle))
                        .border_1()
                        .border_color(rgba(pal.border))
                        .text_color(rgb(pal.text))
                        .hover(|style| style.bg(rgba(pal.hover)))
                })
                .on_click(cx.listener(move |this, _event, window, cx| {
                    this.launch_selected(reveal, window, cx);
                }))
                .child(text)
        };

        let is_file = matches!(hit.kind.as_str(), "file" | "folder");
        let launchable = is_file || hit.kind == "application";

        let buttons = launchable.then(|| {
            div()
                .flex()
                .flex_row()
                .gap_2()
                .pt_1()
                .child(button(
                    "preview-open",
                    kind_meta(&hit.kind).1,
                    true,
                    false,
                    cx,
                ))
                .when(is_file, |this| {
                    this.child(button("preview-reveal", "Show in Folder", false, true, cx))
                })
        });

        // The pane scrolls; its content keeps its natural height (flex_shrink_0) and overflows
        // the pane's maximum height instead of being squeezed or cut off.
        Some(
            div()
                .id("preview")
                .flex()
                .flex_col()
                .w(px(304.))
                .max_w_full()
                .min_h(px(0.))
                .flex_shrink_0()
                .overflow_y_scroll()
                .track_scroll(&self.preview_scroll)
                .border_l_1()
                .border_color(rgba(pal.divider))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_shrink_0()
                        .gap_3()
                        .w_full()
                        .p_4()
                        .child(header)
                        .children(info)
                        .children(contents)
                        .children(body)
                        .children(buttons),
                )
                .into_any_element(),
        )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let mut hints: Vec<(&'static str, &'static str)> = Vec::new();

        match self
            .results
            .get(self.selected)
            .filter(|hit| !is_inert(&hit.kind))
        {
            Some(hit) => {
                hints.push(("Enter", action_label_for(hit)));

                match hit.kind.as_str() {
                    "file" | "folder" => {
                        hints.push(("Ctrl+Enter", "Show in folder"));
                        hints.push(("Drag", "Share file"));
                    }
                    _ => hints.push(("Ctrl+C", "Copy")),
                }

                hints.push(("Ctrl+K", "Actions"));

                if self.settings.plugins.dictionary {
                    hints.push(("Ctrl+L", "Look up"));
                }

                // Only worth a hint when the preview actually has more to show.
                if f32::from(self.preview_scroll.max_offset().y).abs() > 0.5 {
                    hints.push(("PgUp/PgDn", "Scroll preview"));
                }
            }
            None => {
                hints.push(("define word", "Dictionary"));
                hints.push(("weather city", "Weather"));
                hints.push(("stock aapl", "Stocks"));
                if self.settings.plugins.nepse {
                    hints.push(("nepse nabil", "NEPSE"));
                }
                hints.push(("nba", "Scores"));
                hints.push(("< name", "Windows"));
            }
        }

        if self.grid_active() {
            hints.push(("\u{2190}\u{2191}\u{2193}\u{2192}", "Move"));
        }

        if self.has_chips() {
            hints.push(("Alt+\u{2190}\u{2192}", "Filter"));
        }

        if self.scope.is_some() && self.query.is_empty() {
            hints.push(("Backspace", "Back"));
        }

        hints.push(("Esc", "Close"));

        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x_4()
            .gap_y_1()
            .px_5()
            .py_2()
            .border_t_1()
            .border_color(rgba(pal.divider))
            .when(self.settings.appearance.launcher_movable, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _event, window, _cx| begin_window_move(window)),
                )
            })
            .children(hints.into_iter().map(|(keys, label)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(self.keycap(keys))
                    .child(div().text_xs().text_color(rgb(pal.text_faint)).child(label))
            }))
    }

    /// The right-click / Ctrl+K actions menu, with a transparent backdrop that dismisses it.
    fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let pal = self.palette;

        let rows = menu.items.iter().enumerate().map(|(index, item)| {
            let action = item.action;
            let selected = index == menu.selected;

            div()
                .id(SharedString::from(format!("menu-{index}")))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_4()
                .mx_1()
                .px_3()
                .h(px(MENU_ROW_H))
                .rounded_md()
                .cursor_pointer()
                .when(selected, |this| {
                    this.bg(rgb(pal.accent)).text_color(rgb(pal.on_accent))
                })
                .when(!selected, |this| {
                    this.text_color(rgb(pal.text))
                        .hover(|style| style.bg(rgba(pal.hover)))
                })
                .on_click(cx.listener(move |this, _event, window, cx| {
                    this.perform(action, window, cx);
                }))
                .child(div().text_sm().child(item.label.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(if selected {
                            rgba(pal.on_accent_dim)
                        } else {
                            rgb(pal.text_faint)
                        })
                        .child(item.shortcut),
                )
        });

        let dismiss = |this: &mut Self,
                       _event: &MouseDownEvent,
                       _window: &mut Window,
                       cx: &mut Context<Self>| {
            this.close_menu(cx);
        };

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(px(WINDOW_H))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .on_mouse_down(MouseButton::Left, cx.listener(dismiss))
                        .on_mouse_down(MouseButton::Right, cx.listener(dismiss)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(menu.x))
                        .top(px(menu.y))
                        .w(px(MENU_W))
                        .py_1()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgba(pal.border))
                        .bg(rgba(pal.menu))
                        .occlude()
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// The search field: one element per character so the mouse can place the caret
    /// (each character has a left and a right half), select by dragging, and double-click
    /// a word or triple-click everything.
    fn render_query(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;

        let caret = || {
            div()
                .w(px(2.))
                .h(px(26.))
                .flex_shrink_0()
                .bg(rgb(pal.accent))
        };

        let field = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .h(px(36.))
            .overflow_hidden()
            .cursor_text()
            .text_2xl();

        // Anywhere past the last character: caret to the end (double-click: select all).
        let filler = div()
            .flex_1()
            .h_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    let end = this.query.len();

                    // The search field edits text; only the bar around it moves the window.
                    cx.stop_propagation();
                    this.mouse_down_at(
                        end,
                        if event.click_count >= 2 { 3 } else { 1 },
                        event.modifiers.shift,
                        cx,
                    );
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let end = this.query.len();

                this.mouse_drag_to(end, event.pressed_button == Some(MouseButton::Left), cx);
            }));

        let token = self.scope_token();

        // "Applications" waits in the empty field, like a placeholder, until you type; once a
        // chip is picked, its token stands in front of the caret instead.
        if let Some(quick) = self.scope.filter(|_| self.query.is_empty()) {
            return match token {
                Some(token) => field.child(token).child(caret()),
                None => field.child(caret()).child(
                    div()
                        .pl_1()
                        .text_color(rgb(pal.text_dim))
                        .child(quick.title()),
                ),
            }
            .child(filler)
            .into_any_element();
        }

        if self.query.is_empty() {
            return field
                .child(caret())
                .child(
                    div()
                        .pl_1()
                        .text_color(rgb(pal.text_dim))
                        .child("Search apps, files, windows, weather, math\u{2026}"),
                )
                .child(filler)
                .into_any_element();
        }

        let (selection_start, selection_end) = self.selection();
        let show_caret = !self.has_selection();

        // One half-character hit area: absolute, transparent, covers the left or right half.
        let hit_area = |index: usize, left_half: bool| {
            div()
                .absolute()
                .top_0()
                .h_full()
                .w_1_2()
                .when(left_half, |this| this.left_0())
                .when(!left_half, |this| this.right_0())
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                        cx.stop_propagation();
                        this.mouse_down_at(index, event.click_count, event.modifiers.shift, cx);
                    }),
                )
                .on_mouse_move(
                    cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                        this.mouse_drag_to(
                            index,
                            event.pressed_button == Some(MouseButton::Left),
                            cx,
                        );
                    }),
                )
        };

        let ghost = self
            .bar_hit()
            .and_then(|hit| ghost_text(&self.query, hit))
            .map(|text| {
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .text_lg()
                    .text_color(rgb(pal.text_faint))
                    .child(text)
                    .into_any_element()
            });

        let mut parts: Vec<AnyElement> = Vec::new();

        for (start, c) in self.query.char_indices() {
            let end = start + c.len_utf8();

            if show_caret && start == self.cursor {
                parts.push(caret().into_any_element());
            }

            let selected =
                start >= selection_start && end <= selection_end && selection_start < selection_end;

            parts.push(
                div()
                    .relative()
                    .flex_shrink_0()
                    .text_color(rgb(pal.text))
                    .when(selected, |this| this.bg(rgba(pal.selection)))
                    .child(c.to_string())
                    .child(hit_area(start, true))
                    .child(hit_area(end, false))
                    .into_any_element(),
            );
        }

        if show_caret && self.cursor >= self.query.len() {
            parts.push(caret().into_any_element());
        }

        field
            .children(token)
            .children(parts)
            .children(ghost)
            .child(filler)
            .into_any_element()
    }
}

// ---- Quick buttons and chips ------------------------------------------------------------

impl LauncherView {
    /// The quick button whose scope is on, if any.
    fn active_quick(&self) -> Option<Quick> {
        self.scope
    }

    /// What the daemon is asked: the typed text, behind the scope's prefix (`kind:app fire`).
    fn search_text(&self) -> String {
        scoped_query(self.scope, self.file_kind, &self.query)
    }

    fn set_scope(&mut self, quick: Quick, cx: &mut Context<Self>) {
        self.scope = Some(quick);
        self.chip = None;
        self.group = None;
        self.file_kind = None;
        self.edited(cx);
    }

    fn clear_scope(&mut self, cx: &mut Context<Self>) {
        self.scope = None;
        self.chip = None;
        self.group = None;
        self.file_kind = None;
        self.edited(cx);
    }

    /// A round button was pressed: type its query, or clear it when it is already showing.
    fn quick_action(&mut self, quick: Quick, cx: &mut Context<Self>) {
        if self.scope == Some(quick) {
            self.clear_scope(cx);
            return;
        }

        if quick == Quick::Clipboard && !self.settings.plugins.clipboard {
            self.notice = Some(
                "Clipboard history is off. Turn it on in Settings, under Search plugins."
                    .to_owned(),
            );
            cx.notify();
            return;
        }

        self.set_scope(quick, cx);
    }

    fn render_quick_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;
        let active = self.active_quick();

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .children(Quick::ALL.into_iter().map(|quick| {
                let on = active == Some(quick);

                rounded_px(div(), 26.)
                    .id(SharedString::from(format!("quick-{quick:?}")))
                    .tooltip(self.hover_tip(quick.hint(on)))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(52.))
                    .flex_shrink_0()
                    .border_1()
                    .cursor_pointer()
                    .when(on, |this| {
                        this.bg(rgba(pal.accent_soft)).border_color(rgb(pal.accent))
                    })
                    .when(!on, |this| {
                        this.bg(rgba(pal.card))
                            .border_color(rgba(pal.border))
                            .hover(|style| style.bg(rgba(pal.accent_soft)))
                    })
                    .child(self.glyph(
                        quick.icon(),
                        22.,
                        if on { pal.accent_fg } else { pal.text_dim },
                    ))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.quick_action(quick, cx);
                    }))
            }))
    }

    /// The sections present in the last search that can be filtered to, in display order.
    fn chip_sections(&self) -> Vec<Section> {
        let mut found: Vec<Section> = Vec::new();

        for hit in &self.all_hits {
            let section = section_of(&hit.kind);

            if chip_label(section).is_some() && !found.contains(&section) {
                found.push(section);
            }
        }

        found.sort_by_key(|section| *section as usize);
        found
    }

    /// Narrow the results to one kind (`None` is "All").
    fn select_chip(&mut self, chip: Option<Section>, cx: &mut Context<Self>) {
        self.chip = chip;

        let hits = self.all_hits.clone();
        let sections = self.all_sections.clone();

        self.install(hits, sections);
        cx.notify();
    }

    /// A tooltip builder in the current palette.
    fn hover_tip(&self, text: &'static str) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let pal = self.palette;

        move |_window, cx| {
            cx.new(|_| HoverTip {
                text: text.into(),
                bg: pal.menu,
                fg: pal.text,
                border: pal.border,
            })
            .into()
        }
    }

    /// The group chips of the Applications grid that apply to the current results.
    fn present_groups(&self) -> Vec<&'static str> {
        APP_GROUPS
            .into_iter()
            .filter(|group| {
                self.all_hits
                    .iter()
                    .any(|hit| app_group(hit) == Some(*group))
            })
            .collect()
    }

    /// Whether chips are showing under the search bar (so Alt+Left/Right has something to do).
    fn has_chips(&self) -> bool {
        if self.scope == Some(Quick::Files) {
            return true;
        }

        if self.is_grid_list(&self.all_hits) {
            return self.present_groups().len() >= 2;
        }

        self.chip_sections().len() >= 2
    }

    /// The chip you picked, shown inside the search field as a blue token ("PDFs", "Internet")
    /// ahead of the text, like Spotlight's.
    fn scope_token(&self) -> Option<AnyElement> {
        let pal = self.palette;

        let (icon, label) = match self.scope? {
            Quick::Files => {
                let kind = self.file_kind?;
                let (label, _, icon) = FILE_KINDS.iter().find(|(_, k, _)| *k == kind)?;

                (*icon, *label)
            }
            Quick::Applications => {
                let group = self.group.as_deref()?;
                let label = *APP_GROUPS.iter().find(|g| **g == group)?;

                (group_icon(label), label)
            }
            _ => return None,
        };

        Some(
            rounded_px(div(), 9.)
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_shrink_0()
                .mr_2()
                .px_2()
                .h(px(30.))
                .bg(rgba(pal.accent_soft))
                .text_color(rgb(pal.accent_fg))
                .text_lg()
                .child(self.glyph(icon, 16., pal.accent_fg))
                .child(label)
                .into_any_element(),
        )
    }

    /// Backspace on an empty field removes the token first (the chip goes back to "All"), then
    /// the scope. `false` when there was no token.
    fn remove_token(&mut self, cx: &mut Context<Self>) -> bool {
        if self.scope == Some(Quick::Files) && self.file_kind.is_some() {
            self.select_file_kind(None, cx);
            return true;
        }

        if self.scope == Some(Quick::Applications) && self.group.is_some() {
            self.select_group(None, cx);
            return true;
        }

        false
    }

    /// Alt+Left/Right: the next or previous chip, wrapping around. `false` when no chips show.
    fn cycle_chip(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        if self.scope == Some(Quick::Files) {
            let current = FILE_KINDS
                .iter()
                .position(|(_, kind, _)| Some(*kind) == self.file_kind)
                .map_or(0, |index| index + 1);
            let next = cycle_index(FILE_KINDS.len() + 1, current, delta);

            self.select_file_kind(next.checked_sub(1).map(|index| FILE_KINDS[index].1), cx);
            return true;
        }

        if self.is_grid_list(&self.all_hits) {
            let groups = self.present_groups();

            if groups.len() < 2 {
                return false;
            }

            let current = groups
                .iter()
                .position(|group| Some(*group) == self.group.as_deref())
                .map_or(0, |index| index + 1);
            let next = cycle_index(groups.len() + 1, current, delta);

            self.select_group(
                next.checked_sub(1).map(|index| groups[index].to_owned()),
                cx,
            );
            return true;
        }

        let sections = self.chip_sections();

        if sections.len() < 2 {
            return false;
        }

        let current = sections
            .iter()
            .position(|section| Some(*section) == self.chip)
            .map_or(0, |index| index + 1);
        let next = cycle_index(sections.len() + 1, current, delta);

        self.select_chip(next.checked_sub(1).map(|index| sections[index]), cx);
        true
    }

    /// The result Enter would open, when the bar should speak for it (grey title after the
    /// text, its icon at the right end).
    fn bar_hit(&self) -> Option<&SearchHit> {
        if self.query.trim().is_empty() {
            return None;
        }

        self.results
            .get(self.selected)
            .filter(|hit| speaks_in_the_bar(hit))
    }

    /// The quick button whose scope is on (the bar shows its icon instead of the magnifier).
    fn scoped_quick(&self) -> Option<Quick> {
        self.scope
    }

    /// Whether `hits` should be drawn as the Applications grid.
    fn is_grid_list(&self, hits: &[SearchHit]) -> bool {
        self.scoped_quick() == Some(Quick::Applications)
            && !hits.is_empty()
            && hits.iter().all(|hit| hit.kind == "application")
    }

    /// Whether `hits` should be drawn as the Files grid: the Files view with nothing typed and
    /// no file type chosen (the recent files). Typing or a chip turns it into the detailed list.
    fn is_files_grid(&self, hits: &[SearchHit]) -> bool {
        self.scope == Some(Quick::Files)
            && self.file_kind.is_none()
            && self.query.trim().is_empty()
            && !apps_as_list()
            && !hits.is_empty()
            && hits
                .iter()
                .all(|hit| matches!(hit.kind.as_str(), "file" | "folder"))
    }

    /// Results are drawn as a grid of tiles (Applications, or the recent files), not as a list
    /// ("View as List" in the "..." menu turns the grids off).
    fn grid_active(&self) -> bool {
        !apps_as_list() && (self.is_grid_list(&self.results) || self.is_files_grid(&self.results))
    }

    /// Move the selection across (`+-1`) or down (`+-GRID_COLS`) the tiles, without wrapping.
    fn move_grid_selection(&mut self, step: isize, cx: &mut Context<Self>) {
        let last = self.results.len() as isize - 1;

        if last >= 0 {
            self.selected = (self.selected as isize + step).clamp(0, last) as usize;
            self.confirming = None;
            self.refresh_preview();
            self.scroll_to_selected();
            cx.notify();
        }
    }

    /// One application of the grid: its icon above its name.
    fn render_tile(&self, index: usize, hit: &SearchHit, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let selected = index == self.selected;
        let is_file = matches!(hit.kind.as_str(), "file" | "folder");

        div()
            .id(SharedString::from(format!("tile-{index}")))
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .flex_1()
            .min_w(px(0.))
            .px_1()
            .py_2()
            .rounded_xl()
            .cursor_pointer()
            .when(selected, |this| this.bg(rgb(pal.accent)))
            .when(!selected, |this| {
                this.hover(|style| style.bg(rgba(pal.hover)))
            })
            .child(self.tile_icon(hit))
            .child(
                div()
                    .w_full()
                    .text_xs()
                    .text_center()
                    // File names get two lines; an application's name fits on one.
                    .when(is_file, |this| this.line_clamp(2))
                    .when(!is_file, |this| this.truncate())
                    .text_color(if selected {
                        rgb(pal.on_accent)
                    } else {
                        rgb(pal.text)
                    })
                    .child(hit.title.clone()),
            )
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.selected = index;
                this.launch_selected(false, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    let at = (f32::from(event.position.x), f32::from(event.position.y));

                    this.open_menu(index, Some(at), cx);
                }),
            )
            .into_any_element()
    }

    /// The picture of a tile: a picture file is its own thumbnail, anything else shows its icon.
    fn tile_icon(&self, hit: &SearchHit) -> AnyElement {
        if self.settings.appearance.show_icons && hit.kind == "file" && is_image_name(&hit.title) {
            if let Some(path) = hit.id.strip_prefix("files:open:") {
                return img(PathBuf::from(path))
                    .size(px(52.))
                    .rounded_md()
                    .flex_shrink_0()
                    .into_any_element();
            }
        }

        self.leading(
            hit,
            if matches!(hit.kind.as_str(), "file" | "folder") {
                52.
            } else {
                48.
            },
        )
    }

    /// A titled block of tiles, `GRID_COLS` to a row: the results `range` of the list.
    fn push_grid_section(
        &self,
        rows: &mut Vec<AnyElement>,
        title: &'static str,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) {
        let pal = self.palette;
        let start = range.start;

        rows.push(
            div()
                .px_5()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(pal.text_faint))
                .child(title)
                .into_any_element(),
        );

        for (row, chunk) in self.results[range].chunks(GRID_COLS).enumerate() {
            let mut line = div().flex().flex_row().gap_1().px_3();

            for (offset, hit) in chunk.iter().enumerate() {
                line = line.child(self.render_tile(start + row * GRID_COLS + offset, hit, cx));
            }

            // Keep the last row's tiles the same width as the others.
            for _ in chunk.len()..GRID_COLS {
                line = line.child(div().flex_1().min_w(px(0.)));
            }

            rows.push(line.into_any_element());
        }
    }

    /// The grids: all the applications, or the recent files in "Suggestions" and "Recents".
    fn render_grid(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = Vec::new();

        if self.is_files_grid(&self.results) {
            let suggested = self.suggested.min(self.results.len());

            if suggested > 0 {
                self.push_grid_section(&mut rows, "Suggestions", 0..suggested, cx);
            }

            if self.results.len() > suggested {
                self.push_grid_section(&mut rows, "Recents", suggested..self.results.len(), cx);
            }
        } else {
            self.push_grid_section(&mut rows, "Applications", 0..self.results.len(), cx);
        }

        rows
    }

    /// Narrow the Applications grid to one group (`None` is "All").
    fn select_group(&mut self, group: Option<String>, cx: &mut Context<Self>) {
        self.group = group;

        let hits = self.all_hits.clone();
        let sections = self.all_sections.clone();

        self.install(hits, sections);
        cx.notify();
    }

    /// "All  Internet  Productivity ...": the group chips of the Applications grid.
    fn render_group_chips(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let present: Vec<&'static str> = APP_GROUPS
            .into_iter()
            .filter(|group| {
                self.all_hits
                    .iter()
                    .any(|hit| app_group(hit) == Some(*group))
            })
            .collect();

        if present.len() < 2 {
            return None;
        }

        let pal = self.palette;
        let mut entries: Vec<(Option<&'static str>, &'static str)> = vec![(None, "All")];

        entries.extend(present.into_iter().map(|group| (Some(group), group)));

        let chips: Vec<AnyElement> = entries
            .into_iter()
            .map(|(group, label)| {
                let on = self.group.as_deref() == group;
                let tint = if on { pal.accent_fg } else { pal.text_dim };

                div()
                    .id(SharedString::from(format!("group-{label}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .text_sm()
                    .cursor_pointer()
                    .when(on, |this| {
                        this.bg(rgba(pal.accent_soft))
                            .text_color(rgb(pal.accent_fg))
                    })
                    .when(!on, |this| {
                        this.bg(rgba(pal.pill))
                            .text_color(rgb(pal.text_dim))
                            .hover(|style| style.bg(rgba(pal.hover)))
                    })
                    .when_some(group, |this, group| {
                        this.child(self.glyph(group_icon(group), 14., tint))
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.select_group(group.map(str::to_owned), cx);
                    }))
                    .into_any_element()
            })
            .collect();

        Some(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_2()
                .px_4()
                .pt_3()
                .pb_2()
                .border_b_1()
                .border_color(rgba(pal.divider))
                .children(chips)
                .into_any_element(),
        )
    }

    // ---- The "..." menu, the tips panel and the "Try" chips ----

    /// Whether a tip makes sense with the plugins that are switched on.
    fn tip_enabled(&self, tip: &Tip) -> bool {
        let plugins = &self.settings.plugins;

        match tip.plugin {
            None => true,
            Some("files") => plugins.files,
            Some("calculator") => plugins.calculator,
            Some("units") => plugins.units,
            Some("system") => plugins.system,
            Some("windows") => plugins.windows,
            Some("dictionary") => plugins.dictionary,
            Some("weather") => plugins.weather,
            Some("nexdesk") => plugins.nexdesk,
            Some("clipboard") => plugins.clipboard,
            Some(_) => true,
        }
    }

    /// A tip was clicked: leave any scope and put its example in the search field.
    fn use_tip(&mut self, fill: &str, cx: &mut Context<Self>) {
        self.scope = None;
        self.chip = None;
        self.group = None;
        self.file_kind = None;
        self.set_query(fill, cx);
    }

    /// Run the same search again so the list is laid out afresh (grid or list).
    fn relayout(&mut self) {
        let hits = self.all_hits.clone();
        let sections = self.all_sections.clone();

        self.install(hits, sections);
    }

    fn more_action(&mut self, action: MoreAction, window: &mut Window, cx: &mut Context<Self>) {
        self.more = false;

        match action {
            MoreAction::Grid => {
                APPS_AS_LIST.store(false, Ordering::Relaxed);
                self.relayout();
            }
            MoreAction::List => {
                APPS_AS_LIST.store(true, Ordering::Relaxed);
                self.relayout();
            }
            MoreAction::Tips => self.tips = true,
            MoreAction::Settings => {
                let _ = self.requests.try_send(UiRequest::OpenSettings);
                window.remove_window();
                return;
            }
        }

        cx.notify();
    }

    /// The "..." menu under the search bar: view options of the current view, tips, settings.
    fn render_more(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.more {
            return None;
        }

        let pal = self.palette;
        let mut items: Vec<(MoreAction, &'static str, bool)> = Vec::new();

        if matches!(self.scope, Some(Quick::Applications | Quick::Files)) {
            items.push((MoreAction::Grid, "View as Grid", !apps_as_list()));
            items.push((MoreAction::List, "View as List", apps_as_list()));
        }

        items.push((MoreAction::Tips, "Tips & shortcuts", false));
        items.push((MoreAction::Settings, "Settings", false));

        let rows: Vec<AnyElement> = items
            .into_iter()
            .enumerate()
            .map(|(index, (action, label, checked))| {
                div()
                    .id(SharedString::from(format!("more-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .mx_1()
                    .px_3()
                    .h(px(MENU_ROW_H))
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(rgb(pal.text))
                    .hover(|style| style.bg(rgba(pal.hover)))
                    .child(
                        div()
                            .w(px(14.))
                            .flex_shrink_0()
                            .text_sm()
                            .text_color(rgb(pal.accent_fg))
                            .child(if checked { "\u{2713}" } else { "" }),
                    )
                    .child(div().text_sm().child(label))
                    .on_click(cx.listener(move |this, _event, window, cx| {
                        this.more_action(action, window, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let dismiss = |this: &mut Self,
                       _event: &MouseDownEvent,
                       _window: &mut Window,
                       cx: &mut Context<Self>| {
            this.more = false;
            cx.notify();
        };

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w_full()
                .h(px(WINDOW_H))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .on_mouse_down(MouseButton::Left, cx.listener(dismiss))
                        .on_mouse_down(MouseButton::Right, cx.listener(dismiss)),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(60.))
                        .right(px(256.))
                        .w(px(MENU_W))
                        .py_1()
                        .rounded_lg()
                        .border_1()
                        .border_color(rgba(pal.border))
                        .bg(rgba(pal.menu))
                        .occlude()
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    /// Everything you can type and every key worth knowing, with examples that run on a click.
    fn render_tips(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;

        let tips: Vec<AnyElement> = TIPS
            .iter()
            .filter(|tip| self.tip_enabled(tip))
            .enumerate()
            .map(|(index, tip)| {
                let fill = tip.fill;

                div()
                    .id(SharedString::from(format!("tip-{index}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .mx_2()
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(pal.hover)))
                    .child(div().text_sm().text_color(rgb(pal.text)).child(tip.title))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(pal.text_faint))
                            .child(tip.example),
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.use_tip(fill, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        let keys: Vec<AnyElement> = SHORTCUTS
            .iter()
            .map(|(keys, what)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(self.keycap(keys))
                    .child(div().text_xs().text_color(rgb(pal.text_dim)).child(*what))
                    .into_any_element()
            })
            .collect();

        let section = |title: &'static str| {
            div()
                .px_5()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(pal.text_faint))
                .child(title)
        };

        div()
            .id("tips")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .pb_3()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_5()
                    .pt_4()
                    .pb_1()
                    .child(
                        div()
                            .flex_1()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(pal.text))
                            .child("Tips & shortcuts"),
                    )
                    .child(
                        div()
                            .id("tips-close")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .text_sm()
                            .cursor_pointer()
                            .bg(rgba(pal.pill))
                            .text_color(rgb(pal.text_dim))
                            .hover(|style| style.bg(rgba(pal.hover)))
                            .child("Close")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.tips = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(section("Click one to try it"))
            .children(tips)
            .child(section("Keyboard"))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap_x_5()
                    .gap_y_2()
                    .px_5()
                    .pt_1()
                    .children(keys),
            )
            .into_any_element()
    }

    /// Clickable examples for an empty launcher with nothing recent to show: no commands to
    /// remember.
    fn render_try(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;

        let chips: Vec<AnyElement> = TIPS
            .iter()
            .filter(|tip| self.tip_enabled(tip))
            .take(8)
            .enumerate()
            .map(|(index, tip)| {
                let fill = tip.fill;

                div()
                    .id(SharedString::from(format!("try-{index}")))
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .text_sm()
                    .cursor_pointer()
                    .bg(rgba(pal.pill))
                    .text_color(rgb(pal.text_dim))
                    .hover(|style| style.bg(rgba(pal.hover)))
                    .child(tip.title)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.use_tip(fill, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_5()
            .py_4()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(pal.text_faint))
                    .child("Try one of these, or just start typing"),
            )
            .child(div().flex().flex_row().flex_wrap().gap_2().children(chips))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(pal.text_faint))
                    .child("Press F1 or use the \u{22ef} menu for all tips and shortcuts."),
            )
            .into_any_element()
    }

    /// Narrow the Files view to one kind of file (`None` is "All").
    fn select_file_kind(&mut self, kind: Option<&'static str>, cx: &mut Context<Self>) {
        self.file_kind = kind;
        self.edited(cx);
    }

    /// "All  Documents  PDFs  Images ...": the file-type chips of the Files view. They search
    /// again (`kind:pdf`) rather than filtering what is already on screen.
    fn render_file_chips(&self, cx: &mut Context<Self>) -> AnyElement {
        let pal = self.palette;
        let mut entries: Vec<(Option<&'static str>, &'static str, Option<&'static str>)> =
            vec![(None, "All", None)];

        entries.extend(
            FILE_KINDS
                .iter()
                .map(|(label, kind, icon)| (Some(*kind), *label, Some(*icon))),
        );

        let chips: Vec<AnyElement> = entries
            .into_iter()
            .map(|(kind, label, icon)| {
                let on = self.file_kind == kind;
                let tint = if on { pal.accent_fg } else { pal.text_dim };

                div()
                    .id(SharedString::from(format!("filekind-{label}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .text_sm()
                    .cursor_pointer()
                    .when(on, |this| {
                        this.bg(rgba(pal.accent_soft))
                            .text_color(rgb(pal.accent_fg))
                    })
                    .when(!on, |this| {
                        this.bg(rgba(pal.pill))
                            .text_color(rgb(pal.text_dim))
                            .hover(|style| style.bg(rgba(pal.hover)))
                    })
                    .when_some(icon, |this, icon| this.child(self.glyph(icon, 14., tint)))
                    .child(label)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.select_file_kind(kind, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap_2()
            .px_4()
            .pt_3()
            .pb_2()
            .border_b_1()
            .border_color(rgba(pal.divider))
            .children(chips)
            .into_any_element()
    }

    /// "Clear history" in the clipboard view: forget every entry, here and in the app.
    fn clear_clips(&mut self, cx: &mut Context<Self>) {
        self.clips.clear();

        if let Some(sender) = UI_SENDER.get() {
            let _ = sender.try_send(UiMessage::ClearClips);
        }

        self.request_search();
        cx.notify();
    }

    /// The title bar of the clipboard view: an icon, "Clipboard", and "Clear history".
    fn render_clip_header(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.clip_filter()?;

        let pal = self.palette;

        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .px_5()
                .pt_4()
                .pb_2()
                .child(self.glyph("icons/clipboard.svg", 18., pal.text_dim))
                .child(
                    div()
                        .flex_1()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text))
                        .child("Clipboard"),
                )
                .when(!self.clips.is_empty(), |this| {
                    this.child(
                        div()
                            .id("clip-clear")
                            .px_3()
                            .py_1()
                            .rounded_full()
                            .text_sm()
                            .cursor_pointer()
                            .bg(rgba(pal.pill))
                            .text_color(rgb(pal.text_dim))
                            .hover(|style| style.bg(rgba(pal.hover)))
                            .child("Clear history")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.clear_clips(cx);
                            })),
                    )
                })
                .into_any_element(),
        )
    }

    /// "All  Applications  Files ...": only when the results hold more than one kind.
    fn render_chips(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.scope == Some(Quick::Files) {
            return Some(self.render_file_chips(cx));
        }

        if self.is_grid_list(&self.all_hits) {
            return self.render_group_chips(cx);
        }

        let sections = self.chip_sections();

        if sections.len() < 2 {
            return None;
        }

        let pal = self.palette;
        let mut entries: Vec<(Option<Section>, &'static str)> = vec![(None, "All")];

        entries.extend(
            sections
                .into_iter()
                .filter_map(|section| chip_label(section).map(|label| (Some(section), label))),
        );

        let chips: Vec<AnyElement> = entries
            .into_iter()
            .map(|(section, label)| {
                let on = self.chip == section;

                div()
                    .id(SharedString::from(format!("chip-{label}")))
                    .px_3()
                    .py_1()
                    .rounded_full()
                    .text_sm()
                    .cursor_pointer()
                    .when(on, |this| {
                        this.bg(rgba(pal.accent_soft))
                            .text_color(rgb(pal.accent_fg))
                    })
                    .when(!on, |this| {
                        this.bg(rgba(pal.pill))
                            .text_color(rgb(pal.text_dim))
                            .hover(|style| style.bg(rgba(pal.hover)))
                    })
                    .child(label)
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.select_chip(section, cx);
                    }))
                    .into_any_element()
            })
            .collect();

        Some(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_2()
                .px_4()
                .pt_3()
                .pb_2()
                .border_b_1()
                .border_color(rgba(pal.divider))
                .children(chips)
                .into_any_element(),
        )
    }
}

impl Focusable for LauncherView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LauncherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut last_section: Option<Section> = None;
        let grid = self.grid_active();

        for index in 0..(if grid { 0 } else { self.results.len() }) {
            let section = self.sections.get(index).copied().unwrap_or(Section::Web);

            if last_section != Some(section) {
                rows.push(
                    div()
                        .px_5()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(pal.text_faint))
                        .child(section_title(section))
                        .into_any_element(),
                );
                last_section = Some(section);
            }

            // Borrowed, not cloned: this runs every frame for every visible row.
            let hit = &self.results[index];

            rows.push(self.render_row(index, hit, cx));
        }

        if grid {
            rows = self.render_grid(cx);
        }

        let show_footer = self.settings.appearance.show_footer;
        let preview = if grid { None } else { self.render_preview(cx) };
        let menu = self.render_menu(cx);
        let tips_on = self.tips;
        let chips = if tips_on { None } else { self.render_chips(cx) };
        let clip_header = if tips_on {
            None
        } else {
            self.render_clip_header(cx)
        };
        let tips_panel = tips_on.then(|| self.render_tips(cx));
        let more_menu = self.render_more(cx);
        let movable = self.settings.appearance.launcher_movable;

        // Like Spotlight: with nothing to show, only the search field and its buttons remain.
        // The exceptions help someone who does not know what to type: the file-type chips of
        // the Files view, the tips panel, and a few clickable examples on an empty launcher.
        let rows_empty = rows.is_empty();
        let files_view = self.scope == Some(Quick::Files);
        let show_try = rows_empty
            && self.query.is_empty()
            && self.scope.is_none()
            && !tips_on
            && self.notice.is_none()
            && self.status.is_none();
        let has_card = !rows_empty
            || self.notice.is_some()
            || self.status.is_some()
            || tips_on
            || files_view
            || show_try;
        let card_h = if !rows_empty || tips_on {
            CARD_FULL_H
        } else {
            CARD_SHORT_H
        };

        // Fit the window to what is drawn: the search row alone, or the row and the card. The
        // "..." menu hangs below the row and needs some room even when nothing else shows.
        let mut want_h = if has_card {
            ROW_H + ROW_GAP + card_h
        } else {
            ROW_H
        };

        if self.more {
            want_h = want_h.max(ROW_H + ROW_GAP + 150.0);
        }

        if (self.window_h - want_h).abs() > 0.5 {
            self.window_h = want_h;

            let width = window.viewport_size().width;

            window.on_next_frame(move |window, _cx| window.resize(size(width, px(want_h))));
        }

        // The search field: a pill of its own, with the round quick buttons beside it.
        let search_bar = rounded_px(div(), 26.)
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .flex_1()
            .min_w(px(0.))
            .h(px(52.))
            .px_5()
            .bg(rgba(pal.card))
            .border_1()
            .border_color(rgba(pal.border))
            .text_color(rgb(pal.text))
            // Drag the bar (outside the text) to move the launcher.
            .when(movable, |this| {
                this.on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_this, _event, window, _cx| begin_window_move(window)),
                )
            })
            .child(match self.scoped_quick() {
                // The active button's icon takes the place of the magnifier.
                Some(quick) => self.glyph(quick.icon(), 20., pal.accent_fg),
                None => self.glyph("icons/search.svg", 20., pal.text_faint),
            })
            .child(self.render_query(cx))
            // The icon of the result Enter would open, like Spotlight's at the right of the bar.
            .when_some(self.bar_hit(), |this, hit| {
                this.child(self.leading(hit, 30.))
            })
            // Clear what was typed (the scope, if any, stays).
            .when(!self.query.is_empty(), |this| {
                this.child(
                    div()
                        .id("clear")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(26.))
                        .flex_shrink_0()
                        .rounded_full()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(pal.hover)))
                        .tooltip(self.hover_tip("Clear"))
                        .child(self.glyph("icons/x.svg", 14., pal.text_dim))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                cx.stop_propagation();
                                this.set_query("", cx);
                            }),
                        ),
                )
            })
            // "...": view options, tips and settings, so nothing has to be remembered.
            .child(
                div()
                    .id("more")
                    .tooltip(self.hover_tip("More"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(30.))
                    .flex_shrink_0()
                    .rounded_full()
                    .cursor_pointer()
                    .when(self.more, |this| this.bg(rgba(pal.accent_soft)))
                    .when(!self.more, |this| {
                        this.hover(|style| style.bg(rgba(pal.hover)))
                    })
                    .child(self.glyph(
                        "icons/more.svg",
                        18.,
                        if self.more {
                            pal.accent_fg
                        } else {
                            pal.text_dim
                        },
                    ))
                    // The bar moves the window on mouse-down; this button must not.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            cx.stop_propagation();
                            this.more = !this.more;
                            cx.notify();
                        }),
                    ),
            );

        let search_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .w_full()
            .child(search_bar)
            .child(self.render_quick_buttons(cx));

        // The results, in a card of their own below the search row.
        let card = rounded_px(div(), pal.radius)
            .flex()
            .flex_col()
            .w_full()
            .bg(rgba(pal.card))
            .h(px(card_h))
            .border_1()
            .border_color(rgba(pal.border))
            .overflow_hidden()
            .text_color(rgb(pal.text))
            .when_some(clip_header, |this, header| this.child(header))
            .when_some(chips, |this, chips| this.child(chips))
            .when(!rows_empty && !tips_on, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_1()
                        .min_h(px(0.))
                        .child(
                            div()
                                .id("results")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.))
                                .min_h(px(0.))
                                .overflow_y_scroll()
                                .track_scroll(&self.scroll)
                                .pt_1()
                                .pb_2()
                                .children(rows),
                        )
                        .when_some(preview, |this, preview| this.child(preview)),
                )
            })
            .when_some(self.notice.clone(), |this, notice| {
                this.child(
                    div()
                        .px_5()
                        .py_3()
                        .text_sm()
                        .text_color(rgb(pal.text_dim))
                        .child(notice),
                )
            })
            .when_some(self.status.clone(), |this, status| {
                this.child(
                    div()
                        .px_5()
                        .py_3()
                        .text_sm()
                        .text_color(rgb(pal.danger))
                        .child(status),
                )
            })
            .when_some(tips_panel, |this, panel| this.child(panel))
            .when(show_try, |this| this.child(self.render_try(cx)))
            .when(show_footer && !rows_empty && !tips_on, |this| {
                this.child(self.render_footer(cx))
            });

        // The window is transparent: only the search row and the card are painted. The menu
        // floats above them.
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event, _window, _cx| this.drag_anchor = None),
            )
            .relative()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .child(search_row)
            .when(has_card, |this| this.child(card))
            .when_some(menu, |this, menu| this.child(menu))
            .when_some(more_menu, |this, menu| this.child(menu))
    }
}

/// Where the launcher window goes, from the Appearance settings.
fn launcher_bounds(settings: &Settings, cx: &App) -> Bounds<gpui::Pixels> {
    let launcher_size = size(px(settings.appearance.width as f32), px(WINDOW_H));
    let centered = Bounds::centered(None, launcher_size, cx);

    if !can_place_windows() {
        return centered;
    }

    match settings.appearance.launcher_position {
        LauncherPosition::Center => centered,
        // Horizontally centered in the upper part of the screen, like Spotlight. Only the
        // top of the (tall, transparent) window is painted.
        LauncherPosition::Top => match cx.primary_display() {
            Some(display) => {
                let area = display.bounds();
                let (left, top) = (f32::from(area.origin.x), f32::from(area.origin.y));
                let (screen_w, screen_h) =
                    (f32::from(area.size.width), f32::from(area.size.height));
                let x = left + (screen_w - settings.appearance.width as f32) / 2.0;
                let y = top + (screen_h * 0.16).min((screen_h - WINDOW_H).max(0.0));

                Bounds::new(point(px(x), px(y)), launcher_size)
            }
            None => centered,
        },
        LauncherPosition::Remember => match settings.appearance.launcher_origin {
            Some((x, y)) => Bounds::new(point(px(x as f32), px(y as f32)), launcher_size),
            None => centered,
        },
    }
}

fn background_for(settings: &Settings) -> WindowBackgroundAppearance {
    if settings.appearance.blur {
        WindowBackgroundAppearance::Blurred
    } else {
        WindowBackgroundAppearance::Transparent
    }
}

fn open_launcher(
    cx: &mut App,
    requests: async_channel::Sender<UiRequest>,
    clips: &mut Vec<ClipEntry>,
) -> Option<WindowHandle<LauncherView>> {
    // Re-read the settings every time, so changes apply on the next open.
    let settings = Settings::load_or_default();
    let bounds = launcher_bounds(&settings, cx);

    // Best effort: whatever is on the clipboard now joins the history (on GNOME/Wayland the
    // background watcher cannot see the clipboard, so this is how entries get captured).
    if settings.plugins.clipboard {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            push_clip(clips, text);
        }
    }

    let close_on_blur = settings.appearance.close_on_blur;
    let track_position =
        can_place_windows() && settings.appearance.launcher_position == LauncherPosition::Remember;
    let snapshot = clips.clone();

    // A new window: forget any earlier drag, and invalidate its auto-close timer.
    DRAG_ACTIVE.store(false, Ordering::SeqCst);
    LAUNCHER_GENERATION.fetch_add(1, Ordering::SeqCst);
    LAUNCHER_ALIVE.store(true, Ordering::SeqCst);
    // Opening can shuffle focus around, e.g. a clipboard poll that was in flight.
    ignore_blur_for(Duration::from_millis(800));

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        window_background: background_for(&settings),
        app_id: Some("aujar-launcher".into()),
        ..Default::default()
    };

    cx.open_window(options, move |window, cx| {
        let view = cx.new(|cx| LauncherView::new(requests, settings, snapshot, cx));
        let focus_handle = view.read(cx).focus_handle.clone();

        // Without focus, key events never reach the view.
        window.focus(&focus_handle, cx);

        view.update(cx, |_view, cx| {
            cx.on_release(|_this, _cx| LAUNCHER_ALIVE.store(false, Ordering::SeqCst))
                .detach();
        });

        // Note where the window sits, so a drag can be remembered (X11 only).
        if track_position {
            view.update(cx, |_view, cx| {
                cx.observe_window_bounds(window, |this, window, _cx| {
                    let origin = window.bounds().origin;
                    let origin = (
                        f32::from(origin.x).round() as i32,
                        f32::from(origin.y).round() as i32,
                    );

                    this.first_origin.get_or_insert(origin);
                    this.last_origin = Some(origin);
                })
                .detach();
            });
        }

        // Click-away closes the launcher. The grace period ignores the focus churn that
        // compositors (especially Wayland) produce while a new window is being mapped.
        if close_on_blur {
            let opened = Instant::now();

            view.update(cx, |_view, cx| {
                cx.observe_window_activation(window, move |_this, window, _cx| {
                    if window.is_window_active() {
                        // Focus is back (drag finished, background read done): resume normal
                        // click-away behaviour shortly after.
                        ignore_blur_for(Duration::from_millis(300));
                        return;
                    }

                    if opened.elapsed() <= Duration::from_millis(500)
                        || DRAG_ACTIVE.load(Ordering::SeqCst)
                    {
                        return;
                    }

                    debug("window lost focus");

                    if now_ms() < BLUR_IGNORE_UNTIL.load(Ordering::SeqCst) {
                        debug("  ignored (expected focus loss)");
                        // Expected focus loss (window drag / clipboard poll): take focus back.
                        window.activate_window();
                        return;
                    }

                    debug("  closing: click-away");
                    window.remove_window();
                })
                .detach();
            });
        }

        view
    })
    .ok()
}

fn open_settings(cx: &mut App) -> Option<WindowHandle<SettingsView>> {
    let settings = Settings::load_or_default();
    let bounds = Bounds::centered(None, size(px(980.), px(720.)), cx);

    // The settings window draws its own title bar and frame (traffic lights, rounded corners,
    // translucency), so ask for client-side decorations and a transparent surface.
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Aujar Settings".into()),
            appears_transparent: true,
            traffic_light_position: None,
        }),
        window_decorations: Some(WindowDecorations::Client),
        window_background: background_for(&settings),
        window_min_size: Some(size(px(720.), px(520.))),
        app_id: Some("aujar-settings".into()),
        ..Default::default()
    };

    cx.open_window(options, |window, cx| {
        let view = cx.new(SettingsView::new);
        let focus_handle = view.read(cx).focus_handle.clone();

        window.focus(&focus_handle, cx);

        view
    })
    .ok()
}

/// Newest first, de-duplicated, bounded. Memory only: never written to disk.
fn push_clip(clips: &mut Vec<ClipEntry>, text: String) {
    let text = text.trim().to_owned();

    if text.is_empty() || text.chars().count() > 4000 {
        return;
    }

    // Copying the same text again moves it to the top with the new time.
    clips.retain(|existing| existing.text != text);
    clips.insert(
        0,
        ClipEntry {
            text,
            copied_at: now_secs(),
        },
    );
    clips.truncate(50);
}

// ---- Window bookkeeping -----------------------------------------------------------------

#[derive(Default)]
struct UiState {
    launcher: Option<WindowHandle<LauncherView>>,
    settings: Option<WindowHandle<SettingsView>>,
    /// Clipboard history, newest first (memory only).
    clips: Vec<ClipEntry>,
}

fn handle_message(
    cx: &mut App,
    state: &mut UiState,
    requests: &async_channel::Sender<UiRequest>,
    message: UiMessage,
) {
    // Forget handles of windows the user already closed (Esc, Enter, window manager).
    if state
        .launcher
        .as_ref()
        .is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_err())
    {
        state.launcher = None;
    }

    if state
        .settings
        .as_ref()
        .is_some_and(|handle| handle.update(cx, |_, _, _| ()).is_err())
    {
        state.settings = None;
    }

    match message {
        UiMessage::Launcher(LauncherCommand::Settings) | UiMessage::OpenSettings => {
            // Replace any existing settings window: it re-reads the file and comes to front.
            if let Some(handle) = state.settings.take() {
                let _ = handle.update(cx, |_, window, _| window.remove_window());
            }

            state.settings = open_settings(cx);
        }
        UiMessage::Launcher(command) => {
            let is_open = state.launcher.is_some();
            debug(&format!(
                "launcher command: {} (open: {is_open})",
                match command {
                    LauncherCommand::Show => "show",
                    LauncherCommand::Hide => "hide",
                    _ => "toggle",
                }
            ));

            let want_open = match command {
                LauncherCommand::Show => true,
                LauncherCommand::Hide => false,
                _ => !is_open,
            };

            match (is_open, want_open) {
                (false, true) => {
                    state.launcher = open_launcher(cx, requests.clone(), &mut state.clips)
                }
                (true, false) => {
                    if let Some(handle) = state.launcher.take() {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                }
                _ => {}
            }
        }
        UiMessage::Results {
            query,
            results,
            attempt,
        } => {
            if let Some(handle) = state.launcher.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    // Ignore answers for a query the user has already edited.
                    if view.search_text() == query {
                        view.set_results(results);

                        // A lookup (definition, weather, ...) is still being fetched: ask
                        // again shortly; the daemon answers from its cache once it is done.
                        if has_loading(&view.results) && attempt < MAX_POLLS {
                            let _ = view.requests.try_send(UiRequest::Search {
                                query,
                                attempt: attempt + 1,
                            });
                        }

                        cx.notify();
                    }
                });
            }
        }
        UiMessage::Clipboard(text) => push_clip(&mut state.clips, text),
        UiMessage::ClearClips => state.clips.clear(),
        UiMessage::RecentFiles(files) => {
            if let Some(handle) = state.launcher.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    view.recent_files = files;

                    // Only the empty launcher shows file history.
                    if view.query.trim().is_empty() {
                        view.build_empty_state();
                        cx.notify();
                    }
                });
            }
        }
        UiMessage::ThumbReady => {
            if let Some(handle) = state.launcher.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    view.refresh_preview();
                    cx.notify();
                });
            }
        }
        UiMessage::Failed(message) => {
            if let Some(handle) = state.launcher.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    view.status = Some(message);
                    cx.notify();
                });
            }
        }
    }
}

// ---- Daemon bridge ----------------------------------------------------------------------

fn socket_path() -> PathBuf {
    if let Some(socket) = std::env::var_os("AUJAR_SOCKET") {
        return PathBuf::from(socket);
    }

    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) => PathBuf::from(dir).join("aujar.sock"),
        None => PathBuf::from("/tmp/aujar.sock"),
    }
}

async fn search(socket: &Path, query: &str) -> Result<Vec<SearchHit>> {
    let request = Request::Search(SearchRequest {
        query: query.to_owned(),
        // The UI caps each section itself; ask for enough to fill all of them. The Applications
        // grid lists everything installed.
        limit: Some(if query.trim().eq_ignore_ascii_case("kind:app") {
            150
        } else {
            40
        }),
    });

    match send(socket, request).await? {
        Response::Search(response) => {
            let mut results = response.results;

            describe_files(&mut results);

            Ok(results)
        }
        Response::Error(message) => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}

async fn execute(socket: &Path, id: &str) -> Result<()> {
    let request = Request::Execute(ExecuteRequest { id: id.to_owned() });

    match send(socket, request).await? {
        Response::Executed(done) => {
            eprintln!("aujar-ui: launched {}", done.title);
            Ok(())
        }
        Response::Error(message) => bail!(message),
        other => bail!("unexpected response: {other:?}"),
    }
}

/// Handles search/execute requests from the UI, one fresh connection each.
async fn serve_requests(
    socket: PathBuf,
    tx: async_channel::Sender<UiMessage>,
    requests: async_channel::Receiver<UiRequest>,
) {
    let _ = UI_SENDER.set(tx.clone());
    let latest = Arc::new(AtomicU64::new(0));

    while let Ok(request) = requests.recv().await {
        match request {
            UiRequest::Search { query, attempt } => {
                // Searches run concurrently and only the newest one reports back: a slow
                // query (cold file index, a lookup) must not hold up the keystrokes after it.
                let id = latest.fetch_add(1, Ordering::SeqCst) + 1;
                let latest = Arc::clone(&latest);
                let socket = socket.clone();
                let tx = tx.clone();

                tokio::spawn(async move {
                    // Typing fast supersedes this search before it is sent (debounce); a
                    // re-ask for a loading lookup waits a little longer.
                    let wait = if attempt == 0 {
                        SEARCH_DEBOUNCE
                    } else {
                        POLL_DELAY
                    };

                    tokio::time::sleep(wait).await;

                    if latest.load(Ordering::SeqCst) != id {
                        return;
                    }

                    let message = match search(&socket, &query).await {
                        Ok(results) => UiMessage::Results {
                            query,
                            results,
                            attempt,
                        },
                        Err(error) => UiMessage::Failed(format!("{error:#}")),
                    };

                    if latest.load(Ordering::SeqCst) == id {
                        let _ = tx.send(message).await;
                    }
                });
            }
            UiRequest::Execute(id) => {
                if let Err(error) = execute(&socket, &id).await {
                    eprintln!("aujar-ui: launch failed: {error:#}");
                }
            }
            UiRequest::OpenSettings => {
                if tx.send(UiMessage::OpenSettings).await.is_err() {
                    break;
                }
            }
            UiRequest::Recents => {
                // File history: what the desktop says you opened recently, newest first.
                if let Ok(mut results) = search(&socket, "kind:recent").await {
                    results.retain(|hit| hit.kind == "file" || hit.kind == "folder");

                    if tx.send(UiMessage::RecentFiles(results)).await.is_err() {
                        break;
                    }
                }
            }
            UiRequest::Thumbnail(path) => {
                let tx = tx.clone();

                // pdftoppm can take a moment; keep it off the async runtime's threads.
                tokio::task::spawn_blocking(move || {
                    if generate_pdf_thumb(&path) {
                        let _ = tx.send_blocking(UiMessage::ThumbReady);
                    }
                });
            }
        }
    }
}

/// One subscription to the daemon's event stream.
async fn run_session(socket: &Path, tx: &async_channel::Sender<UiMessage>) -> Result<()> {
    let mut stream = subscribe(socket).await?;

    eprintln!("aujar-ui: subscribed to the daemon");

    while let Some(event) = stream.recv().await? {
        match event {
            Event::Launcher(command) => {
                if tx.send(UiMessage::Launcher(command)).await.is_err() {
                    return Ok(());
                }
            }
            Event::Shutdown => break,
        }
    }

    Ok(())
}

/// Talks to the daemon on its own thread; reconnects if the daemon restarts.
fn spawn_bridge(
    socket: PathBuf,
    tx: async_channel::Sender<UiMessage>,
    requests: async_channel::Receiver<UiRequest>,
) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime");

        runtime.block_on(async move {
            tokio::spawn(serve_requests(socket.clone(), tx.clone(), requests));

            loop {
                if let Err(error) = run_session(&socket, &tx).await {
                    eprintln!("aujar-ui: {error:#} (retrying)");
                }

                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
    });
}

/// Polls the system clipboard for the history (X11 via `xclip`, Wayland via `wl-paste`).
/// GNOME's Wayland session does not allow background clipboard reads, so there the watcher
/// gives up quietly and history only gets what is on the clipboard when the launcher opens.
fn spawn_clipboard_watcher(tx: async_channel::Sender<UiMessage>) {
    std::thread::spawn(move || {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();

        let (program, args): (&str, &[&str]) = if wayland {
            ("wl-paste", &["--no-newline", "--type", "text"])
        } else {
            ("xclip", &["-selection", "clipboard", "-o"])
        };

        let mut enabled = false;
        let mut settings_stamp: Option<SystemTime> = None;
        let mut failures = 0u32;
        let mut ticks = 0u32;
        let mut last = String::new();

        loop {
            std::thread::sleep(Duration::from_millis(1500));

            // The setting can change while we run; look at the file every few seconds, and
            // only parse it when it actually changed.
            if ticks % 4 == 0 {
                let stamp = fs::metadata(Settings::path())
                    .and_then(|meta| meta.modified())
                    .ok();

                if stamp != settings_stamp || ticks == 0 {
                    settings_stamp = stamp;
                    enabled = Settings::load_or_default().plugins.clipboard;
                }
            }

            ticks = ticks.wrapping_add(1);

            // Never poll while the launcher is open: `wl-paste` can take focus for a moment on
            // GNOME/Wayland, which closed the launcher. (open_launcher captures the clipboard
            // itself.)
            if !enabled || LAUNCHER_ALIVE.load(Ordering::SeqCst) {
                continue;
            }

            ignore_blur_for(Duration::from_millis(1500));

            match Command::new(program).args(args).output() {
                Ok(output) if output.status.success() => {
                    failures = 0;

                    // Skip huge copies (a file's contents, an image as text): not history material.
                    if output.stdout.len() <= 64 * 1024 {
                        if let Ok(text) = String::from_utf8(output.stdout) {
                            if text != last {
                                last = text.clone();

                                if tx.send_blocking(UiMessage::Clipboard(text)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
                // An empty clipboard is an error exit for wl-paste, not a broken tool.
                Ok(output)
                    if String::from_utf8_lossy(&output.stderr).contains("Nothing is copied") => {}
                _ => {
                    failures += 1;

                    if failures >= 10 {
                        return;
                    }
                }
            }
        }
    });
}

/// Only one UI per user session. A second one (say, the installed service plus a dev build)
/// would also subscribe to the daemon, so every shortcut press opened two launchers that
/// fought over focus and closed each other.
fn ensure_single_instance() -> Option<std::os::unix::net::UnixListener> {
    use std::os::unix::net::{UnixListener, UnixStream};

    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = dir.join("aujar-ui.lock");

    match UnixListener::bind(&path) {
        Ok(listener) => Some(listener),
        Err(_) => {
            // Taken: a live instance answers; a stale file left by a crash does not.
            if UnixStream::connect(&path).is_ok() {
                return None;
            }

            let _ = fs::remove_file(&path);
            UnixListener::bind(&path).ok()
        }
    }
}

fn main() {
    let Some(_instance) = ensure_single_instance() else {
        eprintln!("aujar-ui: already running (stop it first: systemctl --user stop aujar-ui)");

        // EX_TEMPFAIL, not 0: with `Restart=always` a clean exit would respawn this process
        // every two seconds for as long as the other instance lives. The unit lists 75 in
        // `RestartPreventExitStatus=`.
        std::process::exit(75);
    };

    let (message_tx, message_rx) = async_channel::unbounded::<UiMessage>();
    let (request_tx, request_rx) = async_channel::unbounded::<UiRequest>();

    spawn_clipboard_watcher(message_tx.clone());
    spawn_bridge(socket_path(), message_tx, request_rx);

    // Closing the launcher window (Esc / Enter) must not end the process: the UI has to
    // stay alive to receive the next toggle. By default GPUI quits with the last window.
    application()
        .with_assets(assets::Assets)
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx: &mut App| {
            cx.spawn(async move |cx| {
                let mut state = UiState::default();

                while let Ok(message) = message_rx.recv().await {
                    cx.update(|cx| handle_message(cx, &mut state, &request_tx, message));
                }
            })
            .detach();
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(year: i32, yday: i32, weekday: usize, hour: u32, minute: u32) -> Option<Stamp> {
        Some(Stamp {
            year,
            yday,
            weekday,
            hour,
            minute,
        })
    }

    #[test]
    fn clock_text_is_twelve_hour() {
        assert_eq!(clock_text(0, 5), "12:05 AM");
        assert_eq!(clock_text(9, 30), "9:30 AM");
        assert_eq!(clock_text(12, 0), "12:00 PM");
        assert_eq!(clock_text(16, 19), "4:19 PM");
        assert_eq!(clock_text(23, 59), "11:59 PM");
    }

    #[test]
    fn copied_text_reads_like_a_person() {
        let now = stamp(2026, 300, 2, 17, 0);
        let age = |minutes: u64| Duration::from_secs(minutes * 60);

        assert_eq!(copied_text(now, now, Duration::from_secs(5)), "just now");
        assert_eq!(
            copied_text(now, stamp(2026, 300, 2, 16, 19), age(41)),
            "4:19 PM"
        );
        assert_eq!(
            copied_text(now, stamp(2026, 299, 1, 21, 5), age(1200)),
            "yesterday 9:05 PM"
        );
        assert_eq!(
            copied_text(now, stamp(2026, 297, 6, 8, 0), age(60 * 24 * 3)),
            "Sat 8:00 AM"
        );
        assert_eq!(
            copied_text(now, stamp(2026, 280, 4, 8, 0), age(60 * 24 * 20)),
            "20 days ago"
        );
        // New Year's Eve seen from New Year's Day.
        assert_eq!(
            copied_text(
                stamp(2027, 0, 5, 10, 0),
                stamp(2026, 364, 4, 23, 30),
                age(600)
            ),
            "yesterday 11:30 PM"
        );
        // Without a local clock, the age is the honest answer.
        assert_eq!(copied_text(None, None, age(5)), "5 min ago");
    }

    #[test]
    fn local_stamp_works_for_the_present() {
        let stamp = local_stamp(now_secs()).expect("the C library knows the local time");

        assert!(stamp.hour < 24 && stamp.minute < 60 && stamp.weekday < 7);
        assert!((0..366).contains(&stamp.yday));
    }

    #[test]
    fn url_hosts_are_extracted() {
        assert_eq!(
            url_host("https://en.wikipedia.org/wiki/Mac_OS_X_Tiger").as_deref(),
            Some("en.wikipedia.org")
        );
        assert_eq!(
            url_host("http://example.com?q=1").as_deref(),
            Some("example.com")
        );
        assert_eq!(url_host("https://"), None);
        assert_eq!(url_host("ftp://example.com"), None);
    }

    #[test]
    fn pushing_a_copy_again_moves_it_to_the_top_with_a_new_time() {
        let mut clips = vec![
            ClipEntry {
                text: "two".into(),
                copied_at: 200,
            },
            ClipEntry {
                text: "one".into(),
                copied_at: 100,
            },
        ];

        push_clip(&mut clips, "  one \n".into());

        let texts: Vec<&str> = clips.iter().map(|clip| clip.text.as_str()).collect();

        assert_eq!(texts, ["one", "two"]);
        assert!(clips[0].copied_at >= 200);

        push_clip(&mut clips, "   ".into());
        assert_eq!(clips.len(), 2);
    }

    #[test]
    fn file_details_become_a_readable_subtitle() {
        assert_eq!(parse_file_meta("meta:1234:99"), Some((1234, 99)));
        assert_eq!(parse_file_meta("meta:x:99"), None);
        assert_eq!(parse_file_meta("group:Internet"), None);

        let hit = |kind: &str, title: &str, detail: Option<&str>| SearchHit {
            id: format!("files:open:/home/u/Downloads/{title}"),
            title: title.into(),
            subtitle: Some("~/Downloads".into()),
            kind: kind.into(),
            score: 0,
            icon: None,
            detail: detail.map(str::to_owned),
        };

        let mut hits = vec![
            hit("file", "invoice.pdf", Some("meta:58368:0")),
            hit("folder", "Receipts", Some("meta:0:0")),
            hit("file", "old.txt", None),
            hit("application", "Firefox", Some("meta:1:1")),
        ];

        describe_files(&mut hits);

        // No date when the daemon does not know one (0); a folder has no size.
        assert_eq!(
            hits[0].subtitle.as_deref(),
            Some("PDF document \u{b7} 57.0 KB \u{b7} ~/Downloads")
        );
        assert_eq!(hits[0].detail, None);
        assert_eq!(
            hits[1].subtitle.as_deref(),
            Some("Folder \u{b7} ~/Downloads")
        );

        // Without details (an older daemon) or for other kinds, nothing changes.
        assert_eq!(hits[2].subtitle.as_deref(), Some("~/Downloads"));
        assert_eq!(hits[3].detail.as_deref(), Some("meta:1:1"));

        // A recent file gets a date in words.
        let mut recent = vec![hit(
            "file",
            "a.png",
            Some(&format!("meta:2048:{}", now_secs().saturating_sub(30))),
        )];

        describe_files(&mut recent);

        assert_eq!(
            recent[0].subtitle.as_deref(),
            Some("PNG image \u{b7} 2.0 KB \u{b7} just now \u{b7} ~/Downloads")
        );
    }

    #[test]
    fn every_chip_and_token_icon_is_built_in_and_the_types_differ() {
        use gpui::AssetSource;

        let mut icons: Vec<&str> = Vec::new();

        icons.extend(FILE_KINDS.iter().map(|(_, _, icon)| *icon));
        icons.extend(APP_GROUPS.iter().map(|group| group_icon(group)));
        icons.extend(Quick::ALL.iter().map(|quick| quick.icon()));

        for icon in &icons {
            assert!(
                assets::Assets.load(icon).unwrap().is_some(),
                "{icon} is not built in"
            );
        }

        // PDFs, pictures, videos and the rest must not all look alike.
        let mut file_icons: Vec<&str> = FILE_KINDS.iter().map(|(_, _, icon)| *icon).collect();

        file_icons.sort_unstable();
        file_icons.dedup();
        assert_eq!(file_icons.len(), FILE_KINDS.len());

        // Groups may share an icon only when the meaning is the same; at least most differ.
        let mut group_icons: Vec<&str> = APP_GROUPS.iter().map(|g| group_icon(g)).collect();

        group_icons.sort_unstable();
        group_icons.dedup();
        assert!(group_icons.len() >= 8, "{group_icons:?}");
    }

    #[test]
    fn the_files_grid_has_suggestions_then_recents() {
        // Only recents: a header, then rows of five.
        assert_eq!(files_grid_rows(0, 7), [1, 1, 1, 1, 1, 2, 2]);
        // Three suggestions, five more recents: header 0, one row, header 2, one row.
        assert_eq!(files_grid_rows(3, 8), [1, 1, 1, 3, 3, 3, 3, 3]);
        // Five suggestions fill a row; recents start after their own header.
        assert_eq!(files_grid_rows(5, 7), [1, 1, 1, 1, 1, 3, 3]);
        // Only suggestions.
        assert_eq!(files_grid_rows(2, 2), [1, 1]);
        assert!(files_grid_rows(0, 0).is_empty());
    }

    #[test]
    fn pictures_get_thumbnails() {
        assert!(is_image_name("shot.PNG"));
        assert!(is_image_name("a.b.jpeg"));
        assert!(is_image_name("x.webp"));
        assert!(!is_image_name("report.pdf"));
        assert!(!is_image_name("png"));
        assert!(!is_image_name("photo.png.zip"));
    }

    #[test]
    fn chips_cycle_and_wrap() {
        // Four positions: All, A, B, C.
        assert_eq!(cycle_index(4, 0, 1), 1);
        assert_eq!(cycle_index(4, 3, 1), 0);
        assert_eq!(cycle_index(4, 0, -1), 3);
        assert_eq!(cycle_index(4, 2, -1), 1);
        assert_eq!(cycle_index(1, 0, 1), 0);
        assert_eq!(cycle_index(0, 0, 1), 0);
    }

    #[test]
    fn every_button_says_what_it_does() {
        for quick in Quick::ALL {
            assert!(!quick.hint(false).is_empty());
            assert_ne!(quick.hint(false), quick.hint(true));
        }
    }

    #[test]
    fn folders_are_shown_with_a_tilde() {
        let home = Some(Path::new("/home/u"));

        assert_eq!(
            pretty_folder_in(Path::new("/home/u/Downloads/x"), home),
            "~/Downloads/x"
        );
        assert_eq!(pretty_folder_in(Path::new("/home/u"), home), "~");
        assert_eq!(pretty_folder_in(Path::new("/mnt/data"), home), "/mnt/data");
        assert_eq!(pretty_folder_in(Path::new("/home/u/a"), None), "/home/u/a");
    }

    #[test]
    fn the_window_fits_its_content() {
        // The row alone, the row with a short card, the row with the full card.
        assert!(ROW_H < ROW_H + ROW_GAP + CARD_SHORT_H);
        assert!(CARD_SHORT_H < CARD_FULL_H);
        // Never taller than the window the launcher opens with.
        assert!(ROW_H + ROW_GAP + CARD_FULL_H <= WINDOW_H);
    }

    #[test]
    fn the_bar_completes_with_the_title_of_the_result() {
        let hit = |kind: &str, title: &str| SearchHit {
            id: "x".into(),
            title: title.into(),
            subtitle: None,
            kind: kind.into(),
            score: 0,
            icon: None,
            detail: None,
        };

        assert_eq!(
            ghost_text("fire", &hit("application", "Firefox")).as_deref(),
            Some(" \u{2014} Firefox")
        );
        // Typing the whole title adds nothing; case does not matter.
        assert_eq!(ghost_text("FIREFOX", &hit("application", "Firefox")), None);
        assert_eq!(ghost_text("  ", &hit("application", "Firefox")), None);

        // The web-search row and clipboard entries do not complete the text.
        assert_eq!(
            ghost_text("fire", &hit("web", "Search the web for fire")),
            None
        );
        assert_eq!(ghost_text("fire", &hit("clip", "fire alarm")), None);

        // A long title is cut.
        let long = ghost_text("a", &hit("file", &"x".repeat(100))).unwrap();
        assert!(long.chars().count() < 50, "{long}");
    }

    #[test]
    fn app_groups_come_from_the_daemon_detail() {
        let hit = |detail: Option<&str>| SearchHit {
            id: "apps:x.desktop".into(),
            title: "X".into(),
            subtitle: None,
            kind: "application".into(),
            score: 0,
            icon: None,
            detail: detail.map(str::to_owned),
        };

        assert_eq!(app_group(&hit(Some("group:Internet"))), Some("Internet"));
        assert_eq!(app_group(&hit(Some("something else"))), None);
        assert_eq!(app_group(&hit(None)), None);
        assert!(APP_GROUPS.contains(&"Other"));
    }

    #[test]
    fn quick_buttons_have_titles() {
        for quick in Quick::ALL {
            assert!(!quick.title().is_empty());
        }

        assert_eq!(Quick::Applications.title(), "Applications");
    }

    #[test]
    fn typed_text_searches_inside_the_scope() {
        // No scope: the text is the query, untouched.
        assert_eq!(scoped_query(None, None, "  fire "), "  fire ");

        // An empty field lists everything in the scope.
        assert_eq!(
            scoped_query(Some(Quick::Applications), None, ""),
            "kind:app"
        );
        assert_eq!(
            scoped_query(Some(Quick::Applications), None, "   "),
            "kind:app"
        );
        assert_eq!(scoped_query(Some(Quick::Actions), None, ""), "kind:action");
        assert_eq!(scoped_query(Some(Quick::Files), None, ""), "kind:recent");
        assert_eq!(scoped_query(Some(Quick::Clipboard), None, ""), "clip");

        // Text narrows it.
        assert_eq!(
            scoped_query(Some(Quick::Applications), None, " fire "),
            "kind:app fire"
        );
        assert_eq!(
            scoped_query(Some(Quick::Actions), None, "lock"),
            "kind:action lock"
        );
        assert_eq!(
            scoped_query(Some(Quick::Clipboard), None, "url"),
            "clip url"
        );
        assert_eq!(
            scoped_query(Some(Quick::Files), None, "invoice"),
            "kind:file invoice"
        );

        // A file-type chip searches by kind, with or without text.
        assert_eq!(
            scoped_query(Some(Quick::Files), Some("pdf"), ""),
            "kind:pdf"
        );
        assert_eq!(
            scoped_query(Some(Quick::Files), Some("pdf"), " invoice "),
            "kind:pdf invoice"
        );
        // The chip only belongs to the Files view.
        assert_eq!(scoped_query(None, Some("pdf"), "x"), "x");
        assert_eq!(
            scoped_query(Some(Quick::Applications), Some("pdf"), "x"),
            "kind:app x"
        );

        // Every scoped query with text is still a listing (no web row, no top hit), except
        // Files and Clipboard, which have their own handling.
        assert!(is_listing_query("kind:app fire"));
        assert!(is_listing_query("kind:action lock"));
        assert!(!is_listing_query("kind:apple"));
    }

    #[test]
    fn every_tip_is_something_the_launcher_understands() {
        for tip in &TIPS {
            assert!(!tip.title.is_empty() && !tip.example.is_empty() && !tip.fill.is_empty());
            // What a click types is the start of what the example shows.
            assert!(
                tip.example.starts_with(tip.fill.trim_end()),
                "{} / {}",
                tip.example,
                tip.fill
            );
        }

        // Clicks fill the field, so no two tips may do the same thing.
        let mut fills: Vec<&str> = TIPS.iter().map(|tip| tip.fill).collect();

        fills.sort_unstable();
        fills.dedup();
        assert_eq!(fills.len(), TIPS.len());

        for (label, kind, _) in FILE_KINDS {
            assert!(!label.is_empty() && !kind.contains(' '));
        }
    }

    #[test]
    fn a_scope_shows_only_its_own_kinds() {
        assert!(scope_keeps(Quick::Applications, "application"));
        assert!(!scope_keeps(Quick::Applications, "file"));
        assert!(!scope_keeps(Quick::Applications, "web"));
        assert!(scope_keeps(Quick::Files, "file"));
        assert!(scope_keeps(Quick::Files, "folder"));
        assert!(!scope_keeps(Quick::Files, "application"));
        assert!(scope_keeps(Quick::Actions, "system"));
        assert!(!scope_keeps(Quick::Actions, "application"));
        assert!(scope_keeps(Quick::Clipboard, "clip"));
    }

    #[test]
    fn listing_queries_are_the_quick_button_queries() {
        assert!(is_listing_query("kind:app"));
        assert!(is_listing_query("kind:action"));
        assert!(is_listing_query("kind:recent"));
        assert!(!is_listing_query("kind:pdf"));
        // Text after the trigger is still a listing: it searches inside the scope.
        assert!(is_listing_query("kind:app firefox"));
        assert!(is_listing_query("kind:action lock"));
        assert!(!is_listing_query("kind:apple"));
        assert!(!is_listing_query("firefox"));

        // Every button except Clipboard (which has its own `clip` view) lists something.
        for quick in Quick::ALL {
            if quick != Quick::Clipboard {
                assert!(is_listing_query(quick.query()), "{quick:?}");
            }
        }
    }

    #[test]
    fn only_real_result_kinds_get_a_chip() {
        assert_eq!(chip_label(Section::Applications), Some("Applications"));
        assert_eq!(chip_label(Section::Commands), Some("Actions"));
        assert_eq!(chip_label(Section::Files), Some("Files"));
        assert_eq!(chip_label(Section::Clipboard), Some("Clipboard"));

        for section in [
            Section::Recent,
            Section::RecentFiles,
            Section::Frequent,
            Section::Answer,
            Section::Lookup,
            Section::TopHit,
        ] {
            assert_eq!(chip_label(section), None, "{section:?}");
        }
    }

    #[test]
    fn detects_urls() {
        assert_eq!(
            url_from_query("github.com/rust-lang").as_deref(),
            Some("https://github.com/rust-lang")
        );
        assert_eq!(
            url_from_query("https://example.org").as_deref(),
            Some("https://example.org")
        );
        assert_eq!(url_from_query("3.14"), None);
        assert_eq!(url_from_query("hello world.com"), None);
        assert_eq!(url_from_query("firefox"), None);
    }

    #[test]
    fn encodes_queries() {
        assert_eq!(percent_encode("rust lifetimes"), "rust+lifetimes");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
    }

    #[test]
    fn only_power_actions_need_confirmation() {
        assert!(is_dangerous("system:shutdown"));
        assert!(!is_dangerous("system:lock"));
        assert!(!is_dangerous("apps:firefox.desktop"));
    }
}
