//! Aujar launcher UI (GPUI): a Spotlight-style search window with PowerToys-style
//! plugins, and a settings window.
//!
//! Design: the UI holds **no search logic**. A background thread (tokio) talks to the
//! daemon over IPC; the GPUI thread only renders state and forwards keystrokes.
//!
//! Keys: type to search, Left/Right/Home/End to edit, Up/Down/Tab to select, Enter to
//! launch, Ctrl+Enter to show a file in its folder, Ctrl+C to copy, Alt+1..9 to launch a
//! row, Esc to clear/close. Limitation: plain key events only (no IME / composition yet).

mod settings_view;
mod theme;

use anyhow::{Result, bail};
use aujar_config::{History, Settings};
use aujar_ipc::{
    Event, ExecuteRequest, LauncherCommand, Request, Response, SearchHit, SearchRequest, send,
    subscribe,
};
use gpui::{
    AnyElement, App, Bounds, ClipboardItem, Context, FocusHandle, Focusable, KeyDownEvent,
    SharedString, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowOptions,
    div, img, prelude::*, px, rgb, rgba, size,
};
use gpui_platform::application;
use settings_view::SettingsView;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime},
};
use theme::Palette;

/// Window height; the card inside is content-sized and the rest is transparent.
const WINDOW_H: f32 = 580.0;

/// Bridge thread -> UI thread.
enum UiMessage {
    Launcher(LauncherCommand),
    Results {
        query: String,
        results: Vec<SearchHit>,
    },
    Failed(String),
    OpenSettings,
    /// New text on the system clipboard (from the watcher thread).
    Clipboard(String),
}

/// UI thread -> bridge thread.
enum UiRequest {
    Search(String),
    Execute(String),
    OpenSettings,
}

// ---- Sections ---------------------------------------------------------------------------

/// Declaration order is display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Recent,
    Frequent,
    Answer,
    Clipboard,
    Applications,
    Commands,
    Files,
    Web,
}

const SECTION_COUNT: usize = 8;

fn section_of(kind: &str) -> Section {
    match kind {
        "calculation" => Section::Answer,
        "clip" => Section::Clipboard,
        "application" => Section::Applications,
        "system" | "command" | "action" => Section::Commands,
        "file" | "folder" => Section::Files,
        _ => Section::Web,
    }
}

fn section_title(section: Section) -> &'static str {
    match section {
        Section::Recent => "Recent",
        Section::Frequent => "Frequently used",
        Section::Answer => "Answer",
        Section::Clipboard => "Clipboard",
        Section::Applications => "Applications",
        Section::Commands => "Commands",
        Section::Files => "Files & Folders",
        Section::Web => "Web",
    }
}

/// (badge, Enter-action label) for a result kind.
fn kind_meta(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "application" => ("App", "Open"),
        "calculation" => ("Answer", "Copy"),
        "clip" => ("Clip", "Copy"),
        "file" => ("File", "Open"),
        "folder" => ("Folder", "Open"),
        "system" => ("System", "Run"),
        "command" => ("Command", "Run"),
        "action" => ("Aujar", "Open"),
        _ => ("Web", "Open"),
    }
}

/// Results that end the session or the machine ask for a second Enter.
fn is_dangerous(id: &str) -> bool {
    matches!(id, "system:restart" | "system:shutdown" | "system:logout")
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
    }
}

// ---- Preview ----------------------------------------------------------------------------

