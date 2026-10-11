mod apps;
mod calculator;
mod currency;
mod files;
mod icons;
mod model;
mod ranking;
mod shell;
mod system;
mod units;

pub use apps::{AppsProvider, DesktopEntry, default_dirs, parse_desktop_entry, parse_exec};
pub use calculator::{CalculatorProvider, evaluate};
pub use currency::CurrencyProvider;
pub use files::FilesProvider;
pub use icons::resolve_icon;
pub use model::{LaunchSpec, Provider, ResultKind, SearchResult};
pub use ranking::{score, score_strict};
pub use shell::ShellProvider;
pub use system::SystemProvider;
pub use units::{UnitsProvider, convert_query};

/// Which providers a [`Launcher`] is built with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LauncherOptions {
    pub applications: bool,
    pub calculator: bool,
    pub units: bool,
    /// Downloads exchange rates, so it is opt-in.
    pub currency: bool,
    pub files: bool,
    pub system: bool,
    /// Runs arbitrary commands, so it is opt-in.
    pub shell: bool,
    pub file_roots: Vec<String>,
    pub file_depth: usize,
}

impl Default for LauncherOptions {
    fn default() -> Self {
        Self {
            applications: true,
            calculator: true,
            units: true,
            currency: false,
            files: true,
            system: true,
            shell: false,
            file_roots: vec!["~".to_owned()],
            file_depth: 4,
        }
    }
}

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

    /// Every default provider except the opt-in shell.
    pub fn with_default_providers() -> Self {
        Self::with_options(&LauncherOptions::default())
    }

    pub fn with_options(options: &LauncherOptions) -> Self {
        let mut launcher = Self::new();

        if options.calculator {
            launcher.register(Box::new(CalculatorProvider));
        }

        if options.units {
            launcher.register(Box::new(UnitsProvider));
        }

        if options.currency {
            launcher.register(Box::new(CurrencyProvider::new()));
        }

        if options.applications {
            launcher.register(Box::new(AppsProvider::from_env()));
        }

        if options.files {
            launcher.register(Box::new(FilesProvider::new(
                &options.file_roots,
                options.file_depth,
            )));
        }

        if options.system {
            launcher.register(Box::new(SystemProvider::new()));
        }

        if options.shell {
            launcher.register(Box::new(ShellProvider));
        }

        launcher
    }

    pub fn register(&mut self, provider: Box<dyn Provider>) {
        self.providers.push(provider);
    }

    /// Resolve a result id into a launch command. Only ids that a registered
    /// provider recognises can be launched; arbitrary commands are impossible.
    pub fn resolve(&self, id: &str) -> Result<LaunchSpec, String> {
        let (provider_id, _) = id
            .split_once(':')
            .ok_or_else(|| format!("malformed result id `{id}`"))?;

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
            icon: None,
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
