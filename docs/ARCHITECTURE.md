# Nexora Architecture

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

`nexora` CLI → versioned JSON over Unix socket → `nexora-daemon` → module → response.

## Backends

Each module that touches the desktop defines a trait and selects an implementation at
runtime from the detected session/compositor (see [FEATURE_MATRIX](FEATURE_MATRIX.md)).

## Rename

`RenamePlanner` → `RenamePlan` (per-item status) → `execute` (two-phase). The CLI only
translates arguments into a request.

## Known debt

- `Capability` is defined in both `nexora-core` and `nexora-runtime`; consolidate.
- `nexora-ipc` depends on `nexora-rename`; move protocol types out of domain crates.
- Capabilities are declared but not enforced.
