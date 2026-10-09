use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    ranking::{score, score_strict},
};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

/// Maximum directory nesting scanned below each applications directory.
const MAX_DEPTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub exec: String,
    pub terminal: bool,
}

/// Parse a freedesktop `.desktop` file. Returns `None` for entries that
/// should not be shown (not an Application, `NoDisplay`, `Hidden`, or
/// missing `Name`/`Exec`).
pub fn parse_desktop_entry(id: &str, content: &str) -> Option<DesktopEntry> {
    let mut in_entry = false;
    let mut name = None;
    let mut comment = None;
    let mut exec = None;
    let mut kind = None;
    let mut hidden = false;
    let mut terminal = false;

    for raw in content.lines() {
        let line = raw.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }

        if !in_entry {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let (key, value) = (key.trim(), value.trim());

        match key {
            "Name" => name = Some(value.to_owned()),
            "Comment" => comment = Some(value.to_owned()),
            "Exec" => exec = Some(value.to_owned()),
            "Type" => kind = Some(value.to_owned()),
            "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
            "NoDisplay" | "Hidden" if value.eq_ignore_ascii_case("true") => hidden = true,
            _ => {}
        }
    }

    if hidden || kind.as_deref() != Some("Application") {
        return None;
    }

    Some(DesktopEntry {
        id: id.to_owned(),
        name: name?,
        comment: comment.filter(|value: &String| !value.is_empty()),
        exec: exec?,
        terminal,
    })
}

/// Split a desktop-entry `Exec` value into arguments **without a shell**.
///
/// Handles double quotes and backslash escapes inside quotes, `%%`, and drops
/// field codes such as `%u`/`%F` (no files or URLs are passed).
pub fn parse_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut in_quotes = false;
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            '\\' if in_quotes => {
                match chars.peek().copied() {
                    Some(next) if matches!(next, '"' | '`' | '$' | '\\') => {
                        current.push(next);
                        chars.next();
                    }
                    _ => current.push('\\'),
                }

                has_token = true;
            }
            '%' => match chars.peek().copied() {
                Some('%') => {
                    chars.next();
                    current.push('%');
                    has_token = true;
                }
                Some(code) if code.is_ascii_alphabetic() => {
                    chars.next();
                }
                _ => {
                    current.push('%');
                    has_token = true;
                }
            },
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            _ => {
                current.push(c);
                has_token = true;
            }
        }
    }

    if has_token {
        args.push(current);
    }

    args
}

/// XDG application directories, highest priority first.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);

    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));

    if let Some(data_home) = data_home {
        dirs.push(data_home.join("applications"));
    }

    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());

    for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
        dirs.push(Path::new(dir).join("applications"));
    }

    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));

    if let Some(home) = home {
        dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
    }

    dirs
}

fn collect(
    dir: &Path,
    base: &Path,
    depth: usize,
    entries: &mut Vec<DesktopEntry>,
    seen: &mut HashSet<String>,
) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };

    for item in read_dir.flatten() {
        let path = item.path();

        if path.is_dir() {
            if depth < MAX_DEPTH {
                collect(&path, base, depth + 1, entries, seen);
            }

            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
            continue;
        }

        let Ok(relative) = path.strip_prefix(base) else {
            continue;
        };

        // Per the desktop-entry spec, subdirectories become `-` in the id.
        let id = relative.to_string_lossy().replace('/', "-");

        // First directory wins, even if it is hidden (user overrides).
        if !seen.insert(id.clone()) {
            continue;
        }

        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };

        if let Some(entry) = parse_desktop_entry(&id, &content) {
            entries.push(entry);
        }
    }
}

/// Searches installed desktop applications.
pub struct AppsProvider {
    entries: Vec<DesktopEntry>,
}

impl AppsProvider {
    pub fn from_dirs(dirs: &[PathBuf]) -> Self {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        for dir in dirs {
            collect(dir, dir, 0, &mut entries, &mut seen);
        }

        Self { entries }
    }

