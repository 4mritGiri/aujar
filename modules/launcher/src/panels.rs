//! Pages of GNOME Settings: type `wifi`, `bluetooth`, `display` or `sound` and Enter opens that
//! page directly, instead of making you open Settings and look for it.
//!
//! This sits beside `system.rs`, which has the quick toggles (Wi-Fi on/off, mute, dark mode) and
//! one "System Settings" entry. Nothing is shown unless `gnome-control-center` is installed.
//! An unknown page name only opens the main Settings window, so a page that a newer or older
//! GNOME has renamed still does something sensible.

use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    ranking::score,
};

struct Panel {
    /// The id of the result (`panel:<id>`).
    id: &'static str,
    title: &'static str,
    /// Shown under the title.
    group: &'static str,
    /// The argument of `gnome-control-center`.
    page: &'static str,
    keywords: &'static [&'static str],
}

const PANELS: &[Panel] = &[
    Panel {
        id: "wifi",
        title: "Wi-Fi",
        group: "Network & Internet",
        page: "wifi",
        keywords: &["wifi", "wi-fi", "wireless", "internet", "hotspot"],
    },
    Panel {
        id: "network",
        title: "Network",
        group: "Network & Internet",
        page: "network",
        keywords: &["network", "ethernet", "vpn", "proxy", "lan", "dns"],
    },
    Panel {
        id: "bluetooth",
        title: "Bluetooth",
        group: "Devices",
        page: "bluetooth",
        keywords: &["bluetooth", "pairing", "headphones"],
    },
    Panel {
        id: "display",
        title: "Displays",
        group: "Devices",
        page: "display",
        keywords: &[
            "display",
            "monitor",
            "screen",
            "resolution",
            "night light",
            "scale",
            "refresh rate",
        ],
    },
    Panel {
        id: "sound",
        title: "Sound",
        group: "Devices",
        page: "sound",
        keywords: &[
            "sound",
            "audio",
            "volume",
            "speakers",
            "microphone",
            "output",
        ],
    },
    Panel {
        id: "power",
        title: "Power",
        group: "System",
        page: "power",
        keywords: &["power", "battery", "power saver", "suspend", "performance"],
    },
    Panel {
        id: "keyboard",
        title: "Keyboard",
        group: "Devices",
        page: "keyboard",
        keywords: &["keyboard", "shortcuts", "layout", "input source", "compose"],
    },
    Panel {
        id: "mouse",
        title: "Mouse & Touchpad",
        group: "Devices",
        page: "mouse",
        keywords: &[
            "mouse",
            "touchpad",
            "trackpad",
            "pointer",
            "scrolling",
            "tap to click",
        ],
    },
    Panel {
        id: "printers",
        title: "Printers",
        group: "Devices",
        page: "printers",
        keywords: &["printer", "printers", "printing"],
    },
    Panel {
        id: "users",
        title: "Users",
        group: "System",
        page: "user-accounts",
        keywords: &["users", "accounts", "password", "login", "automatic login"],
    },
    Panel {
        id: "datetime",
        title: "Date & Time",
        group: "System",
        page: "datetime",
        keywords: &["date", "time", "clock", "timezone", "time zone"],
    },
    Panel {
        id: "region",
        title: "Region & Language",
        group: "System",
        page: "region",
        keywords: &["region", "language", "locale", "formats"],
    },
    Panel {
        id: "privacy",
        title: "Privacy & Security",
        group: "Privacy",
        page: "privacy",
        keywords: &[
            "privacy",
            "security",
            "location",
            "camera",
            "usage history",
            "screen lock",
        ],
    },
    Panel {
        id: "lock",
        title: "Screen Lock",
        group: "Privacy",
        page: "lock",
        keywords: &[
            "screen lock",
            "lock screen",
            "blank screen",
            "automatic lock",
        ],
    },
    Panel {
        id: "notifications",
        title: "Notifications",
        group: "Apps",
        page: "notifications",
        keywords: &["notifications", "do not disturb", "alerts"],
    },
    Panel {
        id: "applications",
        title: "Apps",
        group: "Apps",
        page: "applications",
        keywords: &[
            "apps",
            "default apps",
            "default applications",
            "permissions",
        ],
    },
    Panel {
        id: "search",
        title: "Search",
        group: "Apps",
        page: "search",
        keywords: &["search", "search locations", "indexing"],
    },
    Panel {
        id: "multitasking",
        title: "Multitasking",
        group: "Desktop",
        page: "multitasking",
        keywords: &[
            "multitasking",
            "workspaces",
            "hot corner",
            "tiling",
            "app switching",
        ],
    },
    Panel {
        id: "appearance",
        title: "Appearance & Wallpaper",
        group: "Desktop",
        page: "background",
        keywords: &[
            "appearance",
            "background",
            "wallpaper",
            "dark mode",
            "dark style",
            "accent colour",
            "accent color",
            "theme",
        ],
    },
    Panel {
        id: "accounts",
        title: "Online Accounts",
        group: "Accounts",
        page: "online-accounts",
        keywords: &[
            "online accounts",
            "google",
            "nextcloud",
            "microsoft",
            "cloud",
        ],
    },
    Panel {
        id: "sharing",
        title: "Sharing",
        group: "System",
        page: "sharing",
        keywords: &[
            "sharing",
            "remote desktop",
            "screen sharing",
            "ssh",
            "file sharing",
            "media sharing",
            "device name",
        ],
    },
    Panel {
        id: "color",
        title: "Color",
        group: "Devices",
        page: "color",
        keywords: &["color", "colour", "color profile", "calibration"],
    },
    Panel {
        id: "wacom",
        title: "Wacom Tablet",
        group: "Devices",
        page: "wacom",
        keywords: &["wacom", "tablet", "pen", "stylus"],
    },
    Panel {
        id: "about",
        title: "About",
        group: "System",
        page: "info-overview",
        keywords: &[
            "about",
            "system info",
            "hostname",
            "device name",
            "os version",
            "memory",
        ],
    },
    Panel {
        id: "accessibility",
        title: "Accessibility",
        group: "Desktop",
        page: "universal-access",
        keywords: &[
            "accessibility",
            "a11y",
            "screen reader",
            "zoom",
            "large text",
            "high contrast",
            "cursor size",
        ],
    },
];

