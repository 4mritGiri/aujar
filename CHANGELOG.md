# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Added
- `aujar shortcut install|remove`: adds/removes a GNOME custom shortcut (Ctrl+Alt+Space) running `aujar launcher toggle` via `gsettings`; works where the portal route fails (GNOME 50: "app id required"). Docs record the portal findings.
- Color picker gives up after 120 s instead of waiting forever; `aujar doctor` explains the "app id required" portal failure; troubleshooting steps for it.
- Clearer version-skew handling: the daemon answers unparseable requests with an error, and the client explains an empty reply; `install-local.sh` now restarts the daemon.
- `aujar doctor` and IPC `Status`: reports daemon reachability, session, subscribed UIs and global-shortcut state (bound trigger or the reason it is unavailable); daemon logs each shortcut press; troubleshooting guide.
- UI: Enter on a calculation result copies the answer to the clipboard.
- Color picker: `aujar color pick` (desktop portal `PickColor`, via the daemon, gated by `screen_capture`) and `aujar color convert <hex>`; hex/RGB/HSL conversion in `aujar-color` with tests.
- Packaging: `.deb` (cargo-deb), `.rpm` (cargo-generate-rpm), tarball for x86_64 and aarch64, Arch PKGBUILD, per-user `install-local.sh`; CI builds and installs the deb on every PR; tag-triggered release workflow.
- Native global hotkey (`aujar-hotkeys`): registers Ctrl+Alt+Space through the `GlobalShortcuts` portal and toggles the launcher; falls back with a log message when the portal is missing. `Options::hotkeys` (tests disable it); policy `global_hotkeys` disables it.
- GPUI spike: keyboard-driven launcher (type, Up/Down, Enter to launch, Esc to close).
- GPUI spike (`apps/aujar-ui`, pinned `gpui-pre =0.3.3` snapshot, non-default workspace member: `cargo run -p aujar-ui --release`): toggles a window from daemon events and shows search results. See docs/UI_SPIKE.md.
- IPC event stream: `Subscribe` connections, `Launcher` show/hide/toggle broadcast, `Shutdown` event; `aujar launcher` and `aujar events`.
- `aujar_ipc::{encode_line, decode_line, Envelope, subscribe, EventStream}`; daemon and client share one framing implementation.
- Daemon integration tests for the event stream. Docs: IPC protocol and keyboard-shortcut recipes.
- `Execute` IPC request and `aujar run <id>`: launches indexed applications (no shell, field codes dropped, own process group).
- Capability enforcement: each request maps to a capability; `/etc/aujar/policy.toml` can deny capabilities (fail-closed).
- `Capability::ALL` / `Capability::parse`; `tokio` `process` feature.
- Launcher engine: provider trait, application (`.desktop`) and calculator providers, tiered ranking.
- IPC `Search` request/response (protocol v1, additive) and CLI `aujar search` / `aujar modules`.
- ADR 0002 (GPUI UI proposal) and UI/UX design doc.
- Feature matrix, threat model, packaging notes, ADR process.
- Governance, security policy, code of conduct, contributing guide, issue/PR templates.
- CI matrix (Ubuntu/Fedora/Arch), MSRV, `cargo-deny`, Dependabot, release workflow.
- Daemon handles `Request::Modules`.

### Changed
- systemd unit now targets `/usr/bin/aujar` and `graphical-session.target`; removed mount-namespace hardening that fails in unprivileged user units.
- `Capability` now has a single definition in `aujar-core` (re-exported by `aujar-runtime`).
- Daemon rejects IPC clients whose uid is neither the daemon's owner nor root; 5 s request read timeout.
- Default socket path uses `XDG_RUNTIME_DIR` instead of a hard-coded uid.
- License metadata set to Apache-2.0 to match `LICENSE`.
- IPC socket is `0600`; requests capped at 64 KB; daemon handles SIGTERM.
- systemd unit and desktop entry now call `aujar daemon`.

### Fixed
- Non-exhaustive match on `Request` in the daemon.
- Rename rejects targets containing path separators, NUL, `.` or `..`.

### Removed
- Unreferenced legacy `powertoys-*` crates and apps.

## [0.1.1]
- Rename engine with plan/preview, collision detection and two-phase execution.