/// Details shown in the right-hand panel for the selected result.
struct Preview {
    title: String,
    badge: &'static str,
    rows: Vec<(&'static str, String)>,
    /// First lines of a text file, or the full text of a clipboard entry.
    text: Option<String>,
    image: Option<PathBuf>,
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

fn build_preview(hit: &SearchHit, clips: &[String]) -> Preview {
    let (badge, _) = kind_meta(&hit.kind);

    let mut preview = Preview {
        title: hit.title.clone(),
        badge,
        rows: Vec::new(),
        text: None,
        image: None,
    };

    match hit.kind.as_str() {
        "file" | "folder" => {
            let Some(path) = hit.id.strip_prefix("files:open:").map(PathBuf::from) else {
                return preview;
            };

            if let Some(parent) = hit.subtitle.clone() {
                preview.rows.push(("Location", parent));
            }

            if let Ok(meta) = fs::metadata(&path) {
                if meta.is_dir() {
                    if let Ok(entries) = fs::read_dir(&path) {
                        preview.rows.push(("Contains", format!("{} items", entries.count())));
                    }
                } else {
                    preview.rows.push(("Size", human_size(meta.len())));
                }

                if let Ok(elapsed) = meta
                    .modified()
                    .and_then(|time| SystemTime::now().duration_since(time).map_err(std::io::Error::other))
                {
                    preview.rows.push(("Modified", age_text(elapsed)));
                }

                if meta.is_file() {
                    let extension = path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(str::to_ascii_lowercase)
                        .unwrap_or_default();

                    if IMAGE_EXTENSIONS.contains(&extension.as_str()) && meta.len() < 15_000_000 {
                        preview.image = Some(path.clone());
                    } else if TEXT_EXTENSIONS.contains(&extension.as_str()) && meta.len() < 1_000_000 {
                        preview.text = read_text_preview(&path);
                    }
                }
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
        _ => {
            if let Some(subtitle) = &hit.subtitle {
                preview.rows.push(("About", subtitle.clone()));
            }
        }
    }

    preview
}

// ---- Launcher view ----------------------------------------------------------------------

struct LauncherView {
    query: String,
    /// Caret position: a byte offset into `query`, always on a character boundary.
    cursor: usize,
    results: Vec<SearchHit>,
    /// Section of each result, same length as `results`.
    sections: Vec<Section>,
    selected: usize,
    status: Option<String>,
    /// A hint (not an error), shown dimmed.
    notice: Option<String>,
    /// Id of a dangerous result that is waiting for a second Enter.
    confirming: Option<String>,
    preview: Option<Preview>,
    settings: Settings,
    palette: Palette,
    history: History,
    /// Snapshot of the clipboard history taken when the launcher opened (newest first).
    clips: Vec<String>,
    requests: async_channel::Sender<UiRequest>,
    focus_handle: FocusHandle,
}

impl LauncherView {
    fn new(
        requests: async_channel::Sender<UiRequest>,
        settings: Settings,
        clips: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            query: String::new(),
            cursor: 0,
            results: Vec::new(),
            sections: Vec::new(),
            selected: 0,
            status: None,
            notice: None,
            confirming: None,
            preview: None,
            palette: Palette::from_settings(&settings),
            history: History::load_or_default(),
            settings,
            clips,
            requests,
            focus_handle: cx.focus_handle(),
        };

        view.show_empty_state();

        view
    }

    // ---- Query editing ----

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

    fn insert_text(&mut self, text: &str) {
        self.query.insert_str(self.cursor, text);
        self.cursor += text.len();
    }

    fn delete_range(&mut self, start: usize, end: usize) {
        self.query.replace_range(start..end, "");
        self.cursor = start;
    }

    /// Call after the query text changed.
    fn edited(&mut self, cx: &mut Context<Self>) {
        self.cursor = self.cursor.min(self.query.len());
        self.request_search();
        cx.notify();
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

        if self.query.trim().is_empty() {
            self.show_empty_state();
            return;
        }

        if let Some(filter) = self.clip_filter() {
            self.show_clips(&filter);
            return;
        }

        let _ = self.requests.try_send(UiRequest::Search(self.query.clone()));
    }

    fn install(&mut self, hits: Vec<SearchHit>, sections: Vec<Section>) {
        self.results = hits;
        self.sections = sections;
        self.selected = 0;
        self.confirming = None;
        self.refresh_preview();
    }

    fn refresh_preview(&mut self) {
        self.preview = if self.settings.appearance.show_preview {
            self.results
                .get(self.selected)
                .map(|hit| build_preview(hit, &self.clips))
        } else {
            None
        };
    }

    /// Empty search box: recently and frequently used things.
    fn show_empty_state(&mut self) {
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
        };

        let recent = self.history.recent(6);
        let recent_ids: Vec<String> = recent.iter().map(|entry| entry.id.clone()).collect();
        let skip: Vec<&str> = recent_ids.iter().map(String::as_str).collect();
        let frequent = self.history.frequent(4, &skip);

        let mut hits = Vec::new();
        let mut sections = Vec::new();

        for entry in recent {
            hits.push(as_hit(entry));
            sections.push(Section::Recent);
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
                Section::Applications => apps_cap,
                Section::Commands => 4,
                Section::Files => apps_cap.min(6),
                Section::Web => 3,
                Section::Recent | Section::Frequent | Section::Clipboard => usize::MAX,
            };

            counts[section as usize] += 1;
            counts[section as usize] <= cap
        });

        let sections = hits.iter().map(|hit| section_of(&hit.kind)).collect();

        self.install(hits, sections);
        self.status = None;
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.results.len() as isize;

        if len > 0 {
            self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
            self.confirming = None;
            self.refresh_preview();
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

        hit.title.clone()
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        let key = keystroke.key.as_str();

        // Alt+1..9 launches that row directly (Spotlight's Cmd+1..9).
        if modifiers.alt {
            if let Some(number) = key.parse::<usize>().ok().filter(|n| (1..=9).contains(n)) {
                if number <= self.results.len() {
                    self.selected = number - 1;
                    self.launch_selected(false, window, cx);
                }
                return;
            }
        }

        // Emacs/readline-style chords, word movement and copy.
        if modifiers.control {
            match key {
                "n" | "j" => self.move_selection(1, cx),
                "p" | "k" => self.move_selection(-1, cx),
                "enter" => self.launch_selected(true, window, cx),
                "c" => {
                    if let Some(hit) = self.results.get(self.selected) {
                        cx.write_to_clipboard(ClipboardItem::new_string(self.copy_text(hit)));
                        window.remove_window();
                    }
                }
                "a" => {
                    self.cursor = 0;
                    cx.notify();
                }
                "e" => {
                    self.cursor = self.query.len();
                    cx.notify();
                }
                "left" => {
                    self.cursor = self.word_left(self.cursor);
                    cx.notify();
                }
                "right" => {
                    self.cursor = self.word_right(self.cursor);
                    cx.notify();
                }
                "u" => {
                    self.query.clear();
                    self.cursor = 0;
                    self.edited(cx);
                }
                "w" | "backspace" => {
                    let start = self.word_left(self.cursor);
                    self.delete_range(start, self.cursor);
                    self.edited(cx);
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

                        self.insert_text(&line);
                        self.edited(cx);
                    }
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
                    self.query.clear();
                    self.cursor = 0;
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
                self.move_selection(if modifiers.shift { -1 } else { 1 }, cx);
                return;
            }
            "left" => {
                self.cursor = self.prev_boundary(self.cursor);
                cx.notify();
                return;
            }
            "right" => {
                self.cursor = self.next_boundary(self.cursor);
                cx.notify();
                return;
            }
            "home" => {
                self.cursor = 0;
                cx.notify();
                return;
            }
            "end" => {
                self.cursor = self.query.len();
                cx.notify();
                return;
            }
            "backspace" => {
                if self.cursor > 0 {
                    let start = self.prev_boundary(self.cursor);
                    self.delete_range(start, self.cursor);
                    self.edited(cx);
                }
                return;
            }
            "delete" => {
                if self.cursor < self.query.len() {
                    let end = self.next_boundary(self.cursor);
                    self.delete_range(self.cursor, end);
                    self.edited(cx);
                }
                return;
            }
            _ => {}
        }

        // Other chords (Super, ...) are not text.
        if modifiers.alt || modifiers.platform {
            return;
        }

        if let Some(text) = keystroke
            .key_char
            .as_ref()
            .filter(|text| !text.chars().any(char::is_control))
        {
            self.insert_text(text);
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

    fn leading(&self, hit: &SearchHit, tile: f32) -> AnyElement {
        if self.settings.appearance.show_icons {
            if let Some(icon) = hit.icon.as_deref() {
                return img(PathBuf::from(icon))
                    .size(px(tile))
                    .flex_shrink_0()
                    .into_any_element();
            }
        }

        let (color, glyph) = match hit.kind.as_str() {
            "folder" => (0xd9a23a, "F".to_owned()),
            "file" => (0x6b7a90, "D".to_owned()),
            "system" => (0xc2414f, "S".to_owned()),
            "command" => (0x3d3d48, ">".to_owned()),
            "clip" => (0x6b5bd6, "C".to_owned()),
            "action" => (self.settings.appearance.accent.rgb(), "A".to_owned()),
            "web" => (0x2f8fd8, "\u{2197}".to_owned()),
            _ => {
                const PALETTE: [u32; 6] =
                    [0x5e5ce6, 0x30a46c, 0xe5732d, 0xd6409f, 0x2f8fd8, 0x8e6bd8];
                let initial = hit
                    .title
                    .chars()
                    .find(|c| c.is_alphanumeric())
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_owned());
                let sum: usize = hit.title.bytes().map(usize::from).sum();

                (PALETTE[sum % PALETTE.len()], initial)
            }
        };

        div()
            .flex()
            .items_center()
            .justify_center()
            .size(px(tile))
            .flex_shrink_0()
            .rounded_lg()
            .bg(rgb(color))
            .text_color(rgb(0xffffff))
            .text_lg()
            .child(glyph)
            .into_any_element()
    }

    fn keycap(&self, label: &str) -> impl IntoElement {
        div()
            .px_2()
            .py_0p5()
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

        let title_color = if selected { rgb(pal.on_accent) } else { rgb(pal.text) };
        let sub_color = if selected { rgba(pal.on_accent_dim) } else { rgb(pal.text_dim) };

        let base = div()
            .id(SharedString::from(format!("row-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .mx_2()
            .px_3()
            .rounded_lg()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.selected = index;
                this.launch_selected(false, window, cx);
            }));

        let base = if selected {
            base.bg(rgb(if dangerous { pal.danger } else { pal.accent }))
        } else {
            base.hover(|style| style.bg(rgba(pal.hover)))
        };

        let (badge, _) = kind_meta(&hit.kind);
        let trailing = if selected {
            if confirming { "Enter again".to_owned() } else { "Enter".to_owned() }
        } else if index < 9 {
            format!("Alt+{}", index + 1)
        } else {
            badge.to_owned()
        };

        if hit.kind == "calculation" {
            // Answer card: the result is the star, like Spotlight's calculator.
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
                        .child(div().text_3xl().text_color(title_color).child(hit.title.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(sub_color)
                        .child(if selected { "Enter to copy".to_owned() } else { trailing }),
                )
                .into_any_element();
        }

        let subtitle = if confirming {
            Some("Press Enter again to confirm".to_owned())
        } else {
            hit.subtitle.clone()
        };

        base.py_2()
            .child(self.leading(hit, 36.))
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
            .child(div().text_xs().text_color(sub_color).child(trailing))
            .into_any_element()
    }

    fn render_preview(&self) -> Option<AnyElement> {
        let preview = self.preview.as_ref()?;
        let hit = self.results.get(self.selected)?;
        let pal = self.palette;

        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .w(px(270.))
                .flex_shrink_0()
                .p_4()
                .border_l_1()
                .border_color(rgba(pal.divider))
                .child(self.leading(hit, 56.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_lg().text_color(rgb(pal.text)).child(preview.title.clone()))
                        .child(div().text_xs().text_color(rgb(pal.text_dim)).child(preview.badge)),
                )
                .when_some(preview.image.clone(), |this, path| {
                    this.child(img(path).w_full().max_h(px(170.)).rounded_md())
                })
                .children(preview.rows.iter().map(|(label, value)| {
                    div()
                        .flex()
                        .flex_col()
                        .child(div().text_xs().text_color(rgb(pal.text_dim)).child(*label))
                        .child(div().text_sm().text_color(rgb(pal.text)).child(value.clone()))
                }))
                .when_some(preview.text.clone(), |this, text| {
                    this.child(
                        div()
                            .p_2()
                            .rounded_md()
                            .bg(rgba(pal.subtle))
                            .overflow_hidden()
                            .text_xs()
                            .text_color(rgb(pal.text_dim))
                            .child(text),
                    )
                })
                .into_any_element(),
        )
    }

    fn render_footer(&self) -> impl IntoElement {
        let pal = self.palette;
        let mut hints: Vec<(&'static str, &'static str)> = Vec::new();

        match self.results.get(self.selected) {
            Some(hit) => {
                hints.push(("Enter", kind_meta(&hit.kind).1));

                match hit.kind.as_str() {
                    "file" | "folder" => {
                        hints.push(("Ctrl+Enter", "Show in folder"));
                        hints.push(("Ctrl+C", "Copy path"));
                    }
                    "calculation" => {}
                    _ => hints.push(("Ctrl+C", "Copy")),
                }

                hints.push(("Alt+1-9", "Quick open"));
            }
            None => {
                hints.push(("g query", "Web shortcut"));
                hints.push(("> cmd", "Run command"));
                hints.push(("10 km to mi", "Convert"));

                if self.settings.plugins.clipboard {
                    hints.push(("clip", "Clipboard"));
                }
            }
        }

        hints.push(("Esc", "Close"));

        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_4()
            .px_5()
            .py_2()
            .border_t_1()
            .border_color(rgba(pal.divider))
            .bg(rgba(pal.subtle))
            .children(hints.into_iter().map(|(keys, label)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1p5()
                    .child(self.keycap(keys))
                    .child(div().text_xs().text_color(rgb(pal.text_dim)).child(label))
            }))
    }

    /// The search field with a caret at the cursor position.
    fn render_query(&self) -> AnyElement {
        let pal = self.palette;

        let caret = || {
            div()
                .w(px(2.))
                .h(px(26.))
                .flex_shrink_0()
                .bg(rgb(pal.accent))
        };

        let field = div().flex().flex_row().items_center().flex_1().text_2xl();

        if self.query.is_empty() {
            return field
                .child(caret())
                .child(
                    div()
                        .pl_1()
                        .text_color(rgb(pal.text_dim))
                        .child("Search apps, files, math and the web"),
                )
                .into_any_element();
        }

        let (before, after) = self.query.split_at(self.cursor.min(self.query.len()));

        field
            .text_color(rgb(pal.text))
            .child(before.to_owned())
            .child(caret())
            .child(after.to_owned())
            .into_any_element()
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
            let hit = self.results[index].clone();
            let section = self.sections.get(index).copied().unwrap_or(Section::Web);

            if last_section != Some(section) {
                rows.push(
                    div()
                        .px_5()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .text_color(rgb(pal.text_dim))
                        .child(section_title(section))
                        .into_any_element(),
                );
                last_section = Some(section);
            }

            rows.push(self.render_row(index, &hit, cx));
        }

        let show_footer = self.settings.appearance.show_footer;
        let preview = self.render_preview();

        // The window is transparent: only this card is painted, so with no results it
        // collapses to the search bar (plus the hint bar), like Spotlight.
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .w_full()
            .bg(rgba(pal.card))
            .border_1()
            .border_color(rgba(pal.border))
            .rounded_xl()
            .overflow_hidden()
            .text_color(rgb(pal.text))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .px_5()
                    .py_4()
                    .child(
                        div()
                            .size(px(18.))
                            .flex_shrink_0()
                            .rounded_full()
                            .border_2()
                            .border_color(rgb(pal.text_dim)),
                    )
                    .child(self.render_query())
                    .child(self.keycap("Esc")),
            )
            .when(!rows.is_empty(), |this| {
                this.child(div().h(px(1.)).w_full().bg(rgba(pal.divider))).child(
                    div()
                        .flex()
                        .flex_row()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .overflow_hidden()
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
            .when(show_footer, |this| this.child(self.render_footer()))
    }
}

fn open_launcher(
    cx: &mut App,
    requests: async_channel::Sender<UiRequest>,
    clips: &mut Vec<String>,
) -> Option<WindowHandle<LauncherView>> {
    // Re-read the settings every time, so changes apply on the next open.
    let settings = Settings::load_or_default();
    let width = settings.appearance.width as f32;
    let bounds = Bounds::centered(None, size(px(width), px(WINDOW_H)), cx);

    // Best effort: whatever is on the clipboard now joins the history (on GNOME/Wayland the
    // background watcher cannot see the clipboard, so this is how entries get captured).
    if settings.plugins.clipboard {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            push_clip(clips, text);
        }
    }

    let close_on_blur = settings.appearance.close_on_blur;
    let snapshot = clips.clone();

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        app_id: Some("aujar-launcher".into()),
        ..Default::default()
    };

