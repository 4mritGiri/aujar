//! Programs on `PATH` (`htop`, `python3`, `btop`): run one in a terminal.
//!
//! Only programs found in the index can be run, and only without arguments, so a result id can
//! never be forged into an arbitrary command line (that is what the opt-in `>` shell is for).
//! Programs that have a desktop entry are left to the applications provider.

use crate::{
    apps::in_terminal,
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::RwLock,
    time::{Duration, Instant},
};

/// How long an index is trusted before `PATH` is scanned again.
const RESCAN: Duration = Duration::from_secs(60);
/// At most this many commands per query: they are a supplement, not the main list.
const MAX_RESULTS: usize = 3;
/// Shorter queries match too many programs to be useful.
const MIN_QUERY: usize = 2;
/// Hard cap so a strange `PATH` cannot exhaust memory.
const MAX_ENTRIES: usize = 20_000;

/// A command typed in full is very likely what was meant.
const EXACT_SCORE: u32 = 900;
/// Prefix matches stay well below applications with the same prefix (800+).
const PREFIX_BASE: u32 = 300;

/// Runs the program, then keeps the terminal open until Enter so output stays readable.
/// The program path arrives as `$0`; nothing is spliced into the script.
const HOLD_SCRIPT: &str = r#""$0"; status=$?; printf '\n[%s exited with status %d. Press Enter to close.]' "${0##*/}" "$status"; read -r _"#;

#[derive(Debug, Clone)]
struct Command {
    name: String,
    path: PathBuf,
}

struct Index {
    /// Sorted by name.
    commands: Vec<Command>,
    built: Instant,
}

pub struct CommandsProvider {
    /// Overrides `PATH` (tests).
    dirs: Option<Vec<PathBuf>>,
    /// Program names that already have a desktop entry.
    hidden: HashSet<String>,
    index: RwLock<Index>,
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default()
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    // `metadata` follows symlinks: `/usr/bin/python3 -> python3.12` counts.
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

/// Names a person would type: no hidden files, no editor backups, nothing with spaces.
fn plausible_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.ends_with('~')
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
}

fn scan(dirs: &[PathBuf], hidden: &HashSet<String>) -> Vec<Command> {
    let mut found: HashMap<String, PathBuf> = HashMap::new();

    for dir in dirs {
        // Relative entries (`.`, empty) would make the result depend on the daemon's cwd.
        if !dir.is_absolute() {
            continue;
        }

        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };

        for entry in entries.flatten() {
            if found.len() >= MAX_ENTRIES {
                break;
            }

            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };

            // The first directory on PATH wins, as in a shell.
            if found.contains_key(&name) || !plausible_name(&name) || hidden.contains(&name) {
                continue;
            }

            let path = entry.path();

            if is_executable(&path) {
                found.insert(name, path);
            }
        }
    }

    let mut commands: Vec<Command> = found
        .into_iter()
        .map(|(name, path)| Command { name, path })
        .collect();

    commands.sort_by(|a, b| a.name.cmp(&b.name));

    commands
}

impl CommandsProvider {
    /// Index `PATH`, leaving out `hidden` program names (those with a desktop entry).
    pub fn new(hidden: HashSet<String>) -> Self {
        Self::build(None, hidden)
    }

    /// Index `dirs` instead of `PATH`.
    pub fn from_dirs(dirs: Vec<PathBuf>, hidden: HashSet<String>) -> Self {
        Self::build(Some(dirs), hidden)
    }

    fn build(dirs: Option<Vec<PathBuf>>, hidden: HashSet<String>) -> Self {
        let commands = scan(dirs.as_deref().unwrap_or(&path_dirs()), &hidden);

        Self {
            dirs,
            hidden,
            index: RwLock::new(Index {
                commands,
                built: Instant::now(),
            }),
        }
    }

    fn rescan_if_stale(&self) {
        let stale = self
            .index
            .read()
            .map(|index| index.built.elapsed() >= RESCAN)
            .unwrap_or(true);

        if !stale {
            return;
        }

        let commands = scan(self.dirs.as_deref().unwrap_or(&path_dirs()), &self.hidden);
        let mut index = self
            .index
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        *index = Index {
            commands,
            built: Instant::now(),
        };
    }

    fn lookup(&self, name: &str) -> Option<Command> {
        let index = self
            .index
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        index
            .commands
            .binary_search_by(|command| command.name.as_str().cmp(name))
            .ok()
            .map(|position| index.commands[position].clone())
    }
}

impl Provider for CommandsProvider {
    fn id(&self) -> &'static str {
        "commands"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let query = query.trim();

        // A command takes no arguments here; `htop -d 5` belongs to the `>` shell.
        if query.chars().count() < MIN_QUERY || query.contains(char::is_whitespace) {
            return Vec::new();
        }

        self.rescan_if_stale();

