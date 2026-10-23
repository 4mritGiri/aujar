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
    AnyElement, App, Bounds, ClipboardItem, Context, ExternalDragPayload, FileDragPaths,
    FocusHandle, Focusable, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, ScrollHandle, SharedString, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowHandle, WindowOptions,
    div, img, point, prelude::*, px, rgb, rgba, size, svg,
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
const LIST_MAX_H: f32 = 430.0;

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
    /// Recently used files (file history) for the empty launcher.
    RecentFiles(Vec<SearchHit>),
    /// A PDF thumbnail finished rendering; redraw the preview.
    ThumbReady,
}

/// UI thread -> bridge thread.
enum UiRequest {
    /// `attempt` > 0 is a re-ask for a lookup that was still loading: it waits a moment first.
    Search { query: String, attempt: u32 },
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

    cache_dir().join("thumbs").join(format!("{:016x}", hasher.finish()))
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
        .args(["-png", "-f", "1", "-l", "1", "-singlefile", "-scale-to", "560"])
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

    fs::File::open(path).ok()?.take(4096).read_to_end(&mut buffer).ok()?;

    // A cut multi-byte character at the 4 KiB boundary is fine; NUL bytes mean binary.
    if buffer.contains(&0) {
        return None;
    }

    let text = String::from_utf8_lossy(&buffer);
    let lines: Vec<String> = text.lines().take(14).map(|line| first_chars(line, 70)).collect();

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

fn build_preview(hit: &SearchHit, clips: &[String]) -> Preview {
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
                            if count > 5000 { "5,000+ items".to_owned() } else { format!("{count} items") },
                        ));
                    }
                } else {
                    preview.rows.push(("Size", human_size(meta.len())));
                }

                let since = |time: std::io::Result<SystemTime>| {
                    time.and_then(|time| SystemTime::now().duration_since(time).map_err(std::io::Error::other))
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
                    } else if IMAGE_EXTENSIONS.contains(&extension.as_str()) && meta.len() < 15_000_000 {
                        preview.image = Some(path.clone());
                    } else if TEXT_EXTENSIONS.contains(&extension.as_str()) && meta.len() < 1_000_000 {
                        preview.text = read_text_preview(&path);
                    }
                }
            }

            if let Some(parent) = hit.subtitle.clone() {
                preview.rows.push(("Location", parent));
            }
        }
        "clip" => {
            let text = hit
                .id
                .strip_prefix("clip:")
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| clips.get(index));

            if let Some(text) = text {
                preview.rows.push(("Length", format!("{} characters", text.chars().count())));
                preview.text = Some(first_chars(text, 700));
            }
        }
        "calculation" => {
            if let Some(expression) = &hit.subtitle {
                preview.rows.push(("Input", expression.trim_start_matches("= ").to_owned()));
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
    clips: Vec<String>,
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
        clips: Vec<String>,
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
            .map_or(0, |at| at + self.query[at..].chars().next().map_or(1, char::len_utf8));
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
        self.undo.push((self.query.clone(), self.anchor, self.cursor));

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
            self.redo.push((self.query.clone(), self.anchor, self.cursor));
            self.restore(previous);
            self.edited(cx);
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.redo.pop() {
            self.undo.push((self.query.clone(), self.anchor, self.cursor));
            self.restore(next);
            self.edited(cx);
        }
    }

    /// Call after the query text changed.
    fn edited(&mut self, cx: &mut Context<Self>) {
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

        let lower = self.query.trim().to_lowercase();

        for keyword in ["clip", "cb"] {
            if lower == keyword {
                return Some(String::new());
            }

            if let Some(rest) = lower.strip_prefix(keyword).filter(|rest| rest.starts_with(' ')) {
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

        if self.query.trim().is_empty() {
            self.show_empty_state();
            return;
        }

        if let Some(filter) = self.clip_filter() {
            self.show_clips(&filter);
            return;
        }

        let _ = self.requests.try_send(UiRequest::Search {
            query: self.query.clone(),
            attempt: 0,
        });
    }

    fn install(&mut self, hits: Vec<SearchHit>, sections: Vec<Section>) {
        // Where each result sits among the list's children (a header precedes each section).
        let mut headers = 0;
        let mut previous = None;
        let mut row_index = Vec::with_capacity(sections.len());

        for (index, section) in sections.iter().enumerate() {
            if previous != Some(*section) {
                headers += 1;
                previous = Some(*section);
            }

            row_index.push(index + headers);
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

        if let Some(path) = self.preview.as_mut().and_then(|preview| preview.thumb_request.take()) {
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

        let hits: Vec<SearchHit> = self
            .clips
            .iter()
            .enumerate()
            .filter(|(_, text)| filter.is_empty() || text.to_lowercase().contains(&filter))
            .take(8)
            .map(|(index, text)| SearchHit {
                id: format!("clip:{index}"),
                title: first_chars(text.lines().next().unwrap_or_default().trim(), 90),
                subtitle: Some(format!("{} characters", text.chars().count())),
                kind: "clip".to_owned(),
                score: 0,
                icon: None,
                detail: None,
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
        let query = self.query.trim().to_owned();
        let lower = query.to_lowercase();
        let is_shell = query.starts_with('>');

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

        if !is_shell && !query.is_empty() {
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
            let boost = 150 * history.count_of(&hit.id).min(5);

            (
                section_of(&hit.kind) as usize,
                std::cmp::Reverse(hit.score + boost),
            )
        });

        let apps_cap = self.settings.appearance.max_results as usize;
        let mut counts = [0usize; SECTION_COUNT];

        hits.retain(|hit| {
            let section = section_of(&hit.kind);
            let cap = match section {
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
            };

            counts[section as usize] += 1;
            counts[section as usize] <= cap
        });

        let mut sections: Vec<Section> = hits.iter().map(|hit| section_of(&hit.kind)).collect();

        // Top Hit (like Spotlight): one strong application/command match, shown first.
        // Skipped when there is an Answer: that already is the top result.
        if !sections.contains(&Section::Answer) {
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

        if let Some(text) = hit
            .id
            .strip_prefix("clip:")
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| self.clips.get(index))
        {
            return text.clone();
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
        let selected = if self.has_selection() { self.selected_text().to_owned() } else { String::new() };
        let title = self.results.get(self.selected).map(|hit| hit.title.clone()).unwrap_or_default();

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
        let title = self.results.get(self.selected).map(|hit| hit.title.as_str()).unwrap_or("");

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
            y: y.clamp(8.0, (WINDOW_H - height - 8.0).max(8.0)),
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
                "," => {
                    let _ = self.requests.try_send(UiRequest::OpenSettings);
                    window.remove_window();
                }
                _ => {}
            }
            return;
        }

        match key {
            "escape" => {
                // First Esc clears the query (like Spotlight), second closes.
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
        let sub_color = if selected { rgba(pal.on_accent_dim) } else { rgb(pal.text_dim) };

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
                if confirming { "Enter again" } else { action_label }
            } else {
                kind_label
            })
            .when(selected && !inert, |this| {
                this.child(self.glyph("icons/enter.svg", 14., if selected { pal.on_accent } else { pal.text_dim }))
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
                        .child(div().text_xs().text_color(sub_color).child(
                            hit.subtitle.clone().unwrap_or_else(|| "Answer".to_owned()),
                        ))
                        .child(div().text_3xl().text_color(title_color).child(hit.title.clone())),
                )
                .child(trailing)
                .into_any_element();
        }

        if card {
            // Weather, quotes, scores and definitions: a larger row with the key facts.
            let first_definition = (hit.kind == "definition")
                .then(|| hit.detail.as_deref().map(|detail| definition_parts(detail).0))
                .flatten()
                .filter(|text| !text.is_empty());

            let subtitle_color = match (hit.kind.as_str(), hit.subtitle.as_deref().and_then(price_direction)) {
                ("stock", Some(up)) if !selected => rgb(if up { pal.positive } else { pal.negative }),
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
                            this.child(div().text_sm().text_color(subtitle_color).truncate().child(subtitle))
                        })
                        .when_some(first_definition, |this, text| {
                            this.child(div().text_sm().text_color(sub_color).line_clamp(2).child(text))
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

        base.py_2()
            .child(self.leading(hit, 32.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_hidden()
                    .child(div().text_base().text_color(title_color).truncate().child(hit.title.clone()))
                    .when_some(subtitle, |this, subtitle| {
                        this.child(div().text_xs().text_color(sub_color).truncate().child(subtitle))
                    }),
            )
            .child(trailing)
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
                    body.push(div().text_sm().text_color(rgb(pal.text)).child(fact).into_any_element());
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
                            .child(div().w(px(42.)).flex_shrink_0().text_color(rgb(pal.text)).child(name))
                            .child(div().flex_1().min_w(px(0.)).truncate().text_color(rgb(pal.text_dim)).child(condition))
                            .child(div().flex_shrink_0().text_color(rgb(pal.text)).child(high))
                            .child(div().flex_shrink_0().w(px(34.)).text_right().text_color(rgb(pal.text_faint)).child(low))
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
                                    .font_weight(if won { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
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
        let rich = matches!(hit.kind.as_str(), "definition" | "weather" | "stock" | "sports");

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
                        .child(div().text_xs().text_color(rgb(pal.text_dim)).child(preview.badge.clone())),
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
                        .child(div().text_xs().text_color(rgb(pal.text_dim)).child(preview.badge.clone())),
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
        let button = |id: &'static str, text: &'static str, primary: bool, reveal: bool, cx: &mut Context<Self>| {
            rounded_px(div(), 8.)
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .flex_1()
                .h(px(32.))
                .cursor_pointer()
                .text_sm()
                .when(primary, |this| this.bg(rgb(pal.accent)).text_color(rgb(pal.on_accent)))
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
                .child(button("preview-open", kind_meta(&hit.kind).1, true, false, cx))
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
                .max_h(px(LIST_MAX_H + 48.))
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

        match self.results.get(self.selected).filter(|hit| !is_inert(&hit.kind)) {
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
                .when(selected, |this| this.bg(rgb(pal.accent)).text_color(rgb(pal.on_accent)))
                .when(!selected, |this| {
                    this.text_color(rgb(pal.text)).hover(|style| style.bg(rgba(pal.hover)))
                })
                .on_click(cx.listener(move |this, _event, window, cx| {
                    this.perform(action, window, cx);
                }))
                .child(div().text_sm().child(item.label.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(if selected { rgba(pal.on_accent_dim) } else { rgb(pal.text_faint) })
                        .child(item.shortcut),
                )
        });

        let dismiss = |this: &mut Self, _event: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>| {
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
                    this.mouse_down_at(end, if event.click_count >= 2 { 3 } else { 1 }, event.modifiers.shift, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                let end = this.query.len();

                this.mouse_drag_to(end, event.pressed_button == Some(MouseButton::Left), cx);
            }));

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
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _window, cx| {
                    this.mouse_drag_to(index, event.pressed_button == Some(MouseButton::Left), cx);
                }))
        };

        let mut parts: Vec<AnyElement> = Vec::new();

        for (start, c) in self.query.char_indices() {
            let end = start + c.len_utf8();

            if show_caret && start == self.cursor {
                parts.push(caret().into_any_element());
            }

            let selected = start >= selection_start && end <= selection_end && selection_start < selection_end;

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

        field.children(parts).child(filler).into_any_element()
    }
}

impl Focusable for LauncherView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LauncherView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pal = self.palette;

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut last_section: Option<Section> = None;

        for index in 0..self.results.len() {
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

        let show_footer = self.settings.appearance.show_footer;
        let preview = self.render_preview(cx);
        let menu = self.render_menu(cx);
        let card = rounded_px(div(), pal.radius)
            .flex()
            .flex_col()
            .w_full()
            .bg(rgba(pal.card))
            .border_1()
            .border_color(rgba(pal.border))
            .overflow_hidden()
            .text_color(rgb(pal.text))
            // The search bar.
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_5()
                    .py_4()
                    // Drag the bar (outside the text) to move the launcher.
                    .when(self.settings.appearance.launcher_movable, |this| {
                        this.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_this, _event, window, _cx| begin_window_move(window)),
                        )
                    })
                    .child(self.glyph("icons/search.svg", 22., pal.text_faint))
                    .child(self.render_query(cx)),
            )
            .when(!rows.is_empty(), |this| {
                this.child(div().h(px(1.)).w_full().bg(rgba(pal.divider))).child(
                    div()
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .id("results")
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w(px(0.))
                                .max_h(px(LIST_MAX_H))
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
                        .pb_3()
                        .text_sm()
                        .text_color(rgb(pal.text_dim))
                        .child(notice),
                )
            })
            .when_some(self.status.clone(), |this, status| {
                this.child(
                    div()
                        .px_5()
                        .pb_3()
                        .text_sm()
                        .text_color(rgb(pal.danger))
                        .child(status),
                )
            })
            .when(show_footer, |this| this.child(self.render_footer(cx)));

        // The window is transparent: only the card is painted, so with no results it collapses
        // to the search bar. The menu floats above it.
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
            .w_full()
            .child(card)
            .when_some(menu, |this, menu| this.child(menu))
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
                let (screen_w, screen_h) = (f32::from(area.size.width), f32::from(area.size.height));
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
    clips: &mut Vec<String>,
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
    let track_position = can_place_windows()
        && settings.appearance.launcher_position == LauncherPosition::Remember;
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
                    let origin = (f32::from(origin.x).round() as i32, f32::from(origin.y).round() as i32);

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
fn push_clip(clips: &mut Vec<String>, text: String) {
    let text = text.trim().to_owned();

    if text.is_empty() || text.chars().count() > 4000 {
        return;
    }

    clips.retain(|existing| *existing != text);
    clips.insert(0, text);
    clips.truncate(50);
}

// ---- Window bookkeeping -----------------------------------------------------------------

#[derive(Default)]
struct UiState {
    launcher: Option<WindowHandle<LauncherView>>,
    settings: Option<WindowHandle<SettingsView>>,
    /// Clipboard history, newest first (memory only).
    clips: Vec<String>,
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
        UiMessage::Results { query, results, attempt } => {
            if let Some(handle) = state.launcher.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    // Ignore answers for a query the user has already edited.
                    if view.query == query {
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
        // The UI caps each section itself; ask for enough to fill all of them.
        limit: Some(40),
    });

    match send(socket, request).await? {
        Response::Search(response) => Ok(response.results),
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
                    let wait = if attempt == 0 { SEARCH_DEBOUNCE } else { POLL_DELAY };

                    tokio::time::sleep(wait).await;

                    if latest.load(Ordering::SeqCst) != id {
                        return;
                    }

                    let message = match search(&socket, &query).await {
                        Ok(results) => UiMessage::Results { query, results, attempt },
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
                let stamp = fs::metadata(Settings::path()).and_then(|meta| meta.modified()).ok();

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
        return;
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
                    let _ = cx.update(|cx| handle_message(cx, &mut state, &request_tx, message));
                }
            })
            .detach();
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_urls() {
        assert_eq!(url_from_query("github.com/rust-lang").as_deref(), Some("https://github.com/rust-lang"));
        assert_eq!(url_from_query("https://example.org").as_deref(), Some("https://example.org"));
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