/// At most this many pages are offered for one query.
const MAX_RESULTS: usize = 4;

/// The page results rank a little below applications and answers: they are a way in, not the
/// thing most people mean by a bare word.
const PENALTY: u32 = 150;

pub struct SettingsPanelsProvider {
    /// Full path of `gnome-control-center`, when it is installed.
    program: Option<String>,
}

impl SettingsPanelsProvider {
    pub fn new() -> Self {
        Self {
            program: find_on_path("gnome-control-center"),
        }
    }

    /// Use a fixed program instead of looking for one (tests).
    pub fn with_program(program: Option<String>) -> Self {
        Self { program }
    }
}

impl Default for SettingsPanelsProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn find_on_path(program: &str) -> Option<String> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .and_then(|path| path.to_str().map(str::to_owned))
}

/// The words to look for: lower case, without a trailing "settings" ("wifi settings" means
/// "wifi"). `None` for queries that are too short or that belong to another provider.
fn normalize(query: &str) -> Option<String> {
    let mut text = query.trim().to_lowercase();

    if text.chars().count() < 2 || text.starts_with("kind:") || text.starts_with('>') {
        return None;
    }

    for suffix in [" settings", " setting", " preferences", " panel", " page"] {
        if let Some(stripped) = text.strip_suffix(suffix) {
            text = stripped.trim().to_owned();
            break;
        }
    }

    (text.chars().count() >= 2).then_some(text)
}

impl Provider for SettingsPanelsProvider {
    fn id(&self) -> &'static str {
        "panels"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        if self.program.is_none() {
            return Vec::new();
        }

        let Some(text) = normalize(query) else {
            return Vec::new();
        };

        let mut found: Vec<(u32, &Panel)> = PANELS
            .iter()
            .filter_map(|panel| {
                std::iter::once(panel.title)
                    .chain(panel.keywords.iter().copied())
                    .filter_map(|candidate| score(&text, candidate))
                    .max()
                    .map(|points| (points.saturating_sub(PENALTY), panel))
            })
            .collect();

