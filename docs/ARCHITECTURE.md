# Aujar Architecture

## Principles

1. Platform before utilities.
2. Modules own their domain logic.
3. CLI/UI layers do not implement business logic.
4. Every crate explicitly declares its dependencies.
5. IPC is versioned.
6. Filesystem mutation requires explicit intent.
7. Wayland and X11 are capability-specific backends.
8. Features degrade gracefully: unsupported backends report it instead of failing silently.

## Layout

```text
apps/       user-facing binaries (CLI today, UI later)
crates/     platform: core types, config, IPC, runtime, daemon, platform detection, plugin
modules/    feature domains (rename, window, zones, clipboard, ...)
packaging/  systemd unit, desktop entry, distro packaging
docs/       architecture, ADRs, feature matrix, threat model
```

## Request flow

`aujar` CLI → versioned JSON over Unix socket → `aujar-daemon` → module → response. UIs may instead
`Subscribe` and receive pushed events ([IPC](IPC.md)).

## Backends

Each module that touches the desktop defines a trait and selects an implementation at
runtime from the detected session/compositor (see [FEATURE_MATRIX](FEATURE_MATRIX.md)).

## Rename

`RenamePlanner` → `RenamePlan` (per-item status) → `execute` (two-phase). The CLI only
translates arguments into a request.

## UI

The UI is a separate GPUI process that talks to the daemon over IPC; see [UI_UX](UI_UX.md) and
[ADR 0002](adr/0002-ui-framework-gpui.md).

## Known debt

- `aujar-ipc` depends on `aujar-rename`; move protocol types out of domain crates.
- Capability enforcement is policy-wide only; per-client grants are not implemented.
