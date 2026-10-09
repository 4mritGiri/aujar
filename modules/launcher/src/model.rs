use serde::{Deserialize, Serialize};

/// What kind of thing a search result represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResultKind {
    Application,
    Calculation,
}

impl ResultKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Application => "application",
            Self::Calculation => "calculation",
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
}

/// A source of search results (applications, calculator, files, ...).
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;

    /// Return unranked-limit results for `query`; the launcher sorts and truncates.
    fn search(&self, query: &str) -> Vec<SearchResult>;
}
