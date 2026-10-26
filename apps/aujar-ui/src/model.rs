//! UI logic that does not need a window: how results are grouped, which actions a result
//! offers, and how the daemon's `detail` text becomes card rows. Kept free of GPUI so it can
//! be unit-tested without a display.

use aujar_ipc::SearchHit;

// ---- Sections ---------------------------------------------------------------------------

/// Declaration order is display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Recent,
    RecentFiles,
    Frequent,
    Answer,
    /// Definitions, weather, quotes, scores, and the placeholders shown while they load.
    Lookup,
    TopHit,
    Clipboard,
    Applications,
    Windows,
    Commands,
    Files,
    Web,
}

pub const SECTION_COUNT: usize = 12;

pub fn section_of(kind: &str) -> Section {
    match kind {
        "calculation" => Section::Answer,
        "definition" | "weather" | "stock" | "sports" | "loading" | "notice" => Section::Lookup,
        "clip" => Section::Clipboard,
        "application" => Section::Applications,
        "window" => Section::Windows,
        "system" | "command" | "action" => Section::Commands,
        "file" | "folder" => Section::Files,
        _ => Section::Web,
    }
}

pub fn section_title(section: Section) -> &'static str {
    match section {
        Section::Recent => "Recent",
        Section::RecentFiles => "Recent Files",
        Section::Frequent => "Frequently Used",
        Section::Answer => "Answer",
        Section::Lookup => "Lookup",
        Section::TopHit => "Top Hit",
        Section::Clipboard => "Clipboard",
        Section::Applications => "Applications",
        Section::Windows => "Windows",
        Section::Commands => "Commands",
        Section::Files => "Files & Folders",
        Section::Web => "Web",
    }
}

/// (label shown at the right of a row, what Enter does) for a result kind.
pub fn kind_meta(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "application" => ("Application", "Open"),
        "calculation" => ("Answer", "Copy"),
        "clip" => ("Clipboard", "Copy"),
        "file" => ("File", "Open"),
        "folder" => ("Folder", "Open"),
        "system" => ("System", "Run"),
        "command" => ("Command", "Run"),
        "action" => ("Aujar", "Open"),
        "definition" => ("Dictionary", "Open entry"),
        "weather" => ("Weather", "Open forecast"),
        "stock" => ("Stocks", "Open quote"),
        "sports" => ("Sports", "Open game"),
        "window" => ("Window", "Switch"),
        "loading" => ("", "Wait"),
        "notice" => ("", "Dismiss"),
        _ => ("Web", "Open"),
    }
}

/// Results that end the session or the machine ask for a second Enter.
pub fn is_dangerous(id: &str) -> bool {
    matches!(id, "system:restart" | "system:shutdown" | "system:logout")
}

/// Kinds drawn as a larger card instead of a one-line row.
pub fn is_card(kind: &str) -> bool {
    matches!(
        kind,
        "calculation" | "definition" | "weather" | "stock" | "sports"
    )
}

/// Kinds that cannot be launched; Enter does nothing for them.
pub fn is_inert(kind: &str) -> bool {
    matches!(kind, "loading" | "notice")
}

/// Whether the list still contains a placeholder, i.e. the daemon is fetching something and
/// the UI should ask again shortly.
pub fn has_loading(hits: &[SearchHit]) -> bool {
    hits.iter().any(|hit| hit.kind == "loading")
}

/// The icon (an asset path) that stands for a result kind.
pub fn icon_for_kind(kind: &str, id: &str) -> &'static str {
    match kind {
        "folder" => "icons/folder.svg",
        "file" => "icons/file.svg",
        "application" => "icons/app.svg",
        "calculation" => "icons/calculator.svg",
        "system" => "icons/power.svg",
        "command" => "icons/terminal.svg",
        "clip" => "icons/clipboard.svg",
        "action" => "icons/sliders.svg",
        "definition" => "icons/book.svg",
        "weather" => weather_icon(id),
        "stock" => "icons/trend.svg",
        "sports" => "icons/trophy.svg",
        "window" => "icons/window.svg",
        "loading" => "icons/loader.svg",
        "notice" => "icons/alert.svg",
        _ => "icons/globe.svg",
    }
}

