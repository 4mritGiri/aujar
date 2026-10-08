# Nexora Architecture

## Principles

1. Platform before utilities.
2. Modules own their domain logic.
3. CLI/UI layers do not implement business logic.
4. Every crate explicitly declares its dependencies.
5. IPC is versioned.
6. Filesystem mutation requires explicit intent.
7. Wayland and X11 are capability-specific backends.

## Rename

The rename engine is intentionally separated into:

- `RenamePlanner`
- `RenamePlan`
- validation/status
- executor

The CLI only translates arguments into a request.

## Future

The daemon will become the long-running host for:

- module lifecycle
- hotkeys
- event bus
- persistent state
- IPC routing
- capability enforcement

The UI will consume those services rather than duplicating them.
