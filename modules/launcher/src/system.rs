//! System commands: lock, sleep, restart, shut down, log out, system settings.

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    ranking::score,
};

struct Action {
    id: &'static str,
    title: &'static str,
    subtitle: &'static str,
    keywords: &'static [&'static str],
    program: &'static str,
    args: Vec<String>,
}

pub struct SystemProvider {
    actions: Vec<Action>,
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

impl SystemProvider {
    pub fn new() -> Self {
        let mut actions = vec![
            Action {
                id: "system:lock",
                title: "Lock Screen",
                subtitle: "Lock this session",
                keywords: &["lock", "lock screen"],
                program: "loginctl",
                args: vec!["lock-session".into()],
            },
            Action {
                id: "system:sleep",
                title: "Sleep",
                subtitle: "Suspend to RAM",
                keywords: &["sleep", "suspend"],
                program: "systemctl",
                args: vec!["suspend".into()],
            },
            Action {
                id: "system:restart",
                title: "Restart",
                subtitle: "Reboot the computer",
                keywords: &["restart", "reboot"],
                program: "systemctl",
                args: vec!["reboot".into()],
            },
            Action {
                id: "system:shutdown",
                title: "Shut Down",
                subtitle: "Power off the computer",
                keywords: &["shut down", "shutdown", "power off", "poweroff"],
                program: "systemctl",
                args: vec!["poweroff".into()],
            },
        ];

        // Ends the whole login session without a prompt, on any systemd desktop.
        if let Ok(session) = std::env::var("XDG_SESSION_ID") {
            if !session.is_empty() && session.chars().all(|c| c.is_ascii_alphanumeric()) {
                actions.push(Action {
                    id: "system:logout",
                    title: "Log Out",
                    subtitle: "End this session",
                    keywords: &["log out", "logout", "sign out"],
                    program: "loginctl",
                    args: vec!["terminate-session".into(), session],
                });
            }
        }

        for (program, title) in [
            ("gnome-control-center", "System Settings"),
            ("systemsettings", "System Settings"),
        ] {
            if on_path(program) {
                actions.push(Action {
                    id: "system:settings",
                    title,
                    subtitle: "Open the desktop settings",
                    keywords: &["settings", "preferences", "control center"],
                    program,
                    args: Vec::new(),
                });
                break;
            }
        }

        Self { actions }
    }
}

impl Default for SystemProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for SystemProvider {
    fn id(&self) -> &'static str {
        "system"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        if query.trim().chars().count() < 2 {
            return Vec::new();
        }

        self.actions
            .iter()
            .filter_map(|action| {
                let best = std::iter::once(action.title)
                    .chain(action.keywords.iter().copied())
                    .filter_map(|text| score(query, text))
                    .max()?;

                // Below applications: a stray "sl" must not outrank the app you want.
                Some(SearchResult {
                    id: action.id.to_owned(),
                    title: action.title.to_owned(),
                    subtitle: Some(action.subtitle.to_owned()),
                    kind: ResultKind::System,
                    score: best.saturating_sub(250),
                    icon: None,
                })
            })
            .collect()
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let action = self.actions.iter().find(|action| action.id == id)?;

        Some(Ok(LaunchSpec {
            title: action.title.to_owned(),
            program: action.program.to_owned(),
            args: action.args.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_keyword() {
        let results = SystemProvider::new().search("reboot");

        assert!(results.iter().any(|result| result.id == "system:restart"));
    }

    #[test]
    fn ignores_one_character_queries() {
        assert!(SystemProvider::new().search("s").is_empty());
    }

    #[test]
    fn resolves_known_ids_only() {
        let provider = SystemProvider::new();

        assert!(provider.resolve("system:lock").is_some());
        assert!(provider.resolve("system:rm-rf").is_none());
    }
}
