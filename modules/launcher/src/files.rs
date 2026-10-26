//! File and folder search over a background-built in-memory name index.
//!
//! Besides names, the index knows modification times and the desktop's "recently used" list,
//! so these work: `report`, `kind:pdf`, `kind:pdf invoice`, `kind:image`, `ext:rs`,
//! `kind:folder`, and `kind:recent` (your file history).
//!
//! Only paths that are in the index can be opened, so a result id can never be forged
//! into "open an arbitrary path".

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    recent,
};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, UNIX_EPOCH},
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
    /// Last modification, Unix seconds (0 when unknown).
    modified: u64,
    /// Last use according to the desktop's recent-files list, Unix seconds.
    recent: Option<u64>,
}

impl Entry {
    fn extension(&self) -> &str {
        if self.is_dir {
            return "";
        }

        self.name_lower.rsplit_once('.').map_or("", |(_, ext)| ext)
    }

    /// Newest of "modified" and "recently used": what "latest" means to a person.
    fn activity(&self) -> u64 {
        self.modified.max(self.recent.unwrap_or(0))
    }
}

/// What a `kind:` / `ext:` token asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Filter {
    Folder,
    Recent,
    Extensions(Vec<String>),
}

fn extensions(list: &[&str]) -> Filter {
    Filter::Extensions(list.iter().map(|ext| (*ext).to_owned()).collect())
}

fn kind_filter(name: &str) -> Filter {
    match name {
        "folder" | "folders" | "directory" | "dir" => Filter::Folder,
        "recent" | "history" => Filter::Recent,
        "pdf" => extensions(&["pdf"]),
        "image" | "images" | "photo" | "picture" => {
            extensions(&["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "heic"])
        }
        "document" | "documents" | "doc" => extensions(&[
            "pdf", "doc", "docx", "odt", "txt", "md", "rtf", "ppt", "pptx", "odp", "xls", "xlsx",
            "ods", "csv",
        ]),
        "audio" | "music" => extensions(&["mp3", "flac", "wav", "ogg", "m4a", "opus"]),
        "video" | "movie" => extensions(&["mp4", "mkv", "webm", "mov", "avi"]),
        "archive" | "zip" => extensions(&["zip", "tar", "gz", "xz", "7z", "rar", "bz2", "zst"]),
        "code" | "source" => extensions(&[
            "rs", "py", "js", "ts", "c", "h", "cpp", "go", "java", "sh", "toml", "json", "yaml",
            "yml", "html", "css",
        ]),
        other => Filter::Extensions(vec![other.trim_start_matches('.').to_owned()]),
    }
}

/// Split `kind:pdf invoice` into `("invoice", [pdf filter])`.
fn parse_query(query: &str) -> (String, Vec<Filter>) {
    let mut text = Vec::new();
    let mut filters = Vec::new();

    for token in query.split_whitespace() {
        let lower = token.to_lowercase();

        match lower
            .strip_prefix("kind:")
            .or_else(|| lower.strip_prefix("type:"))
            .or_else(|| lower.strip_prefix("ext:"))
        {
            Some(name) if !name.is_empty() => filters.push(kind_filter(name)),
            _ => text.push(lower),
        }
    }

    (text.join(" "), filters)
}

fn matches_filters(entry: &Entry, filters: &[Filter]) -> bool {
    let mut wants_recent = false;
    let mut kinds = Vec::new();

    for filter in filters {
        match filter {
            Filter::Recent => wants_recent = true,
            other => kinds.push(other),
        }
    }

    if wants_recent && entry.recent.is_none() {
        return false;
    }

    // Several kinds mean "any of these".
    kinds.is_empty()
        || kinds.iter().any(|kind| match kind {
            Filter::Folder => entry.is_dir,
            Filter::Extensions(list) => list.iter().any(|ext| ext == entry.extension()),
            Filter::Recent => true,
        })
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

fn make_entry(path: PathBuf, name: String, is_dir: bool, modified: u64) -> Entry {
    Entry {
        name_lower: name.to_lowercase(),
        name,
        path,
        is_dir,
        modified,
        recent: None,
    }
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

            let modified = item
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |elapsed| elapsed.as_secs());

            entries.push(make_entry(path, name, is_dir, modified));
        }
    }

    entries
}

/// Mark (or add) the files from the desktop's recent-files list.
fn merge_recent(entries: &mut Vec<Entry>) {
    let recents = recent::read_recent_files();

    if recents.is_empty() {
        return;
    }

    let positions: HashMap<PathBuf, usize> = entries
        .iter()
        .enumerate()
        .map(|(position, entry)| (entry.path.clone(), position))
        .collect();

    for (path, stamp) in recents {
        match positions.get(&path) {
            Some(&position) => entries[position].recent = Some(stamp),
            None => {
                // Outside the indexed roots (or in a hidden directory): still your history.
                let Some(name) = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                else {
                    continue;
                };

                let mut entry = make_entry(path, name, false, 0);
                entry.recent = Some(stamp);
                entries.push(entry);
            }
        }
    }
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
                    let mut entries = scan(&roots, max_depth);

                    merge_recent(&mut entries);

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
    fn from_entries(entries: Vec<Entry>) -> Self {
        Self {
            index: Arc::new(RwLock::new(entries)),
        }
    }
}

fn pretty_parent(path: &Path) -> String {
    let parent = path.parent().unwrap_or(path);

    // Compare whole path components: `/home/ann2` is not inside `/home/ann`.
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if !home.as_os_str().is_empty() => match parent.strip_prefix(&home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => parent.display().to_string(),
        },
        _ => parent.display().to_string(),
    }
}

