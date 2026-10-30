//! Runtime configuration, validated and resolved from the user's settings.

use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use aujar_config::{EasyMouse, MatrixMode, MouseBorders, platform};
use aujar_core::Size;

use crate::{
    error::{Error, Result},
    keys::HotkeyBindings,
    layout::{Layout, MachineName, Shape},
    protocol::MAX_FILE_BYTES,
    router::SwitchPolicy,
};

/// Everything the engine needs, already checked.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub local: MachineName,
    pub layout: Layout,
    pub listen: SocketAddr,
    /// Every other machine in the layout and the address configured for it, if any.
    pub peers: BTreeMap<MachineName, Option<String>>,
    pub default_port: u16,
    pub policy: SwitchPolicy,
    pub move_relatively: bool,
    pub hide_mouse_at_edge: bool,
    pub same_subnet_only: bool,
    pub share_clipboard: bool,
    pub transfer_files: bool,
    pub max_file_bytes: u64,
    pub receive_dir: PathBuf,
    pub block_screensaver: bool,
    pub allow_remote_lock: bool,
    pub hotkeys: HotkeyBindings,
    pub screen_override: Option<Size>,
    /// Where to write the audit log; `None` switches auditing off.
    pub audit_log: Option<PathBuf>,
    pub timing: Timing,
}

/// Intervals and timeouts. Tests shorten them.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub heartbeat: Duration,
    pub peer_timeout: Duration,
    pub reconnect_min: Duration,
    pub reconnect_max: Duration,
    pub connect_timeout: Duration,
    /// How long the machine that loses the duplicate-connection tie-break waits for the
    /// other side to dial before dialing itself.
    pub dial_grace: Duration,
    pub clipboard_poll: Duration,
    pub keep_awake: Duration,
    pub screen_check: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(5),
            peer_timeout: Duration::from_secs(15),
            reconnect_min: Duration::from_secs(1),
            reconnect_max: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(5),
            dial_grace: Duration::from_secs(1),
            clipboard_poll: Duration::from_millis(700),
            keep_awake: Duration::from_secs(20),
            screen_check: Duration::from_secs(5),
        }
    }
}

impl EngineConfig {
    pub fn from_settings(settings: &MouseBorders) -> Result<Self> {
        let local = if settings.machine_name.trim().is_empty() {
            hostname_as_machine_name()
        } else {
            MachineName::new(settings.machine_name.trim())?
        };

        let mut machines = Vec::new();
        let mut addresses = BTreeMap::new();

        for machine in &settings.machines {
            let name = MachineName::new(&machine.name)?;
            let address = machine.address.trim();

            addresses.insert(
                name.clone(),
                (!address.is_empty()).then(|| address.to_owned()),
            );
            machines.push((name, machine.slot));
        }

        let shape = match settings.matrix {
            MatrixMode::Row => Shape::Row,
            MatrixMode::Grid => Shape::Grid,
            MatrixMode::Free => Shape::Plane,
        };
        let layout = Layout::new(shape, settings.wrap_mouse, machines)?;

        if !layout.contains(&local) {
            return Err(Error::Config(format!(
                "this machine (`{local}`) is not in `mouse_borders.machines`; add it, or set \
                 `machine_name` to the name it has there"
            )));
        }

        if layout.len() < 2 {
            return Err(Error::Config(
                "the layout needs at least one other machine".into(),
            ));
        }

        let peers = addresses
            .into_iter()
            .filter(|(name, _)| *name != local)
            .collect();

        Ok(Self {
            local,
            layout,
            listen: SocketAddr::from(([0, 0, 0, 0], settings.port)),
            peers,
            default_port: settings.port,
            policy: SwitchPolicy {
                easy_mouse: settings.easy_mouse,
                block_corners: settings.block_corners,
                ..SwitchPolicy::default()
            },
            move_relatively: settings.move_relatively,
            hide_mouse_at_edge: settings.hide_mouse_at_edge,
            same_subnet_only: settings.same_subnet_only,
            share_clipboard: settings.share_clipboard,
            transfer_files: settings.transfer_files,
            max_file_bytes: (u64::from(settings.max_file_mb) * 1024 * 1024).min(MAX_FILE_BYTES),
            receive_dir: receive_dir(&settings.receive_dir)?,
            block_screensaver: settings.block_screensaver,
            allow_remote_lock: settings.allow_remote_lock,
            hotkeys: HotkeyBindings::from_settings(&settings.hotkeys)?,
            screen_override: parse_screen(&settings.screen)?,
            audit_log: settings.audit_log.then(crate::audit::user_log_path),
            timing: Timing::default(),
        })
    }

    /// Whether switching with the mouse is switched off entirely.
    pub fn easy_mouse_enabled(&self) -> bool {
        self.policy.easy_mouse != EasyMouse::Disabled
    }
}

/// This machine's hostname, made into a valid machine name.
pub fn hostname_as_machine_name() -> MachineName {
    let raw = gethostname::gethostname().to_string_lossy().into_owned();

    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .take(63)
        .collect();

    MachineName::new(&cleaned).unwrap_or_else(|_| MachineName::new("aujar").expect("valid"))
}