/// Weather ids look like `weather:<icon-key>:<place>`.
pub fn weather_icon(id: &str) -> &'static str {
    match id.split(':').nth(1).unwrap_or("") {
        "clear" => "icons/sun.svg",
        "clear-night" => "icons/moon.svg",
        "rain" => "icons/rain.svg",
        "snow" => "icons/snow.svg",
        "storm" => "icons/storm.svg",
        "fog" => "icons/fog.svg",
        _ => "icons/cloud.svg",
    }
}

/// Tile color behind a glyph for kinds that have no application icon, `0xRRGGBB`.
pub fn tile_color(kind: &str, id: &str, accent: u32) -> u32 {
    match kind {
        "folder" => 0xe0a526,
        "file" => 0x6f7f95,
        "system" => 0xd8434f,
        "command" => 0x3f3f4a,
        "clip" => 0x6b5bd6,
        "action" => accent,
        "definition" => 0xc0392b,
        "weather" => match id.split(':').nth(1).unwrap_or("") {
            "clear" => 0xf0a30a,
            "clear-night" => 0x4a4e9c,
            "rain" | "storm" => 0x3d6fb4,
            "snow" => 0x5aa9d6,
            _ => 0x4f8fd4,
        },
        "stock" => 0x1f9d63,
        "sports" => 0xe5732d,
        "window" => 0x5e5ce6,
        "loading" | "notice" => 0x71717a,
        _ => 0x2f8fd8,
    }
}

// ---- Look up (Spotlight's Cmd+L) ----------------------------------------------------------

/// The `define <word>` query for whatever the person is looking at: the selected text, else the
/// query, else the selected result's title. `None` when there is no plausible word.
pub fn define_query(candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|text| {
        let text = text.trim();
        let lower = text.to_lowercase();

        // Already a dictionary query: keep it.
        if let Some(word) = lower.strip_prefix("define ") {
            return plausible_word(word.trim()).map(|word| format!("define {word}"));
        }

        text.split(|c: char| !(c.is_alphabetic() || c == '-' || c == '\''))
            .find(|word| plausible_word(word).is_some())
            .and_then(plausible_word)
            .map(|word| format!("define {word}"))
    })
}

fn plausible_word(word: &str) -> Option<String> {
    let word = word.trim_matches(|c| c == '-' || c == '\'').to_lowercase();
    let count = word.chars().count();

    ((2..=40).contains(&count)
        && word
            .chars()
            .all(|c| c.is_alphabetic() || c == '-' || c == '\''))
    .then_some(word)
}

// ---- Context menu -------------------------------------------------------------------------

/// What a context-menu entry (or its shortcut) does to the selected result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Same as Enter.
    Primary,
    /// Show a file or folder in the file manager.
    Reveal,
    /// Copy a path, link, answer, definition or text.
    Copy,
    /// Put `define <word>` in the search box.
    LookUp,
    /// Search the web for the query.
    WebSearch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub action: Action,
    pub label: String,
    pub shortcut: &'static str,
}

fn item(action: Action, label: impl Into<String>, shortcut: &'static str) -> MenuItem {
    MenuItem {
        action,
        label: label.into(),
        shortcut,
    }
}

