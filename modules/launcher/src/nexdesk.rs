//! NexDesk saved connections: `rdp office`, `nexdesk office`, or just `rdp` for the list.
//!
//! NexDesk keeps each saved connection as a plain `.rdp` file in `<config>/nexdesk/connections/`
//! and never stores passwords there. This provider only reads those file names (and the address
//! and user inside them, for the subtitle); it never touches NexDesk's password vault. Enter runs
//! `nexdesk --connect <name>`, so NexDesk itself asks for the password.
//!
//! Nothing is shown unless the query starts with one of the trigger words.

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    ranking::score,
};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const TRIGGERS: [&str; 2] = ["rdp", "nexdesk"];
const ID_PREFIX: &str = "nexdesk:connect:";
const MAX_RESULTS: usize = 8;
/// More saved connections than this are ignored; nobody types a filter over thousands.
const MAX_CONNECTIONS: usize = 500;
/// How much of a `.rdp` file is read for the subtitle.
const MAX_FILE_BYTES: u64 = 16 * 1024;
/// A typed trigger outranks ordinary matches (applications top out around 1100).
const BASE_SCORE: u32 = 1200;

pub struct NexDeskProvider {
    dir: Option<PathBuf>,
}

impl NexDeskProvider {
    pub fn new() -> Self {
        Self {
            dir: connections_dir(),
        }
    }

    /// Read connections from `dir` instead of the user's NexDesk folder (tests).
    pub fn with_dir(dir: PathBuf) -> Self {
        Self { dir: Some(dir) }
    }

    /// Names of the saved connections (file names without `.rdp`), sorted case-insensitively.
    fn names(&self) -> Vec<String> {
        let Some(dir) = &self.dir else {
            return Vec::new();
        };
        let Ok(entries) = fs::read_dir(dir) else {
            return Vec::new();
        };

        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("rdp"))
            .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
            .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
            .take(MAX_CONNECTIONS)
            .collect();

        names.sort_by_key(|name| name.to_lowercase());
        names
    }

    fn subtitle(&self, name: &str) -> String {
        let described = self
            .dir
            .as_ref()
            .and_then(|dir| describe(&dir.join(format!("{name}.rdp"))));

        match described {
            Some(text) => format!("NexDesk \u{b7} {text}"),
            None => "NexDesk connection".to_owned(),
        }
    }
}

impl Default for NexDeskProvider {
    fn default() -> Self {
        Self::new()
    }
}

/// `<config>/nexdesk/connections`, where NexDesk saves connections on Linux (it honours
/// `XDG_CONFIG_HOME` the same way).
fn connections_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;

    Some(base.join("nexdesk").join("connections"))
}

/// The filter after a trigger word: `rdp office` gives `office`, `rdp` gives an empty filter,
/// anything else gives `None`.
fn filter_after_trigger(query: &str) -> Option<&str> {
    let query = query.trim();

    TRIGGERS.iter().find_map(|trigger| {
        let rest = query.get(trigger.len()..)?;

        if !query[..trigger.len()].eq_ignore_ascii_case(trigger) {
            return None;
        }

        if rest.is_empty() {
            Some("")
        } else if rest.starts_with(char::is_whitespace) {
            Some(rest.trim())
        } else {
            None
        }
    })
}

/// `address · user` from a `.rdp` file, for the result's subtitle.
fn describe(path: &Path) -> Option<String> {
    let mut text = String::new();

    fs::File::open(path)
        .ok()?
        .take(MAX_FILE_BYTES)
        .read_to_string(&mut text)
        .ok()?;

    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key))
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
            .map(str::to_owned)
    };

    match (field("full address:s:"), field("username:s:")) {
        (Some(address), Some(user)) => Some(format!("{address} \u{b7} {user}")),
        (Some(address), None) => Some(address),
        _ => None,
    }
}

impl Provider for NexDeskProvider {
    fn id(&self) -> &'static str {
        "nexdesk"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some(filter) = filter_after_trigger(query) else {
            return Vec::new();
        };

        let mut matches: Vec<(u32, String)> = self
            .names()
            .into_iter()
            .filter_map(|name| {
                if filter.is_empty() {
                    Some((0, name))
                } else {
                    score(filter, &name).map(|points| (points, name))
                }
            })
            .collect();

        // Best match first; the list is already alphabetical for ties and for an empty filter.
        matches.sort_by_key(|(points, _)| std::cmp::Reverse(*points));
        matches.truncate(MAX_RESULTS);

        matches
            .into_iter()
            .map(|(points, name)| {
                SearchResult::new(
                    format!("{ID_PREFIX}{name}"),
                    name.clone(),
                    ResultKind::Application,
                    BASE_SCORE + points,
                )
                .with_subtitle(self.subtitle(&name))
            })
            .collect()
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let name = id.strip_prefix(ID_PREFIX)?;

        // Only a connection that really is saved can be started: the id is never passed on
        // unchecked.
        if !self.names().iter().any(|known| known == name) {
            return Some(Err(format!("no saved NexDesk connection named `{name}`")));
        }

