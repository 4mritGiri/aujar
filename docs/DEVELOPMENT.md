# Developing Aujar

Everything you need to build, run, test and debug Aujar. The shared editor setup is for
[Zed](https://zed.dev) (`.zed/` in the repository); any editor with rust-analyzer works too.

## Quick start

```bash
git clone --recurse-submodules https://github.com/4mritGiri/aujar && cd aujar
./scripts/setup-dev.sh        # system packages, Rust, just, cargo-deny, nextest, watch
just check                    # fmt + clippy + tests: the same gate CI runs
```

Forgot `--recurse-submodules`? `git submodule update --init` (only NexDesk needs it; Aujar builds
without it).

`scripts/setup-dev.sh` asks before each step and only installs what is missing. Useful flags:
`--yes` (no prompts), `--dry-run` (print, change nothing), `--check` (just report), `--optional`
(also install runtime helpers such as `wmctrl`, `wl-clipboard`, `ffmpegthumbnailer`). It supports
apt, dnf and pacman. Anything else: it prints what to install by hand.

Requirements: Rust 1.85+ (edition 2024, pinned to `stable` by `rust-toolchain.toml`) with
`rustfmt` and `clippy`.

## Opening the project in Zed

Open the repository folder. Three files in `.zed/` are shared with the team:

| File | What it gives you |
|---|---|
| `settings.json` | rust-analyzer runs **clippy** on save, like CI, with its own target dir (`target/rust-analyzer`) so it never blocks your terminal `cargo`. The GPUI UI crate is left out of on-save diagnostics (see below). Format on save, 4-space Rust and 2-space TOML/YAML/JSON/Markdown, as in `.editorconfig`. |
| `tasks.json` | Ready-made tasks: checks, running the daemon and UI, inspecting a running daemon, service control. |
| `debug.json` | Debugger launch configs (CodeLLDB) for the daemon, the CLI, the UI and the Mouse Borders agent. |

**Running a task:** open the command palette and run `task: spawn`, then type part of a name
(`check`, `dev:`, `run:`, `aujar:`). `task: rerun` repeats the last one. Tasks run in Zed's
terminal panel.

**Debugging:** put a breakpoint in the gutter, then `debugger: start` and pick a "Debug: ..."
configuration. It builds first, so you debug the current code, using the `debugging` Cargo
profile (full debug info, so local variables show up; it lives in `target/debugging/` and never
disturbs your normal `target/debug/` builds; the first debugging build compiles everything once).
(Needs a Zed version with the debugger and the CodeLLDB adapter, which Zed fetches itself.) Without a debugger, use logging:
see [Logs](#logs-and-diagnostics).

**Tests:** Zed shows a run button in the gutter next to each `#[test]`; the task *check: cargo
test for the crate of the current file* runs a whole crate.

### `aujar-ui` and rust-analyzer

`aujar-ui` (the GPUI launcher and settings window) needs windowing and GPU headers and has a
large dependency tree, so, like `just lint` and CI, the editor's on-save diagnostics skip it
(`"extraArgs": ["--exclude", "aujar-ui"]` in `.zed/settings.json`). Completion and
go-to-definition still work inside it. When you work on the UI:

- run the task **ui: check**, or `just check-ui`, to get its clippy results; or
- delete that `extraArgs` line from your copy of `.zed/settings.json` while you work on it.

If rust-analyzer reports missing system libraries for the UI, run `./scripts/setup-dev.sh`.

## Repository map

```text
apps/aujar-launcher/   the `aujar` CLI. Cargo package: aujar-launcher-app (binary: aujar)
apps/aujar-ui/         GPUI launcher + settings window (separate process, talks to the daemon)
apps/aujar-mwb/        standalone Mouse Borders agent (also Windows and macOS)
crates/aujar-core/     shared types (Capability, Rect, ...)
crates/aujar-config/   typed settings, policy, persistent state
crates/aujar-ipc/      versioned JSON-line protocol (Request / Response / Event)
crates/aujar-daemon/   the daemon: socket server, module wiring, events
crates/aujar-runtime/  module lifecycle and registry
crates/aujar-platform/ session / desktop detection
crates/aujar-hotkeys/  global shortcuts through the desktop portal
crates/aujar-plugin/   plugin manifest (extension SDK to come)
crates/aujar-mwb-cli/  command-line shared by `aujar mwb` and `aujar-mwb`
modules/               one domain per directory: launcher (search providers), rename,
                       image-resizer, color, awake, mouse-borders, nexdesk, window, zones,
                       clipboard, ruler, workspace (the last five are still stubs)
integrations/nexdesk/  git submodule, its own Cargo workspace, never built with Aujar's cargo
packaging/             systemd units, .desktop files, deb/rpm/Arch, GNOME extension
docs/                  architecture, IPC, feature matrix, threat model, ADRs
```

Rule of thumb from [ARCHITECTURE](ARCHITECTURE.md): domain logic lives in `modules/` or `crates/`;
the CLI and UI only translate requests and render results.

**Naming trap:** `modules/launcher` (package `aujar-launcher`) is the search engine library, while
`apps/aujar-launcher` (package `aujar-launcher-app`) is the command line program. Commands use
`-p aujar-launcher-app`.

**Leftovers:** `crates/{core,config,daemon,ipc,platform,aujar-ui}` and the small stubs in
`apps/` (`always_on_top`, `aujar`, `aujar-daemon`, `aujar-settings`, `color_picker`, `launcher`,
`power_rename`, `screen_ruler`) are not part of the Cargo workspace and are not built. Do not
add code there; they are planned for removal.

## Everyday commands

| Command | What it does |
|---|---|
| `just check` | fmt check, clippy (`-D warnings`), tests; the CI gate (without the UI) |
| `just fmt` | format the workspace |
| `just test` / `just lint` | tests only / clippy only |
| `just check-ui` / `just test-ui` | clippy / tests for `aujar-ui` (needs GPU and windowing headers) |
| `just audit` | `cargo deny check` (licenses, advisories) |
| `just setup` / `just setup-check` | run the setup script / only verify the environment |
| `cargo test -p aujar-mouse-borders` | one crate's tests (the biggest suite, ~290 tests) |
| `cargo aujar <args>` | run the `aujar` CLI: `cargo aujar ping`, `cargo aujar search fire` (alias for `run -p aujar-launcher-app --`) |
| `cargo aujar-ui` / `cargo mwb <args>` | run the UI (debug build) / the Mouse Borders agent |
| `cargo lint` / `cargo lint-ui` / `cargo test-all` | the clippy and test gates as cargo aliases (`.cargo/config.toml`) |
| `just clean-ra` / `just target-size` / `just timings` | delete only rust-analyzer's build dir / show disk use per profile / write a build timing report |
| `just --list` | everything else (builds, packaging, installed-service control) |

The UI is excluded from plain `cargo build` / `cargo check` on purpose. Build it explicitly with
`-p aujar-ui`. Run the platform and UI builds as separate Cargo invocations (the `justfile` does).

## Faster builds

What is already set up in the repository, and what you can add on your machine.

**In the repository (`Cargo.toml`, nothing to do):**

| Setting | Effect |
|---|---|
| `[profile.dev] debug = "line-tables-only"`, `split-debuginfo = "unpacked"` | much less debug info to write and link; backtraces, panics and line breakpoints still work |
| `[profile.dev.package.<codec / crypto crate>] opt-level = 2` | the image codecs (`image`, `png`, `zune-jpeg`, ...) and the cryptography Mouse Borders uses (`snow`, `chacha20poly1305`, `curve25519-dalek`, ...) are optimized even in debug builds, so resizing a picture or running the pairing tests is not painfully slow; Aujar's own crates are untouched |
| `[profile.dev.build-override] opt-level = 1` | build scripts and proc macros run faster on every build |
| `[profile.debugging]` | full debug info for the debugger, in its own `target/debugging/` directory (used by `.zed/debug.json`) |

The first build after pulling these changes recompiles everything once (the profile changed).
Need local variables in a plain `cargo run`? `CARGO_PROFILE_DEV_DEBUG=full cargo run ...`.

**On your machine (optional, in `~/.cargo/config.toml`, so it affects only you):**

- **Fast linker.** Linking the big binaries (`aujar-ui`, the tests) is a large part of an
  incremental rebuild. `./scripts/setup-dev.sh --fast-linker` installs `mold` and `clang` and prints
  the snippet to add:

  ```toml
  [target.x86_64-unknown-linux-gnu]
  linker = "clang"
  rustflags = ["-C", "link-arg=-fuse-ld=mold"]
  ```

  It is deliberately not committed to the repository: a project-wide setting would break the build
  on machines and CI images without `mold`.
- **Compiler cache.** [`sccache`](https://github.com/mozilla/sccache) (`cargo install sccache
  --locked`, then `build.rustc-wrapper = "sccache"` in `~/.cargo/config.toml`) speeds up clean
  builds and switching branches. It matters most for `aujar-ui`'s large GPUI dependency tree.
- **Disk.** `target/` grows quickly with the UI. `just target-size` shows where the space goes;
  `cargo clean -p <crate>` or `cargo sweep` (a separate tool) trim it. rust-analyzer's own
  directory (`target/rust-analyzer`) can be deleted at any time with `just clean-ra`.

Measure instead of guessing: `just timings` (a `cargo build --timings` report in
`target/cargo-timings/`) shows what the time goes to. Run it before and after trying a setting, with
the same starting state (`cargo clean` first for a cold build, or touch one file for an incremental one).

Two habits help more than any setting: use `cargo check` / `cargo clippy` (or the editor) while
writing, and build and test only the crate you are changing (`cargo test -p aujar-rename`)
instead of the whole workspace. Plain `cargo build` and `cargo check` already skip `aujar-ui`.

## Running a development copy

Run the daemon and the UI from source, beside an installed Aujar, on a separate socket. The CLI
and the UI both read `AUJAR_SOCKET`. In Zed, use the `dev:` tasks, which set it for you:

1. `dev: daemon on its own socket (/tmp/aujar-dev.sock)`
2. `dev: UI on the dev socket (debug build)`
3. `dev: events from the dev daemon` / `dev: doctor against the dev daemon`

The same in a terminal:

```bash
export AUJAR_SOCKET=/tmp/aujar-dev.sock
RUST_LOG=debug cargo run -p aujar-launcher-app -- daemon     # terminal 1
cargo run -p aujar-ui                                         # terminal 2
cargo run -p aujar-launcher-app -- search fire                # terminal 3 (same variable)
cargo run -p aujar-launcher-app -- launcher toggle            # opens the dev UI
```

What a separate socket does **not** isolate: your configuration and state files
(`~/.config`, `~/.local/state`), and desktop-wide resources such as the global shortcut and
Mouse Borders' network port. To test the keyboard shortcut or Mouse Borders, stop the installed
services first (`services: stop the installed Aujar`, or
`systemctl --user stop aujar.service aujar-ui.service`) and use the plain `run:` tasks, then
start them again afterwards.

Behaviour that needs real hardware or a real desktop (Wayland edge switching, GNOME hotkeys,
window focus, Windows and macOS Mouse Borders) cannot be covered by unit tests. State the distro,
session type and compositor you tested on in your pull request, as
[CONTRIBUTING](../CONTRIBUTING.md) asks.

## Logs and diagnostics

| Need | Use |
|---|---|
| Verbose daemon logs | `RUST_LOG=debug` (or `RUST_LOG=aujar_mouse_borders=trace,info`); the daemon logs to stderr, and to journald when run by systemd |
| Logs of the installed services | `just logs`, or the task `services: logs of the installed Aujar` |
| Is the daemon up, which session, is the shortcut bound? | `aujar doctor` |
| What can this desktop do? | `aujar platform` |
| See daemon to UI events live | `aujar events`, then press the shortcut |
| Mouse Borders problem report | `aujar mwb report` (paste it into the issue) |
| Installed service state | `just status` |

The `aujar-ui` process does not install its own logging yet, so UI problems are easiest to find
from the daemon side (`aujar events`) or with the debugger. Shared logging setup is on the list
of improvements below.

## Common problems

| Symptom | Fix |
|---|---|
| `pkg-config` or linker errors building `aujar-ui` | `./scripts/setup-dev.sh`; `--check` lists what is missing |
| UI window does not open | `vulkaninfo --summary` (package `vulkan-tools`); see the checklist in [UI_SPIKE](UI_SPIKE.md) |
| The daemon refuses to start because another one is running | the installed service owns the default socket: use the `dev:` tasks or stop the service |
| Shortcut does nothing | [TROUBLESHOOTING](TROUBLESHOOTING.md) |
| NexDesk sources missing | `git submodule update --init` (only `scripts/build-nexdesk.sh` needs them) |
| Editor and terminal `cargo` wait for each other | `.zed/settings.json` gives rust-analyzer its own target dir; if you use another editor, set `rust-analyzer.cargo.targetDir` to `true` |

## Dev container

`.devcontainer/devcontainer.json` builds an Ubuntu 24.04 environment and runs the setup script.
It suits building, `just check` and working on any crate. It has no desktop session, GPU,
portals or input devices, so run the launcher UI and Mouse Borders on your host.

## What is planned for the developer experience

Done: setup (`scripts/setup-dev.sh`, Zed config, this page) and faster builds. Next: `just dev`
with file watching and a two-daemon Mouse Borders lab, shared logging setup for every binary
(including the UI) and IPC tracing, offline fixtures for the online launcher providers, an
integration test for daemon plus CLI, and git hooks for commit messages and sign-off.
