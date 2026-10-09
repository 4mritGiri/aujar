# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versioning: [SemVer](https://semver.org/).

## [Unreleased]

### Added
- GPUI spike: keyboard-driven launcher (type, Up/Down, Enter to launch, Esc to close).
- GPUI spike (`apps/aujar-ui`, pinned `gpui-pre =0.3.3` snapshot, separate workspace): toggles a window from daemon events and shows search results. See docs/UI_SPIKE.md.
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
