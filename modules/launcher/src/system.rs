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

fn action(
    id: &'static str,
    title: &'static str,
    subtitle: &'static str,
    keywords: &'static [&'static str],
    program: &'static str,
    args: &[&str],
) -> Action {
    Action {
        id,
        title,
        subtitle,
        keywords,
        program,
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
    }
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
        let session = std::env::var("XDG_SESSION_ID")
            .ok()
            .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()));

        if let Some(session) = session {
            actions.push(Action {
                id: "system:logout",
                title: "Log Out",
                subtitle: "End this session",
                keywords: &["log out", "logout", "sign out"],
                program: "loginctl",
                args: vec!["terminate-session".into(), session],
            });
        }

        if on_path("nmcli") {
            actions.push(action(
                "system:wifi-on",
                "Turn Wi-Fi On",
                "Enable the wireless radio",
                &["wifi on", "wi-fi on", "enable wifi", "turn on wifi"],
                "nmcli",
                &["radio", "wifi", "on"],
            ));
            actions.push(action(
                "system:wifi-off",
                "Turn Wi-Fi Off",
                "Disable the wireless radio",
                &["wifi off", "wi-fi off", "disable wifi", "turn off wifi"],
                "nmcli",
                &["radio", "wifi", "off"],
            ));
        }

        if on_path("bluetoothctl") {
            actions.push(action(
                "system:bluetooth-on",
                "Turn Bluetooth On",
                "Power on the Bluetooth adapter",
                &["bluetooth on", "enable bluetooth", "turn on bluetooth"],
                "bluetoothctl",
                &["power", "on"],
            ));
            actions.push(action(
                "system:bluetooth-off",
                "Turn Bluetooth Off",
                "Power off the Bluetooth adapter",
                &["bluetooth off", "disable bluetooth", "turn off bluetooth"],
                "bluetoothctl",
                &["power", "off"],
            ));
        }

        // PipeWire first, then PulseAudio.
        if on_path("wpctl") {
            actions.push(action(
                "system:mute",
                "Mute / Unmute Volume",
                "Toggle the default output",
                &["mute", "unmute", "toggle mute", "volume mute"],
                "wpctl",
                &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"],
            ));
        } else if on_path("pactl") {
            actions.push(action(
                "system:mute",
                "Mute / Unmute Volume",
                "Toggle the default output",
                &["mute", "unmute", "toggle mute", "volume mute"],
                "pactl",
                &["set-sink-mute", "@DEFAULT_SINK@", "toggle"],
            ));
        }

        // GNOME appearance (a harmless no-op elsewhere: the schema is missing).
        if on_path("gsettings") {
            actions.push(action(
                "system:dark-mode",
                "Dark Mode",
                "Switch the desktop to the dark style",
                &["dark mode", "dark theme", "night mode"],
                "gsettings",
                &[
                    "set",
                    "org.gnome.desktop.interface",
                    "color-scheme",
                    "prefer-dark",
                ],
            ));
            actions.push(action(
                "system:light-mode",
                "Light Mode",
                "Switch the desktop to the light style",
                &["light mode", "light theme", "day mode"],
                "gsettings",
                &[
                    "set",
                    "org.gnome.desktop.interface",
                    "color-scheme",
                    "default",
                ],
            ));
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

        // `kind:action` lists every action and `kind:action lock` only the matching ones (the
        // launcher's Actions button, with and without text typed).
        let lowered = query.trim().to_lowercase();

        if let Some(text) = lowered
            .strip_prefix("kind:action")
            .filter(|rest| rest.is_empty() || rest.starts_with(' '))
            .map(str::trim)
        {
            return self
                .actions
                .iter()
                .filter_map(|action| {
                    let points = if text.is_empty() {
                        Some(500)
                    } else {
                        std::iter::once(action.title)
                            .chain(action.keywords.iter().copied())
                            .filter_map(|candidate| score(text, candidate))
                            .max()
                    };

                    points.map(|points| SearchResult {
                        id: action.id.to_owned(),
                        title: action.title.to_owned(),
                        subtitle: Some(action.subtitle.to_owned()),
                        kind: ResultKind::System,
                        score: points,
                        icon: None,
                        detail: None,
                    })
                })
                .collect();
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
                    detail: None,
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
    fn kind_action_lists_every_action() {
        let provider = SystemProvider::new();
        let listed = provider.search("kind:action");

        // Four core power/session actions are always present. Additional actions depend on
        // desktop tools (NetworkManager, PipeWire, GNOME, etc.) available on this host.
        assert_eq!(listed.len(), provider.actions.len());
        assert!(listed.len() >= 4);
        assert!(listed.iter().any(|result| result.id == "system:lock"));
        assert!(
            listed
                .iter()
                .all(|result| provider.resolve(&result.id).is_some())
        );

        let locks = provider.search("kind:action lock");

        assert!(locks.iter().any(|result| result.id == "system:lock"));
        assert!(locks.len() < listed.len());
        assert!(provider.search("kind:action zzzzqq").is_empty());
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
