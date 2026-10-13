use serde::{Deserialize, Serialize};

/// What kind of thing a search result represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultKind {
    Application,
    /// A computed answer (arithmetic, unit conversion). Enter copies it.
    Calculation,
    File,
    Folder,
    /// Lock, sleep, restart, ...
    System,
    /// A shell command typed after `>`.
    Command,
    /// A dictionary entry (`define serendipity`). Enter opens the full entry online.
    Definition,
    /// Current weather and a short forecast (`weather paris`).
    Weather,
    /// A stock quote (`stock aapl`, `$tsla`).
    Stock,
    /// A game from a league scoreboard (`nba`, `premier league arsenal`).
    Sports,
    /// An open window that can be switched to (`< firefox`).
    Window,
    /// A lookup that is still being fetched. The UI asks again shortly.
    Loading,
    /// A message instead of a result: a failed lookup or a missing backend. Not launchable.
    Notice,
}

impl ResultKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Calculation => "calculation",
            Self::File => "file",
            Self::Folder => "folder",
            Self::System => "system",
            Self::Command => "command",
            Self::Definition => "definition",
            Self::Weather => "weather",
            Self::Stock => "stock",
            Self::Sports => "sports",
            Self::Window => "window",
            Self::Loading => "loading",
            Self::Notice => "notice",
        }
    }
}

/// A single ranked launcher result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    /// Stable identifier, prefixed by the provider id (e.g. `apps:firefox.desktop`).
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub kind: ResultKind,
    /// Higher is better. Only comparable within one search.
    pub score: u32,
    /// Absolute path of a PNG/SVG icon, when one was found.
    pub icon: Option<String>,
    /// Longer text for the preview pane (definitions, a forecast, quote details), one item
    /// per line. `None` for results that have nothing more to say.
    #[serde(default)]
    pub detail: Option<String>,
}

impl SearchResult {
    /// A result with no subtitle, icon or detail; chain the `with_*` helpers to fill them in.
    pub fn new(id: impl Into<String>, title: impl Into<String>, kind: ResultKind, score: u32) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            kind,
            score,
            icon: None,
            detail: None,
        }
    }

    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn with_optional_subtitle(mut self, subtitle: Option<String>) -> Self {
        self.subtitle = subtitle;
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_icon(mut self, icon: Option<String>) -> Self {
        self.icon = icon;
        self
    }
}

/// A resolved, shell-free command line for launching a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub title: String,
    pub program: String,
    pub args: Vec<String>,
}

/// A source of search results (applications, calculator, files, ...).
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;

    /// Return unranked-limit results for `query`; the launcher sorts and truncates.
    fn search(&self, query: &str) -> Vec<SearchResult>;

    /// Resolve a result id produced by this provider into a launch command.
    /// `None` means the provider does not own `id`; `Some(Err(..))` means it
    /// does but the result cannot be launched.
    fn resolve(&self, _id: &str) -> Option<Result<LaunchSpec, String>> {
        None
    }
}
