//! The daemon starts the user's applications, so its systemd unit must not sandbox them.

use std::{fs, path::PathBuf};

/// Directives whose effect is inherited by every process the service starts. Any of them in
/// `aujar.service` would apply to Firefox, VirtualBox and everything else the launcher opens.
const INHERITED_SANDBOXING: &[&str] = &[
    "NoNewPrivileges",
    "LockPersonality",
    "RestrictSUIDSGID",
    "MemoryDenyWriteExecute",
    "ProtectSystem",
    "ProtectHome",
    "PrivateTmp",
    "PrivateDevices",
    "PrivateUsers",
    "PrivateNetwork",
    "ProtectKernelTunables",
    "ProtectKernelModules",
    "ProtectControlGroups",
    "ProtectClock",
    "ProtectProc",
    "ReadOnlyPaths",
    "InaccessiblePaths",
    "CapabilityBoundingSet",
    "AmbientCapabilities",
    "SystemCallFilter",
    "SystemCallArchitectures",
    "RestrictNamespaces",
    "RestrictAddressFamilies",
    "RestrictRealtime",
    "UMask",
];

fn unit(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packaging/systemd")
        .join(name);

    fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_daemon_unit_does_not_sandbox_the_applications_it_starts() {
    for line in unit("aujar.service").lines() {
        let line = line.trim();

        if line.starts_with('#') {
            continue;
        }

        let key = line.split('=').next().unwrap_or_default().trim();

        assert!(
            !INHERITED_SANDBOXING.contains(&key),
            "aujar.service sets `{key}`, which every launched application inherits \
             (it broke VirtualBox's setuid helper); see the comment in the unit"
        );
    }
}

#[test]
fn the_ui_unit_does_not_either() {
    // The UI starts nothing today, but it shares the user's session and display.
    for line in unit("aujar-ui.service").lines() {
        let key = line.trim().split('=').next().unwrap_or_default().trim();

        assert!(
            !["NoNewPrivileges", "LockPersonality", "ProtectHome", "ProtectSystem"]
                .contains(&key),
            "aujar-ui.service sets `{key}`"
        );
    }
}