    pub fn from_env() -> Self {
        Self::from_dirs(&default_dirs())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Provider for AppsProvider {
    fn id(&self) -> &'static str {
        "apps"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        self.entries
            .iter()
            .filter_map(|entry| {
                let name_score = score(query, &entry.name);
                let comment_score = entry
                    .comment
                    .as_deref()
                    .and_then(|comment| score_strict(query, comment))
                    .map(|value| value / 2);

                let best = name_score.into_iter().chain(comment_score).max()?;

                Some(SearchResult {
                    id: format!("apps:{}", entry.id),
                    title: entry.name.clone(),
                    subtitle: entry.comment.clone(),
                    kind: ResultKind::Application,
                    score: best,
                })
            })
            .collect()
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let key = id.strip_prefix("apps:")?;
        let entry = self.entries.iter().find(|entry| entry.id == key)?;

        if entry.terminal {
            return Some(Err(format!(
                "`{}` needs a terminal, which is not supported yet",
                entry.name
            )));
        }

        let mut parts = parse_exec(&entry.exec).into_iter();

        let Some(program) = parts.next() else {
            return Some(Err(format!("`{}` has an empty Exec line", entry.name)));
        };

        Some(Ok(LaunchSpec {
            title: entry.name.clone(),
            program,
            args: parts.collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "\
[Desktop Entry]
Type=Application
Name=Firefox
Name[ne]=फायरफक्स
Comment=Browse the web
Exec=firefox %u

[Desktop Action new-window]
Name=New Window
Exec=firefox --new-window
";

    #[test]
    fn parses_visible_application() {
        let entry = parse_desktop_entry("firefox.desktop", FIREFOX).unwrap();

        assert_eq!(entry.name, "Firefox");
        assert_eq!(entry.comment.as_deref(), Some("Browse the web"));
        assert_eq!(entry.exec, "firefox %u");
    }

    #[test]
    fn skips_hidden_and_non_applications() {
        let hidden = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n";
        let link = "[Desktop Entry]\nType=Link\nName=X\nExec=x\n";
        let no_exec = "[Desktop Entry]\nType=Application\nName=X\n";

        assert!(parse_desktop_entry("a", hidden).is_none());
        assert!(parse_desktop_entry("b", link).is_none());
        assert!(parse_desktop_entry("c", no_exec).is_none());
    }

    #[test]
    fn scans_directory_and_ranks() {
        let dir = std::env::temp_dir().join(format!(
            "aujar-apps-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("firefox.desktop"), FIREFOX).unwrap();
        fs::write(
            dir.join("files.desktop"),
            "[Desktop Entry]\nType=Application\nName=Files\nExec=files\n",
        )
        .unwrap();

        let provider = AppsProvider::from_dirs(std::slice::from_ref(&dir));
        assert_eq!(provider.len(), 2);

        let results = provider.search("fire");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "apps:firefox.desktop");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn exec_parsing_has_no_shell_semantics() {
        assert_eq!(parse_exec("firefox %u"), ["firefox"]);
        assert_eq!(
            parse_exec(r#"env FOO=1 "my app" --file=%f"#),
            ["env", "FOO=1", "my app", "--file="]
        );
        assert_eq!(
            parse_exec(r#"sh -c "echo \"hi\"""#),
            ["sh", "-c", r#"echo "hi""#]
        );
        assert_eq!(parse_exec("app 100%%"), ["app", "100%"]);
        // Shell metacharacters are plain text, never interpreted.
        assert_eq!(parse_exec("app ; rm -rf ~"), ["app", ";", "rm", "-rf", "~"]);
    }

    #[test]
    fn resolves_only_indexed_applications() {
        let dir = std::env::temp_dir().join(format!(
            "aujar-resolve-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("firefox.desktop"), FIREFOX).unwrap();
        fs::write(
            dir.join("htop.desktop"),
            "[Desktop Entry]\nType=Application\nName=Htop\nExec=htop\nTerminal=true\n",
        )
        .unwrap();

        let provider = AppsProvider::from_dirs(std::slice::from_ref(&dir));

        let spec = provider.resolve("apps:firefox.desktop").unwrap().unwrap();
        assert_eq!(spec.program, "firefox");
        assert!(spec.args.is_empty());

        assert!(provider.resolve("apps:htop.desktop").unwrap().is_err());
        assert!(provider.resolve("apps:missing.desktop").is_none());
        assert!(provider.resolve("calc:4").is_none());

        fs::remove_dir_all(dir).unwrap();
    }
}
