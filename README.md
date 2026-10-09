# Aujar

**Native Linux Productivity Platform** — an open-source, modular toolkit for Linux desktops,
inspired by the utilities Windows users get from PowerToys. Independent project; not
affiliated with Microsoft.

[![CI](https://github.com/4mritGiri/aujar/actions/workflows/ci.yml/badge.svg)](https://github.com/4mritGiri/aujar/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
![MSRV](https://img.shields.io/badge/rust-1.85%2B-orange.svg)

> **Status: early development (v0.1.x).** Only the platform foundation and the batch-rename
> engine are functional. See the [feature matrix](docs/FEATURE_MATRIX.md) for exactly what
> works on X11, Sway/Hyprland, KDE and GNOME.

## About the name

*Aujar* (औजार) means "tool" in Nepali and Hindi, a nod to what the project is: a toolkit of
small, sharp desktop utilities.

## Documentation

| | |
|---|---|
| [Feature matrix](docs/FEATURE_MATRIX.md) | What is done / planned, per compositor |
| [Architecture](docs/ARCHITECTURE.md) | Design principles and layout |
| [Roadmap](docs/ROADMAP.md) | Milestones |
| [Threat model](docs/THREAT_MODEL.md) | Security assumptions |
| [Packaging](docs/PACKAGING.md) | Distro packaging plans |
| [Contributing](CONTRIBUTING.md) · [Governance](GOVERNANCE.md) · [Security](SECURITY.md) · [Code of Conduct](CODE_OF_CONDUCT.md) | Project policies |

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

The CLI communicates with the Aujar daemon through the Unix socket.

```bash
cargo run -p aujar-launcher-app -- daemon   # terminal 1
cargo run -p aujar-launcher-app -- ping     # terminal 2
cargo run -p aujar-launcher-app -- health
cargo run -p aujar-launcher-app -- windows
```

Rename preview:

```bash
cargo run -p aujar-launcher-app -- rename \
  --pattern old \
  --replacement new \
  old_report.txt old_data.txt
```

Apply:

```bash
cargo run -p aujar-launcher-app -- rename \
  --pattern old \
  --replacement new \
  old_report.txt old_data.txt \
  --apply
```

Regex:

```bash
cargo run -p aujar-launcher-app -- rename \
  --regex \
  --pattern '^(.+)\\.jpeg$' \
  --replacement '${1}.jpg' \
  photo.jpeg
```

## Architecture

```text
Aujar
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

## License

Apache-2.0. See [LICENSE](LICENSE).