fn parse_screen(text: &str) -> Result<Option<Size>> {
    let text = text.trim();

    if text.is_empty() {
        return Ok(None);
    }

    let bad = || Error::Config(format!("`screen = \"{text}\"` must look like 1920x1080"));
    let (width, height) = text.split_once(['x', 'X']).ok_or_else(bad)?;
    let width: u32 = width.trim().parse().map_err(|_| bad())?;
    let height: u32 = height.trim().parse().map_err(|_| bad())?;

    if !(1..=32_768).contains(&width) || !(1..=32_768).contains(&height) {
        return Err(bad());
    }

    Ok(Some(Size { width, height }))
}

fn receive_dir(configured: &str) -> Result<PathBuf> {
    let configured = configured.trim();

    if configured.is_empty() {
        return platform::downloads_dir().ok_or_else(|| {
            Error::Config("the home directory is unknown; set `mouse_borders.receive_dir`".into())
        });
    }

    expand_home(Path::new(configured), platform::home_dir().as_deref())
}

fn expand_home(path: &Path, home: Option<&Path>) -> Result<PathBuf> {
    platform::expand_home(path, home)
        .ok_or_else(|| Error::Config("`~` used but the home directory is unknown".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aujar_config::Machine;

    fn settings() -> MouseBorders {
        MouseBorders {
            enabled: true,
            machine_name: "desk".into(),
            machines: vec![
                Machine {
                    name: "desk".into(),
                    slot: 0,
                    address: String::new(),
                },
                Machine {
                    name: "laptop".into(),
                    slot: 1,
                    address: "192.168.1.20".into(),
                },
            ],
            receive_dir: "/tmp/aujar-test".into(),
            ..MouseBorders::default()
        }
    }

    #[test]
    fn resolves_a_valid_configuration() {
        let config = EngineConfig::from_settings(&settings()).unwrap();

        assert_eq!(config.local.as_str(), "desk");
        assert_eq!(config.peers.len(), 1);
        assert_eq!(
            config.peers[&MachineName::new("laptop").unwrap()].as_deref(),
            Some("192.168.1.20")
        );
        assert_eq!(config.max_file_bytes, 100 * 1024 * 1024);
        assert_eq!(config.listen.port(), MouseBorders::DEFAULT_PORT);
    }

    #[test]
    fn explains_what_is_wrong() {
        let mut missing_local = settings();
        missing_local.machine_name = "stranger".into();

        let mut alone = settings();
        alone.machines.truncate(1);

        let mut bad_name = settings();
        bad_name.machines[1].name = "lap top".into();

        let mut duplicate = settings();
        duplicate.machines[1].slot = 0;

        let mut bad_hotkey = settings();
        bad_hotkey.hotkeys.lock_machines = "ll".into();

        let message = |settings| {
            EngineConfig::from_settings(&settings)
                .unwrap_err()
                .to_string()
        };

        assert!(message(missing_local).contains("not in `mouse_borders.machines`"));
        assert!(message(alone).contains("at least one other machine"));
        assert!(message(bad_name).contains("machine name"));
        assert!(message(duplicate).contains("slot 0 is used twice"));
        assert!(message(bad_hotkey).contains("single letter or digit"));
    }

    #[test]
    fn screen_override_parsing() {
        assert_eq!(parse_screen("").unwrap(), None);
        assert_eq!(
            parse_screen("2560x1440").unwrap(),
            Some(Size {
                width: 2560,
                height: 1440
            })
        );
        assert!(parse_screen("2560").is_err());
        assert!(parse_screen("0x10").is_err());
        assert!(parse_screen("axb").is_err());
        assert!(parse_screen("99999x10").is_err());
    }

    #[test]
    fn home_expansion() {
        let home = Path::new("/home/me");

        assert_eq!(
            expand_home(Path::new("~/Dl"), Some(home)).unwrap(),
            Path::new("/home/me/Dl")
        );
        assert_eq!(
            expand_home(Path::new("/abs"), None).unwrap(),
            Path::new("/abs")
        );
        assert!(expand_home(Path::new("~/x"), None).is_err());
    }

    #[test]
    fn max_file_size_is_capped_at_the_protocol_limit() {
        let mut small = settings();
        small.max_file_mb = 5;

        assert_eq!(
            EngineConfig::from_settings(&small).unwrap().max_file_bytes,
            5 * 1024 * 1024
        );
    }

    #[test]
    fn hostname_is_always_a_valid_name() {
        let name = hostname_as_machine_name();

        assert!(MachineName::new(name.as_str()).is_ok());
    }

    #[test]
    fn the_shipped_example_configuration_is_valid() {
        let text = include_str!("../../../config/mouse-borders.example.toml");
        let mut settings: aujar_config::Settings = toml::from_str(text).unwrap();
        settings.mouse_borders.receive_dir = "/tmp/aujar-example".into();

        let config = EngineConfig::from_settings(&settings.mouse_borders).unwrap();

        assert_eq!(config.local.as_str(), "desk");
        assert_eq!(config.layout.len(), 2);
        assert!(config.same_subnet_only);
    }
}
