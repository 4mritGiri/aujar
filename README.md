# Nexora

**Native Linux Productivity Platform**

Nexora is an independent, modular Linux desktop productivity platform written in Rust. It is designed as a platform first: shared core services, versioned IPC, capability-aware modules, and compositor-specific integrations.

## v0.1.1

This release hardens the foundation and introduces a real, safe rename engine with:

- plan/preview before mutation
- explicit `--apply`
- literal and regex replacement
- collision detection
- duplicate target detection
- missing-file detection
- unchanged-name detection
- safe two-phase filesystem execution
- unit tests

## Build

```bash
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features
```

## CLI

The CLI communicates with the Nexora daemon through the Unix socket.

```bash
cargo run -p nexora-launcher-app -- ping
cargo run -p nexora-launcher-app -- health
cargo run -p nexora-launcher-app -- windows
```

Rename preview:

```bash
cargo run -p nexora-launcher-app -- rename \
  --pattern old \
  --replacement new \
  old_report.txt old_data.txt
```

Apply:

```bash
cargo run -p nexora-launcher-app -- rename \
  --pattern old \
  --replacement new \
  old_report.txt old_data.txt \
  --apply
```

Regex:

```bash
cargo run -p nexora-launcher-app -- rename \
  --regex \
  --pattern '^(.+)\\.jpeg$' \
  --replacement '${1}.jpg' \
  photo.jpeg
```

## Architecture

```text
Nexora
├── Core
├── Config
├── IPC
├── Daemon
├── Plugin/Capability system
├── Platform abstraction
└── Modules
    ├── Launcher
    ├── Window
    ├── Zones
    ├── Clipboard
    ├── Color
    ├── Ruler
    └── Rename
```

The next major milestone is the actual desktop Launcher UI and compositor integrations.