        let lowered = query.to_lowercase();
        let index = self
            .index
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut ranked: Vec<(u32, &Command)> = index
            .commands
            .iter()
            .filter_map(|command| {
                let name = command.name.to_lowercase();

                if name == lowered {
                    Some((EXACT_SCORE, command))
                } else if name.starts_with(&lowered) {
                    // Shorter names rank first, as in `ranking::score`.
                    let bonus = 99u32.saturating_sub(name.chars().count() as u32);
                    Some((PREFIX_BASE + bonus, command))
                } else {
                    None
                }
            })
            .collect();

        ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        ranked.truncate(MAX_RESULTS);

        ranked
            .into_iter()
            .map(|(score, command)| {
                SearchResult::new(
                    format!("commands:{}", command.name),
                    &command.name,
                    ResultKind::Command,
                    score,
                )
                .with_subtitle(format!("Run in a terminal · {}", command.path.display()))
            })
            .collect()
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let name = id.strip_prefix("commands:")?;

        let Some(command) = self.lookup(name) else {
            return Some(Err(format!(
                "`{name}` is no longer on PATH (search again to refresh)"
            )));
        };

        let path = command.path.to_string_lossy().into_owned();
        let held = vec!["-c".to_owned(), HOLD_SCRIPT.to_owned(), path];

        Some(
            in_terminal("sh".to_owned(), held)
                .map(|(program, args)| LaunchSpec {
                    title: command.name.clone(),
                    program,
                    args,
                })
                .ok_or_else(|| {
                    "no terminal emulator found; install one or set $TERMINAL".to_owned()
                }),
        )
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn program(dir: &Path, name: &str, mode: u32) {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn provider(hidden: &[&str]) -> (tempfile::TempDir, tempfile::TempDir, CommandsProvider) {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();

        program(first.path(), "htop", 0o755);
        program(first.path(), "htop-helper", 0o755);
        program(first.path(), "notes.txt", 0o644);
        program(first.path(), ".hidden", 0o755);
        program(first.path(), "firefox", 0o755);
        program(second.path(), "htop", 0o755);
        program(second.path(), "hx", 0o755);
        std::fs::create_dir(first.path().join("hdir")).unwrap();

        let provider = CommandsProvider::from_dirs(
            vec![
                first.path().to_owned(),
                PathBuf::from("relative/bin"),
                second.path().to_owned(),
            ],
            hidden.iter().map(|name| (*name).to_owned()).collect(),
        );

        (first, second, provider)
    }

    fn titles(results: &[SearchResult]) -> Vec<&str> {
        results.iter().map(|r| r.title.as_str()).collect()
    }

    #[test]
    fn indexes_only_executable_plausible_files() {
        let (_first, _second, provider) = provider(&[]);
        let index = provider.index.read().unwrap();
        let names: Vec<_> = index.commands.iter().map(|c| c.name.as_str()).collect();

        assert_eq!(names, ["firefox", "htop", "htop-helper", "hx"]);
    }

    #[test]
    fn first_path_directory_wins() {
        let (first, _second, provider) = provider(&[]);

        assert_eq!(
            provider.lookup("htop").unwrap().path,
            first.path().join("htop")
        );
    }

    #[test]
    fn exact_ranks_above_prefix_and_both_need_two_letters() {
        let (_first, _second, provider) = provider(&[]);

        let results = provider.search("htop");
        assert_eq!(titles(&results), ["htop", "htop-helper"]);
        assert_eq!(results[0].score, EXACT_SCORE);
        assert!(results[1].score < 800, "prefix must rank below apps");

        assert_eq!(titles(&provider.search("hx")), ["hx"]);
        assert!(provider.search("h").is_empty());
    }

    #[test]
    fn no_substring_matches_or_arguments() {
        let (_first, _second, provider) = provider(&[]);

        assert!(provider.search("top").is_empty());
        assert!(provider.search("htop -d 5").is_empty());
    }

    #[test]
    fn programs_with_a_desktop_entry_are_hidden() {
        let (_first, _second, provider) = provider(&["firefox"]);

        assert!(provider.search("firefox").is_empty());
        assert!(matches!(provider.resolve("commands:firefox"), Some(Err(_))));
    }

    #[test]
    fn only_indexed_commands_resolve() {
        let (_first, _second, provider) = provider(&[]);

        assert!(provider.resolve("apps:htop").is_none());

        let Some(Err(message)) = provider.resolve("commands:rm -rf ~") else {
            panic!("an unindexed command resolved");
        };
        assert!(message.contains("no longer on PATH"), "{message}");
    }

    #[test]
    fn resolves_to_a_held_terminal_without_splicing_the_path() {
        let (first, _second, provider) = provider(&[]);

        // A machine without any terminal emulator cannot run it; say so instead.
        let Some(result) = provider.resolve("commands:htop") else {
            panic!("not owned");
        };

        match result {
            Ok(spec) => {
                let path = first.path().join("htop").to_string_lossy().into_owned();

                assert_eq!(spec.title, "htop");
                assert_eq!(spec.args.last(), Some(&path));
                assert!(spec.args.contains(&HOLD_SCRIPT.to_owned()));
                assert!(!HOLD_SCRIPT.contains(&path));
            }
            Err(message) => assert!(message.contains("terminal"), "{message}"),
        }
    }
}