        let Some(program) = aujar_nexdesk::locate::locate() else {
            return Some(Err(
                "NexDesk is not installed (see docs/NEXDESK.md, or `aujar nexdesk status`)"
                    .to_owned(),
            ));
        };

        let Some(program) = program.to_str() else {
            return Some(Err("the NexDesk path is not valid UTF-8".to_owned()));
        };

        Some(Ok(LaunchSpec {
            title: format!("NexDesk: {name}"),
            program: program.to_owned(),
            args: vec!["--connect".to_owned(), name.to_owned()],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Removes its directory when the test ends.
    struct Cleanup(PathBuf);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A fresh temporary directory holding the given `(file name, content)` pairs.
    fn connections(files: &[(&str, &str)]) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "aujar-nexdesk-test-{}-{nanos}-{}",
            std::process::id(),
            files.len()
        ));

        fs::create_dir_all(&dir).unwrap();

        for (name, content) in files {
            fs::write(dir.join(name), content).unwrap();
        }

        dir
    }

    fn provider() -> (NexDeskProvider, Cleanup) {
        let dir = connections(&[
            (
                "Office PC.rdp",
                "full address:s:10.0.0.5:3389\r\nusername:s:alice\r\n",
            ),
            ("Build Server.rdp", "full address:s:build.example.com\n"),
            ("notes.txt", "not a connection"),
            ("Home.rdp", ""),
        ]);

        (NexDeskProvider::with_dir(dir.clone()), Cleanup(dir))
    }

    #[test]
    fn only_the_trigger_words_start_a_search() {
        assert_eq!(filter_after_trigger("rdp"), Some(""));
        assert_eq!(filter_after_trigger("  RDP  "), Some(""));
        assert_eq!(filter_after_trigger("rdp office"), Some("office"));
        assert_eq!(filter_after_trigger("nexdesk  build"), Some("build"));
        assert_eq!(filter_after_trigger("NexDesk home"), Some("home"));

        assert_eq!(filter_after_trigger("rdpx"), None);
        assert_eq!(filter_after_trigger("office"), None);
        assert_eq!(filter_after_trigger("firefox"), None);
        assert_eq!(filter_after_trigger(""), None);
        assert_eq!(filter_after_trigger("é"), None);
    }

    #[test]
    fn nothing_is_shown_without_a_trigger() {
        let (provider, _dir) = provider();

        assert!(provider.search("office").is_empty());
        assert!(provider.search("rdpoffice").is_empty());
    }

    #[test]
    fn a_bare_trigger_lists_connections_alphabetically() {
        let (provider, _dir) = provider();
        let titles: Vec<String> = provider
            .search("rdp")
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(titles, ["Build Server", "Home", "Office PC"]);
    }

    #[test]
    fn a_filter_narrows_and_ranks_the_list() {
        let (provider, _dir) = provider();
        let results = provider.search("rdp off");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Office PC");
        assert_eq!(results[0].id, "nexdesk:connect:Office PC");
        assert_eq!(results[0].kind, ResultKind::Application);
        assert_eq!(
            results[0].subtitle.as_deref(),
            Some("NexDesk \u{b7} 10.0.0.5:3389 \u{b7} alice")
        );

        assert!(provider.search("rdp zzzz").is_empty());
    }

    #[test]
    fn the_subtitle_copes_with_missing_fields() {
        let (provider, _dir) = provider();
        let results = provider.search("rdp");

        let by_title = |title: &str| {
            results
                .iter()
                .find(|result| result.title == title)
                .unwrap()
                .subtitle
                .clone()
        };

        assert_eq!(
            by_title("Build Server").as_deref(),
            Some("NexDesk \u{b7} build.example.com")
        );
        assert_eq!(by_title("Home").as_deref(), Some("NexDesk connection"));
    }

    #[test]
    fn a_missing_folder_gives_no_results() {
        let provider = NexDeskProvider::with_dir(PathBuf::from("/definitely/not/here"));

        assert!(provider.search("rdp").is_empty());
    }

    #[test]
    fn ids_of_other_providers_are_not_ours() {
        let (provider, _dir) = provider();

        assert!(provider.resolve("apps:firefox.desktop").is_none());
        assert!(provider.resolve("nexdesk:other").is_none());
    }

    #[test]
    fn an_unknown_connection_cannot_be_started() {
        let (provider, _dir) = provider();

        let Some(Err(message)) = provider.resolve("nexdesk:connect:Not Saved") else {
            panic!("expected an error");
        };

        assert!(message.contains("no saved NexDesk connection"), "{message}");
    }

    #[test]
    fn a_name_cannot_smuggle_a_path_or_an_option() {
        let (provider, _dir) = provider();

        for id in [
            "nexdesk:connect:../../etc/passwd",
            "nexdesk:connect:--help",
            "nexdesk:connect:Office PC --extra",
        ] {
            let Some(Err(message)) = provider.resolve(id) else {
                panic!("{id} must be refused");
            };

            assert!(message.contains("no saved NexDesk connection"), "{message}");
        }
    }
}