impl Provider for FilesProvider {
    fn id(&self) -> &'static str {
        "files"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let (text, filters) = parse_query(query);

        // Plain searches need two characters (and never fire for `10 km to mi`);
        // filters like `kind:pdf` may stand alone.
        if filters.is_empty() && (text.chars().count() < 2 || text.contains(" to ")) {
            return Vec::new();
        }

        let Ok(index) = self.index.read() else {
            return Vec::new();
        };

        let mut hits: Vec<(u64, usize)> = index
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches_filters(entry, &filters))
            .filter(|(_, entry)| text.is_empty() || entry.name_lower.contains(&text))
            .map(|(position, entry)| {
                let rank = if text.is_empty() {
                    // Nothing typed: newest first.
                    entry.activity()
                } else {
                    let base: u64 = if entry.name_lower == text {
                        520
                    } else if entry.name_lower.starts_with(&text) {
                        480
                    } else {
                        380
                    };

                    // Shorter names, shallower paths and files you used recently win.
                    let brevity = 60u64.saturating_sub(entry.name.chars().count() as u64);
                    let depth = (entry.path.components().count() as u64).min(30);
                    let folder_bonus = if entry.is_dir { 10 } else { 0 };
                    let recent_bonus = if entry.recent.is_some() { 40 } else { 0 };

                    base + brevity + folder_bonus + recent_bonus - depth
                };

                (rank, position)
            })
            .collect();

        // A broad query can match tens of thousands of names; only the best few matter, so
        // partition instead of sorting everything.
        if hits.len() > MAX_RESULTS {
            hits.select_nth_unstable_by(MAX_RESULTS - 1, |a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            hits.truncate(MAX_RESULTS);
        }

        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

        let count = hits.len() as u32;

        hits.into_iter()
            .enumerate()
            .map(|(order, (_, position))| {
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
                    // Results are already ordered; the score only has to keep that order and
                    // stay below applications for plain searches.
                    score: 560 + (count - order as u32),
                    icon: None,
                    detail: None,
                }
            })
            .collect()
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let (reveal, path) = if let Some(path) = id.strip_prefix("files:open:") {
            (false, path)
        } else {
            (true, id.strip_prefix("files:reveal:")?)
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

    fn entry(path: &str, is_dir: bool, modified: u64, recent: Option<u64>) -> Entry {
        let path = PathBuf::from(path);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let mut entry = make_entry(path, name, is_dir, modified);
        entry.recent = recent;
        entry
    }

    fn provider() -> FilesProvider {
        FilesProvider::from_entries(vec![
            entry("/home/u/Documents/report.pdf", false, 100, None),
            entry("/home/u/Documents/reports", true, 50, None),
            entry("/home/u/Music/song.mp3", false, 10, None),
            entry("/home/u/Downloads/invoice.pdf", false, 300, Some(900)),
            entry("/home/u/Downloads/old.pdf", false, 5, None),
        ])
    }

    #[test]
    fn finds_by_substring() {
        let results = provider().search("report");

        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.kind == ResultKind::Folder));
    }

    #[test]
    fn short_plain_queries_return_nothing() {
        assert!(provider().search("r").is_empty());
    }

    #[test]
    fn kind_filter_lists_newest_first_without_text() {
        let results = provider().search("kind:pdf");

        let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).collect();

        assert_eq!(titles, ["invoice.pdf", "report.pdf", "old.pdf"]);
    }

    #[test]
    fn kind_filter_combines_with_text() {
        let results = provider().search("kind:pdf old");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "old.pdf");
    }

    #[test]
    fn recent_filter_is_file_history() {
        let results = provider().search("kind:recent");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "invoice.pdf");
    }

    #[test]
    fn folder_and_extension_filters() {
        assert_eq!(provider().search("kind:folder").len(), 1);
        assert_eq!(provider().search("ext:mp3").len(), 1);
    }

    #[test]
    fn open_and_reveal_resolve_only_indexed_paths() {
        let provider = provider();

        let open = provider
            .resolve("files:open:/home/u/Music/song.mp3")
            .unwrap()
            .unwrap();
        assert_eq!(open.args, ["/home/u/Music/song.mp3"]);

        let reveal = provider
            .resolve("files:reveal:/home/u/Music/song.mp3")
            .unwrap()
            .unwrap();
        assert_eq!(reveal.args, ["/home/u/Music"]);

        assert!(provider.resolve("files:open:/etc/passwd").is_none());
    }

    #[test]
    fn home_prefix_matches_whole_components_only() {
        // Not asserting on the real HOME: only that a sibling-looking directory is untouched.
        let shown = pretty_parent(Path::new("/definitely-not-home/x/file.txt"));

        assert_eq!(shown, "/definitely-not-home/x");
    }

    #[test]
    fn many_matches_still_return_the_best_first() {
        let entries = (0..500)
            .map(|index| entry(&format!("/home/u/d/note{index}.txt"), false, index, None))
            .chain([entry("/home/u/note", false, 1, None)])
            .collect();
        let results = FilesProvider::from_entries(entries).search("note");

        assert_eq!(results.len(), MAX_RESULTS);
        assert_eq!(results[0].title, "note");
        assert!(
            results
                .windows(2)
                .all(|pair| pair[0].score >= pair[1].score)
        );
    }

    #[test]
    fn expands_home() {
        assert!(expand_home("relative/path").is_none());
        assert_eq!(expand_home("/abs"), Some(PathBuf::from("/abs")));
    }
}
