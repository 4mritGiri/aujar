//! Launch history: powers "Recent" and "Frequently used" in the empty launcher and nudges
//! ranking toward what you actually open. Stored in `~/.local/state/aujar/history.toml`
//! (readable only by you).

use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

/// Entries kept; the least recently used are dropped first.
const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub count: u32,
    /// Unix seconds.
    pub last_used: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub entries: Vec<HistoryEntry>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl History {
    /// `$XDG_STATE_HOME/aujar/history.toml`, else `~/.local/state/aujar/history.toml`.
    pub fn path() -> PathBuf {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"));

        base.join("aujar").join("history.toml")
    }

    /// Never fails: a missing or broken file is an empty history.
    pub fn load_or_default() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|content| toml::from_str(&content).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let temp = path.with_extension("toml.tmp");
        fs::write(&temp, toml::to_string_pretty(self)?)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
        }

        fs::rename(&temp, &path)?;

        Ok(())
    }

    /// Remember that `id` was just launched.
    pub fn record(
        &mut self,
        id: &str,
        title: &str,
        subtitle: Option<&str>,
        kind: &str,
        icon: Option<&str>,
    ) {
        let stamp = now();

        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.count = entry.count.saturating_add(1);
            entry.last_used = stamp;
            entry.title = title.to_owned();
            entry.subtitle = subtitle.map(str::to_owned);
            entry.icon = icon.map(str::to_owned);
        } else {
            self.entries.push(HistoryEntry {
                id: id.to_owned(),
                title: title.to_owned(),
                subtitle: subtitle.map(str::to_owned),
                kind: kind.to_owned(),
                icon: icon.map(str::to_owned),
                count: 1,
                last_used: stamp,
            });
        }

        if self.entries.len() > MAX_ENTRIES {
            self.entries.sort_by(|a, b| b.last_used.cmp(&a.last_used));
            self.entries.truncate(MAX_ENTRIES);
        }
    }

    pub fn count_of(&self, id: &str) -> u32 {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or(0, |entry| entry.count)
    }

    /// Most recently used first.
    pub fn recent(&self, limit: usize) -> Vec<&HistoryEntry> {
        let mut entries: Vec<&HistoryEntry> = self.entries.iter().collect();

        entries.sort_by(|a, b| b.last_used.cmp(&a.last_used));
        entries.truncate(limit);

        entries
    }

    /// Most used first, only things used at least twice, skipping ids in `skip`.
    pub fn frequent(&self, limit: usize, skip: &[&str]) -> Vec<&HistoryEntry> {
        let mut entries: Vec<&HistoryEntry> = self
            .entries
            .iter()
            .filter(|entry| entry.count >= 2 && !skip.contains(&entry.id.as_str()))
            .collect();

        entries.sort_by(|a, b| b.count.cmp(&a.count).then(b.last_used.cmp(&a.last_used)));
        entries.truncate(limit);

        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_counts_repeats() {
        let mut history = History::default();

        history.record("apps:a", "A", None, "application", None);
        history.record("apps:a", "A", None, "application", None);
        history.record("apps:b", "B", Some("b"), "application", None);

        assert_eq!(history.count_of("apps:a"), 2);
        assert_eq!(history.count_of("apps:b"), 1);
        assert_eq!(history.count_of("apps:c"), 0);
    }

    #[test]
    fn frequent_needs_two_uses_and_honors_skip() {
        let mut history = History::default();

        for _ in 0..3 {
            history.record("apps:a", "A", None, "application", None);
        }
        history.record("apps:b", "B", None, "application", None);

        assert_eq!(history.frequent(5, &[]).len(), 1);
        assert!(history.frequent(5, &["apps:a"]).is_empty());
    }

    #[test]
    fn prunes_to_the_limit() {
        let mut history = History::default();

        for index in 0..(MAX_ENTRIES + 20) {
            history.record(&format!("apps:{index}"), "x", None, "application", None);
        }

        assert_eq!(history.entries.len(), MAX_ENTRIES);
    }

    #[test]
    fn round_trips_through_toml() {
        let mut history = History::default();
        history.record("files:open:/a b", "a b", Some("~"), "file", None);

        let text = toml::to_string_pretty(&history).unwrap();

        assert_eq!(toml::from_str::<History>(&text).unwrap(), history);
    }
}