/// The right-click menu for a result. `can_look_up` says whether a dictionary word is available.
pub fn menu_items(hit: &SearchHit, can_look_up: bool) -> Vec<MenuItem> {
    let (_, primary) = kind_meta(&hit.kind);
    let mut items = Vec::new();

    if is_inert(&hit.kind) {
        return items;
    }

    items.push(item(Action::Primary, primary, "Enter"));

    match hit.kind.as_str() {
        "file" | "folder" => {
            items.push(item(Action::Reveal, "Show in Folder", "Ctrl+Enter"));
            items.push(item(Action::Copy, "Copy Path", "Ctrl+C"));
        }
        "web" => items.push(item(Action::Copy, "Copy Link", "Ctrl+C")),
        "calculation" => items.push(item(Action::Copy, "Copy Answer", "Ctrl+C")),
        "definition" => items.push(item(Action::Copy, "Copy Definition", "Ctrl+C")),
        "weather" | "stock" | "sports" | "clip" => items.push(item(Action::Copy, "Copy", "Ctrl+C")),
        "application" | "system" | "command" | "action" | "window" => {}
        _ => items.push(item(Action::Copy, "Copy", "Ctrl+C")),
    }

    if can_look_up {
        items.push(item(Action::LookUp, "Look Up in Dictionary", "Ctrl+L"));
    }

    items.push(item(Action::WebSearch, "Search the Web", "Ctrl+B"));

    items
}

// ---- Card data (parsed from `SearchHit::detail`) -------------------------------------------

/// First line and the lines after it, for definitions: `(first definition, numbered lines)`.
pub fn definition_parts(detail: &str) -> (String, Vec<String>) {
    let mut lines = detail.lines();
    let first = lines.next().unwrap_or("").to_owned();

    (first, lines.map(str::to_owned).collect())
}

/// A day of the forecast: `(name, condition, high, low)`.
pub type ForecastDay = (String, String, String, String);

/// Weather detail: free-text facts, a blank line, then `name|condition|high|low` per day.
pub fn weather_parts(detail: &str) -> (Vec<String>, Vec<ForecastDay>) {
    let mut facts = Vec::new();
    let mut days = Vec::new();

    for line in detail.lines() {
        let fields: Vec<&str> = line.split('|').collect();

        if let [name, condition, high, low] = fields[..] {
            days.push((
                name.to_owned(),
                condition.to_owned(),
                high.to_owned(),
                low.to_owned(),
            ));
        } else if !line.trim().is_empty() {
            facts.push(line.to_owned());
        }
    }

    (facts, days)
}

/// Quote detail: `label|value` lines.
pub fn stock_rows(detail: &str) -> Vec<(String, String)> {
    detail
        .lines()
        .filter_map(|line| line.split_once('|'))
        .map(|(label, value)| (label.to_owned(), value.to_owned()))
        .collect()
}

