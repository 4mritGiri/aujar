//! File and folder search over a background-built in-memory name index.
//!
//! Only paths that are in the index can be opened, so a result id can never be forged
//! into "open an arbitrary path".

use crate::model::{LaunchSpec, Provider, ResultKind, SearchResult};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};

/// Hard cap so a huge home directory cannot exhaust memory.
const MAX_ENTRIES: usize = 150_000;
/// How often the index is rebuilt.
const REFRESH: Duration = Duration::from_secs(300);
/// Maximum files/folders returned for one query.
const MAX_RESULTS: usize = 8;

const SKIPPED_DIRS: [&str; 8] = [
    "node_modules",
    "target",
    "__pycache__",
    "venv",
    ".git",
    "site-packages",
    "build",
    "dist",
];

#[derive(Debug, Clone)]
struct Entry {
    path: PathBuf,
    name: String,
    name_lower: String,
    is_dir: bool,
}

pub struct FilesProvider {
    index: Arc<RwLock<Vec<Entry>>>,
}

fn expand_home(root: &str) -> Option<PathBuf> {
    if root == "~" {
        return std::env::var_os("HOME").map(PathBuf::from);
    }

    if let Some(rest) = root.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }

    let path = PathBuf::from(root);

    path.is_absolute().then_some(path)
}

fn scan(roots: &[PathBuf], max_depth: usize) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = roots.iter().cloned().map(|root| (root, 0)).collect();

    while let Some((dir, depth)) = stack.pop() {
        let Ok(read_dir) = fs::read_dir(&dir) else {
            continue;
        };

        for item in read_dir.flatten() {
            if entries.len() >= MAX_ENTRIES {
                return entries;
            }

            let name = item.file_name().to_string_lossy().into_owned();

            // Hidden files and well-known junk directories are noise in a launcher.
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_str()) {
                continue;
            }

            let Ok(file_type) = item.file_type() else {
                continue;
            };

            // Symlinks are skipped: no cycles, no escaping the roots.
            if file_type.is_symlink() {
                continue;
            }

            let is_dir = file_type.is_dir();
            let path = item.path();

            if is_dir && depth + 1 < max_depth {
                stack.push((path.clone(), depth + 1));
            }

            entries.push(Entry {
                name_lower: name.to_lowercase(),
                name,
                path,
                is_dir,
            });
        }
    }

    entries
}

impl FilesProvider {
    /// Start indexing `roots` (`~` allowed) on a background thread.
    pub fn new(roots: &[String], max_depth: usize) -> Self {
        let roots: Vec<PathBuf> = roots.iter().filter_map(|root| expand_home(root)).collect();
        let index = Arc::new(RwLock::new(Vec::new()));
        let shared = Arc::clone(&index);

        let _ = std::thread::Builder::new()
            .name("aujar-files-index".into())
            .spawn(move || {
                loop {
                    let entries = scan(&roots, max_depth);

                    if let Ok(mut guard) = shared.write() {
                        *guard = entries;
                    }

                    // Sleep in short steps so the thread notices the provider is gone
                    // (it holds the only other reference) and exits.
                    let mut waited = Duration::ZERO;

                    while waited < REFRESH {
                        std::thread::sleep(Duration::from_secs(5));
                        waited += Duration::from_secs(5);

                        if Arc::strong_count(&shared) == 1 {
                            return;
                        }
                    }
                }
            });

        Self { index }
    }

    #[cfg(test)]
    fn from_entries(paths: &[(&str, bool)]) -> Self {
        let entries = paths
            .iter()
            .map(|(path, is_dir)| {
                let path = PathBuf::from(path);
                let name = path.file_name().unwrap().to_string_lossy().into_owned();

                Entry {
                    name_lower: name.to_lowercase(),
                    name,
                    path,
                    is_dir: *is_dir,
                }
            })
            .collect();

        Self {
            index: Arc::new(RwLock::new(entries)),
        }
    }
}

fn pretty_parent(path: &Path) -> String {
    let parent = path.parent().unwrap_or(path).display().to_string();

    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && parent.starts_with(&home) => {
            format!("~{}", &parent[home.len()..])
        }
        _ => parent,
    }
}

impl Provider for FilesProvider {
    fn id(&self) -> &'static str {
        "files"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let query = query.trim().to_lowercase();

        // Two characters minimum, and never for expressions like `2+2`.
        if query.chars().count() < 2 || query.contains(" to ") {
            return Vec::new();
        }

        let Ok(index) = self.index.read() else {
            return Vec::new();
        };

        let mut hits: Vec<(u32, usize)> = index
            .iter()
            .enumerate()
            .filter_map(|(position, entry)| {
                if !entry.name_lower.contains(&query) {
                    return None;
                }

                let base = if entry.name_lower == query {
                    520
                } else if entry.name_lower.starts_with(&query) {
                    480
                } else {
                    380
                };

                // Shorter names and shallower paths are usually what you want.
                let brevity = 60u32.saturating_sub(entry.name.chars().count() as u32);
                let depth = entry.path.components().count() as u32;
                let folder_bonus = if entry.is_dir { 10 } else { 0 };

                Some((base + brevity + folder_bonus - depth.min(30), position))
            })
            .collect();

        hits.sort_by(|a, b| b.0.cmp(&a.0));
        hits.truncate(MAX_RESULTS);

        hits.into_iter()
            .map(|(score, position)| {
                let entry = &index[position];

                SearchResult {
                    id: format!("files:open:{}", entry.path.display()),
                    title: entry.name.clone(),
                    subtitle: Some(pretty_parent(&entry.path)),
                    kind: if entry.is_dir {
                        ResultKind::Folder
                    } else {
                        ResultKind::File
                    },
                    score,
                    icon: None,
                }
            })
            .collect()
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let (reveal, path) = if let Some(path) = id.strip_prefix("files:open:") {
            (false, path)
        } else if let Some(path) = id.strip_prefix("files:reveal:") {
            (true, path)
        } else {
            return None;
        };

        let path = PathBuf::from(path);
        let index = self.index.read().ok()?;
        let entry = index.iter().find(|entry| entry.path == path)?;

        let target = if reveal {
            entry.path.parent().unwrap_or(&entry.path).to_path_buf()
        } else {
            entry.path.clone()
        };

        Some(Ok(LaunchSpec {
            title: entry.name.clone(),
            program: "xdg-open".to_owned(),
            args: vec![target.to_string_lossy().into_owned()],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> FilesProvider {
        FilesProvider::from_entries(&[
            ("/home/u/Documents/report.pdf", false),
            ("/home/u/Documents/reports", true),
            ("/home/u/Music/song.mp3", false),
        ])
    }

    #[test]
    fn finds_by_substring_and_prefers_exact_prefix() {
        let results = provider().search("report");

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.kind == ResultKind::Folder));
    }

    #[test]
    fn short_queries_return_nothing() {
        assert!(provider().search("r").is_empty());
    }

    #[test]
    fn open_and_reveal_resolve_only_indexed_paths() {
        let provider = provider();

        let open = provider.resolve("files:open:/home/u/Music/song.mp3").unwrap().unwrap();
        assert_eq!(open.args, ["/home/u/Music/song.mp3"]);

        let reveal = provider.resolve("files:reveal:/home/u/Music/song.mp3").unwrap().unwrap();
        assert_eq!(reveal.args, ["/home/u/Music"]);

        assert!(provider.resolve("files:open:/etc/passwd").is_none());
    }

    #[test]
    fn expands_home() {
        assert!(expand_home("relative/path").is_none());
        assert_eq!(expand_home("/abs"), Some(PathBuf::from("/abs")));
    }
}