    cx.open_window(options, move |window, cx| {
        let view = cx.new(|cx| LauncherView::new(requests, settings, snapshot, cx));
        let focus_handle = view.read(cx).focus_handle.clone();

        // Without focus, key events never reach the view.
        window.focus(&focus_handle, cx);

        // Click-away closes the launcher. The grace period ignores the focus churn that
        // compositors (especially Wayland) produce while a new window is being mapped.
        if close_on_blur {
            let opened = Instant::now();

            view.update(cx, |_view, cx| {
                cx.observe_window_activation(window, move |_this, window, _cx| {
                    if !window.is_window_active() && opened.elapsed() > Duration::from_millis(500)
                    {
                        window.remove_window();
                    }
                })
                .detach();
            });
        }

        view
    })
    .ok()
}

fn open_settings(cx: &mut App) -> Option<WindowHandle<SettingsView>> {
    let bounds = Bounds::centered(None, size(px(760.), px(700.)), cx);

    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
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
        UiMessage::Results { query, results } => {
            if let Some(handle) = state.launcher.as_ref() {
                let _ = handle.update(cx, |view, _, cx| {
                    // Ignore answers for a query the user has already edited.
                    if view.query == query {
                        view.set_results(results);
                        cx.notify();
                    }
                });
            }
        }
        UiMessage::Clipboard(text) => push_clip(&mut state.clips, text),
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
    while let Ok(request) = requests.recv().await {
        match request {
            UiRequest::Search(query) => {
                let message = match search(&socket, &query).await {
                    Ok(results) => UiMessage::Results { query, results },
                    Err(error) => UiMessage::Failed(format!("{error:#}")),
                };

                if tx.send(message).await.is_err() {
                    break;
                }
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
        let mut failures = 0u32;
        let mut ticks = 0u32;
        let mut last = String::new();

        loop {
            std::thread::sleep(Duration::from_millis(1500));

            // The setting can change while we run; re-read it every few seconds.
            if ticks % 4 == 0 {
                enabled = Settings::load_or_default().plugins.clipboard;
            }

            ticks = ticks.wrapping_add(1);

            if !enabled {
                continue;
            }

            match Command::new(program).args(args).output() {
                Ok(output) if output.status.success() => {
                    failures = 0;

                    if let Ok(text) = String::from_utf8(output.stdout) {
                        if text != last {
                            last = text.clone();

                            if tx.send_blocking(UiMessage::Clipboard(text)).is_err() {
                                return;
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

fn main() {
    let (message_tx, message_rx) = async_channel::unbounded::<UiMessage>();
    let (request_tx, request_rx) = async_channel::unbounded::<UiRequest>();

    spawn_clipboard_watcher(message_tx.clone());
    spawn_bridge(socket_path(), message_tx, request_rx);

    // Closing the launcher window (Esc / Enter) must not end the process: the UI has to
    // stay alive to receive the next toggle. By default GPUI quits with the last window.
    application()
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