        found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.title.cmp(b.1.title)));
        found.truncate(MAX_RESULTS);

        found
            .into_iter()
            .map(|(points, panel)| {
                SearchResult::new(
                    format!("panel:{}", panel.id),
                    panel.title,
                    ResultKind::Application,
                    points,
                )
                .with_subtitle(format!("Settings \u{b7} {}", panel.group))
            })
            .collect()
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let panel = PANELS
            .iter()
            .find(|panel| id.strip_prefix("panel:") == Some(panel.id))?;

        let Some(program) = &self.program else {
            return Some(Err("GNOME Settings is not installed".to_owned()));
        };

        Some(Ok(LaunchSpec {
            title: format!("{} settings", panel.title),
            program: program.clone(),
            args: vec![panel.page.to_owned()],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider() -> SettingsPanelsProvider {
        SettingsPanelsProvider::with_program(Some("/usr/bin/gnome-control-center".into()))
    }

    fn titles(results: &[SearchResult]) -> Vec<&str> {
        results.iter().map(|result| result.title.as_str()).collect()
    }

    #[test]
    fn common_words_find_their_page() {
        for (query, title) in [
            ("wifi", "Wi-Fi"),
            ("wi-fi", "Wi-Fi"),
            ("bluetooth", "Bluetooth"),
            ("display", "Displays"),
            ("monitor", "Displays"),
            ("sound", "Sound"),
            ("volume", "Sound"),
            ("touchpad", "Mouse & Touchpad"),
            ("timezone", "Date & Time"),
            ("wallpaper", "Appearance & Wallpaper"),
            ("battery", "Power"),
            ("printer", "Printers"),
        ] {
            let results = provider().search(query);

            assert!(
                titles(&results).contains(&title),
                "{query} should offer {title}, got {:?}",
                titles(&results)
            );
        }
    }

    #[test]
    fn the_word_settings_is_ignored() {
        assert_eq!(normalize("wifi settings").as_deref(), Some("wifi"));
        assert_eq!(
            normalize("  Bluetooth Preferences ").as_deref(),
            Some("bluetooth")
        );
        assert_eq!(normalize("display page").as_deref(), Some("display"));

        let plain = titles(&provider().search("wifi"))
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let worded = titles(&provider().search("wifi settings"))
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();

        assert_eq!(plain, worded);
    }

    #[test]
    fn short_or_foreign_queries_get_nothing() {
        assert!(provider().search("w").is_empty());
        assert!(provider().search("").is_empty());
        assert!(provider().search("kind:app wifi").is_empty());
        assert!(provider().search("> wifi").is_empty());
        assert!(provider().search("zzzzqq").is_empty());
    }

    #[test]
    fn at_most_a_few_pages_are_offered() {
        assert!(provider().search("a").is_empty());
        assert!(provider().search("er").len() <= MAX_RESULTS);
    }

    #[test]
    fn nothing_is_offered_without_gnome_settings() {
        let missing = SettingsPanelsProvider::with_program(None);

        assert!(missing.search("wifi").is_empty());

        // A known page still resolves, to a clear message.
        let Some(Err(message)) = missing.resolve("panel:wifi") else {
            panic!("expected an error");
        };

        assert!(message.contains("not installed"), "{message}");
    }

    #[test]
    fn a_page_opens_with_its_argument() {
        let Some(Ok(spec)) = provider().resolve("panel:users") else {
            panic!("expected a launch spec");
        };

        assert_eq!(spec.program, "/usr/bin/gnome-control-center");
        assert_eq!(spec.args, ["user-accounts"]);
        assert!(spec.title.contains("Users"));
    }

    #[test]
    fn only_known_pages_resolve() {
        assert!(provider().resolve("panel:rm-rf").is_none());
        assert!(provider().resolve("panel:").is_none());
        assert!(provider().resolve("apps:firefox.desktop").is_none());
        assert!(provider().resolve("system:lock").is_none());
    }

    #[test]
    fn every_page_has_what_it_needs() {
        let mut ids: Vec<&str> = PANELS.iter().map(|panel| panel.id).collect();

        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PANELS.len(), "ids are unique");

        for panel in PANELS {
            assert!(!panel.title.is_empty() && !panel.group.is_empty());
            assert!(!panel.page.is_empty() && !panel.page.contains(' '));
            assert!(!panel.keywords.is_empty());
            // Nothing may match a bare "settings": that word belongs to the System Settings action.
            assert!(
                !panel
                    .keywords
                    .iter()
                    .any(|keyword| keyword.contains("settings")),
                "{}",
                panel.id
            );
        }
    }
}