/// `true` for a rise, `false` for a fall, from the arrow that starts a quote's subtitle.
pub fn price_direction(subtitle: &str) -> Option<bool> {
    match subtitle.trim_start().chars().next() {
        Some('\u{25b2}') => Some(true),
        Some('\u{25bc}') => Some(false),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scoreboard {
    pub away: String,
    pub home: String,
    pub away_score: String,
    pub home_score: String,
    /// `"away"`, `"home"` or empty.
    pub winner: String,
}

/// Sports detail: `away|home|away score|home score|winner`.
pub fn scoreboard(detail: &str) -> Option<Scoreboard> {
    let fields: Vec<&str> = detail.lines().next()?.split('|').collect();

    let [away, home, away_score, home_score, winner] = fields[..] else {
        return None;
    };

    Some(Scoreboard {
        away: away.to_owned(),
        home: home.to_owned(),
        away_score: away_score.to_owned(),
        home_score: home_score.to_owned(),
        winner: winner.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(kind: &str, id: &str) -> SearchHit {
        SearchHit {
            id: id.to_owned(),
            title: "t".to_owned(),
            subtitle: None,
            kind: kind.to_owned(),
            score: 0,
            icon: None,
            detail: None,
        }
    }

    #[test]
    fn every_kind_has_a_section_icon_and_meta() {
        for kind in [
            "application",
            "calculation",
            "file",
            "folder",
            "system",
            "command",
            "clip",
            "action",
            "definition",
            "weather",
            "stock",
            "sports",
            "window",
            "loading",
            "notice",
            "web",
        ] {
            let _ = section_of(kind);
            assert!(icon_for_kind(kind, "x:y:z").starts_with("icons/"));
            assert!(!kind_meta(kind).1.is_empty());
        }

        assert_eq!(section_of("weather"), Section::Lookup);
        assert_eq!(section_of("window"), Section::Windows);
        assert!((Section::Web as usize) < SECTION_COUNT);
    }

    #[test]
    fn section_order_puts_answers_before_applications() {
        assert!((Section::Answer as usize) < (Section::Lookup as usize));
        assert!((Section::Lookup as usize) < (Section::TopHit as usize));
        assert!((Section::Applications as usize) < (Section::Files as usize));
    }

    #[test]
    fn loading_is_detected() {
        assert!(has_loading(&[
            hit("application", "a"),
            hit("loading", "loading:x")
        ]));
        assert!(!has_loading(&[hit("notice", "n")]));
    }

    #[test]
    fn weather_ids_pick_icons() {
        assert_eq!(weather_icon("weather:rain:Paris"), "icons/rain.svg");
        assert_eq!(weather_icon("weather:clear-night:X"), "icons/moon.svg");
        assert_eq!(weather_icon("weather:???:X"), "icons/cloud.svg");
    }

    #[test]
    fn look_up_prefers_the_first_usable_source() {
        assert_eq!(
            define_query(&["", "serendipity"]).as_deref(),
            Some("define serendipity")
        );
        assert_eq!(
            define_query(&["Ubiquitous", "other"]).as_deref(),
            Some("define ubiquitous")
        );
        assert_eq!(define_query(&["define cat"]).as_deref(), Some("define cat"));
        assert_eq!(define_query(&["a 12 b"]), None);
        assert_eq!(
            define_query(&["Mozilla Firefox"]).as_deref(),
            Some("define mozilla")
        );
        assert_eq!(define_query(&[]), None);
    }

    #[test]
    fn menus_depend_on_the_kind() {
        let file = menu_items(&hit("file", "files:open:/x"), true);
        let labels: Vec<&str> = file.iter().map(|item| item.label.as_str()).collect();

        assert_eq!(
            labels,
            [
                "Open",
                "Show in Folder",
                "Copy Path",
                "Look Up in Dictionary",
                "Search the Web"
            ]
        );

        let app = menu_items(&hit("application", "apps:x"), false);

        assert_eq!(app.len(), 2);
        assert_eq!(app[0].action, Action::Primary);
        assert!(menu_items(&hit("loading", "loading:x"), true).is_empty());
        assert!(menu_items(&hit("notice", "notice:x"), true).is_empty());
    }

    #[test]
    fn parses_weather_detail() {
        let (facts, days) = weather_parts(
            "Partly cloudy\nHumidity 62%\n\nToday|Partly cloudy|20\u{b0}C|11\u{b0}C\nMon|Light rain|17\u{b0}C|10\u{b0}C",
        );

        assert_eq!(facts, ["Partly cloudy", "Humidity 62%"]);
        assert_eq!(days.len(), 2);
        assert_eq!(days[1].0, "Mon");
    }

    #[test]
    fn parses_stock_and_score_detail() {
        assert_eq!(stock_rows("Volume|48M\nbad line\nExchange|Nasdaq").len(), 2);
        assert_eq!(price_direction("\u{25b2} 2.48 (1.10%)"), Some(true));
        assert_eq!(price_direction("\u{25bc} 1"), Some(false));
        assert_eq!(price_direction("No change data"), None);

        let board = scoreboard("Lakers|Celtics|99|102|home").unwrap();

        assert_eq!(board.home_score, "102");
        assert_eq!(board.winner, "home");
        assert!(scoreboard("nonsense").is_none());
    }

    #[test]
    fn splits_definition_detail() {
        let (first, lines) =
            definition_parts("The first.\n1. The first.\n   \u{201c}Example\u{201d}");

        assert_eq!(first, "The first.");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn dangerous_actions_need_confirmation() {
        assert!(is_dangerous("system:shutdown"));
        assert!(!is_dangerous("system:lock"));
        assert!(is_card("weather") && !is_card("application"));
        assert!(is_inert("notice") && !is_inert("window"));
    }
}
