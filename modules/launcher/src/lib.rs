mod apps;
mod calculator;
mod model;
mod ranking;

pub use apps::{AppsProvider, DesktopEntry, default_dirs, parse_desktop_entry, parse_exec};
pub use calculator::{CalculatorProvider, evaluate};
pub use model::{LaunchSpec, Provider, ResultKind, SearchResult};
pub use ranking::{score, score_strict};

#[derive(Debug, Clone)]
pub struct LauncherQuery {
    pub text: String,
}

impl LauncherQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// Aggregates providers and returns ranked results.
pub struct Launcher {
    providers: Vec<Box<dyn Provider>>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Launcher {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Calculator plus installed desktop applications.
    pub fn with_default_providers() -> Self {
        let mut launcher = Self::new();

        launcher.register(Box::new(CalculatorProvider));
        launcher.register(Box::new(AppsProvider::from_env()));

        launcher
    }

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        self.providers.push(provider);
    }

    /// Resolve a result id into a launch command. Only ids that a registered
    /// provider recognises can be launched; arbitrary commands are impossible.
    pub fn resolve(&self, id: &str) -> Result<LaunchSpec, String> {
        let (provider_id, _) = id.split_once(':').ok_or_else(|| format!("malformed result id `{id}`"))?;

        self.providers
            .iter()
            .filter(|provider| provider.id() == provider_id)
            .find_map(|provider| provider.resolve(id))
            .unwrap_or_else(|| {
                Err(format!(
                    "no launchable result with id `{id}` (run `aujar search <text>` to list valid ids)"
                ))
            })
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query = query.trim();

        if query.is_empty() || limit == 0 {
            return Vec::new();
        }

        let mut results: Vec<SearchResult> = self
            .providers
            .iter()
            .flat_map(|provider| provider.search(query))
            .collect();

        results.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.cmp(&b.title)));
        results.truncate(limit);

        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Vec<SearchResult>);

    impl Provider for Fixed {
        fn id(&self) -> &'static str {
            "fixed"
        }

        fn search(&self, _query: &str) -> Vec<SearchResult> {
            self.0.clone()
        }
    }

    fn result(title: &str, score: u32) -> SearchResult {
        SearchResult {
            id: format!("fixed:{title}"),
            title: title.into(),
            subtitle: None,
            kind: ResultKind::Application,
            score,
        }
    }

    #[test]
    fn sorts_by_score_then_title_and_truncates() {
        let mut launcher = Launcher::new();
        launcher.register(Box::new(Fixed(vec![
            result("b", 10),
            result("a", 10),
            result("c", 50),
        ])));

        let titles: Vec<_> = launcher
            .search("x", 2)
            .into_iter()
            .map(|r| r.title)
            .collect();

        assert_eq!(titles, ["c", "a"]);
    }

    #[test]
    fn empty_query_returns_nothing() {
        let mut launcher = Launcher::new();
        launcher.register(Box::new(Fixed(vec![result("a", 1)])));

        assert!(launcher.search("  ", 10).is_empty());
    }

    #[test]
    fn calculator_ranks_first() {
        let mut launcher = Launcher::new();
        launcher.register(Box::new(Fixed(vec![result("2+2 app", 900)])));
        launcher.register(Box::new(CalculatorProvider));

        let results = launcher.search("2+2", 5);

        assert_eq!(results[0].kind, ResultKind::Calculation);
        assert_eq!(results[0].title, "4");
    }
}
