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
crates/aujar-log/      logging setup shared by every binary (RUST_LOG, -v, IPC trace, log file)
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

> **`aujar` is the installed program; `cargo aujar` is the one you are building.** After
> `./scripts/install-local.sh` or a package install, a bare `aujar` in your shell runs the installed
> (older) binary: a new flag such as `-v` or `doctor --dev` will be "unexpected" there. While
> developing, use `cargo aujar ...` (an alias for `cargo run -p aujar-launcher-app --`), or
> `target/debug/aujar`. Without `AUJAR_SOCKET` either one talks to the installed daemon.


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
| `just ci` | everything CI runs: the check above, the MSRV build (Rust 1.85, if installed), `cargo deny` (if installed) and the Mouse Borders crates; run before opening a PR |
| `just dev` | daemon + UI on a private socket, rebuilt and restarted on every save ([details](#the-daily-loop)) |
| `just dev-daemon` / `just dev-isolated` | daemon only / with its own settings and state |
| `just watch` / `just watch-test <crate>` | re-run `cargo check` / one crate's tests on every save |
| `just test-crate <crate> [args]` / `just nextest` | one crate's tests / the whole suite with cargo-nextest |
| `just doctor` | `aujar doctor` and `aujar mwb doctor` (honors `AUJAR_SOCKET`) |
| `just lab-up` / `lab-smoke` / `lab-down` | the Mouse Borders lab ([details](#mouse-borders-lab-two-daemons-on-one-machine)) |
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

## The daily loop

```bash
just dev            # daemon + UI, rebuilt and restarted whenever you save
```

(In Zed: `task: spawn`, then `dev: everything, rebuilt on save`.) It needs `cargo-watch`
(`./scripts/setup-dev.sh` installs it) and does this:

- runs the daemon and the UI **on a private socket**, `/tmp/aujar-dev.sock`, so your installed
  Aujar keeps working next to it (the UI's one-instance lock is named after the socket, so the dev
  UI is not refused while the installed `aujar-ui.service` runs);
- runs the daemon **without the global shortcut** (`AUJAR_NO_HOTKEYS=1`), so it cannot fight the
  installed daemon for it; use the launcher from the CLI instead (see below);
- turns on debug logging for the daemon and the launcher engine;
- rebuilds and restarts the daemon when `crates/`, `modules/` or `apps/aujar-launcher` change, and
  the UI when `crates/`, `modules/` or `apps/aujar-ui` change;
- waits for the daemon's socket before starting the UI (the UI is the slow build);
- on Ctrl+C stops both and removes the socket.

Use it from a second terminal by pointing the CLI at the dev socket:

```bash
export AUJAR_SOCKET=/tmp/aujar-dev.sock
cargo aujar doctor                  # is it healthy?
cargo aujar launcher toggle         # show / hide the dev UI
cargo aujar search fire             # the launcher engine, without the UI
cargo aujar events                  # watch daemon to UI events
```

Variants: `just dev-daemon` (no UI, no GPU build; for the CLI, launcher engine and Mouse Borders),
`just dev-isolated` (own settings and state under `/tmp/aujar-dev-home`, so UI settings changes never
touch your real `~/.config/aujar`; fontconfig also reads `XDG_CONFIG_HOME`, so fonts may differ),
and `./scripts/dev.sh --help` for the rest (`--no-watch`, `--hotkeys`, `--ipc-trace`).

When it does **not** help: the global keyboard shortcut and anything needing the real daemon's
desktop-wide resources. To test the shortcut, stop the installed services
(`task: services: stop the installed Aujar`, or `systemctl --user stop aujar.service
aujar-ui.service`), run `./scripts/dev.sh --hotkeys`, and start them again afterwards. Behaviour that
needs real hardware or a real desktop (Wayland edge switching, GNOME hotkeys, window focus, Windows and
macOS Mouse Borders) cannot be covered by unit tests: state the distro, session type and compositor you
tested on in your pull request, as [CONTRIBUTING](../CONTRIBUTING.md) asks.

If the UI does not reconnect after the daemon restarts (the reconnect check in
[UI_SPIKE](UI_SPIKE.md) is still open), restart `just dev`.

### Environment variables that matter for development

| Variable | Effect |
|---|---|
| `AUJAR_SOCKET` | socket path for the daemon, the CLI and the UI (default `$XDG_RUNTIME_DIR/aujar.sock`) |
| `AUJAR_NO_HOTKEYS=1` | the daemon does not register the global shortcut |
| `RUST_LOG` | log filter, e.g. `debug` or `info,aujar_mouse_borders=trace` |
| `AUJAR_DEBUG`, `AUJAR_IPC_TRACE`, `AUJAR_LOG_FILE` | debug messages / one line per IPC request / also log to a file (see [Logs and diagnostics](#logs-and-diagnostics)) |
| `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | where settings (`aujar/settings.toml`) and state (`state.toml`, the Mouse Borders key and audit log) live |
| `AUJAR_DEV_HOME` | base directory for `just dev-isolated` (default `/tmp/aujar-dev-home`) |
| `AUJAR_LAB_DIR`, `AUJAR_LAB_PORT_A`, `AUJAR_LAB_PORT_B` | the Mouse Borders lab (below) |

## Mouse Borders lab: two daemons on one machine

Mouse Borders needs two computers, which makes everything around it slow to test. The lab starts two
Aujar daemons, `lab-a` and `lab-b`, on your machine, each with its own settings, state, socket and
ports, pairs them with a shared key over `127.0.0.1`, and gives you commands for each.

```bash
just lab-up                         # build, start both, pair, wait until connected, show status
just lab a mwb status               # any aujar command as machine A (or b)
just lab b mwb audit
./scripts/mwb-lab.sh logs a         # follow a daemon's log
./scripts/mwb-lab.sh send           # send a test file from A to B and check it arrived intact
just lab-smoke                      # up, connect, send a file, down: exit 1 on failure
just lab-down                       # stop both (settings and logs stay in /tmp/aujar-lab)
./scripts/mwb-lab.sh clean          # stop and delete the lab directory
```

**Good for:** pairing and the security key, connect and reconnect, the layout and settings, file
transfer, status and the audit log, anything in the wire protocol and the service. Run `just lab-smoke`
after changing `modules/mouse-borders`: it is a quick end-to-end check on top of the unit tests.

**Not for:** moving the pointer or the keyboard between machines. Input capture, edge switching and
injection need real screens; test them on two computers (see [MOUSE_BORDERS](MOUSE_BORDERS.md)).

**Safety.** The lab daemons run with no `DISPLAY`/`WAYLAND_DISPLAY`, no global shortcut, edge switching,
clipboard sharing, screensaver blocking and remote lock turned off, so they leave your desktop alone.
Linux input capture (evdev/uinput), however, does not need a display. If your account can access
`/dev/uinput` or `/dev/input/event*` (the `input` group, which `aujar mwb setup` grants), the lab
**refuses to start**: two daemons could grab your real keyboard. Test as a user without that access,
or set `MWB_LAB_FORCE=1` and do not press the `Ctrl+Alt` hotkeys while it runs.

In Zed: `task: spawn`, then `lab: Mouse Borders up`, `status`, `smoke test`, `down`.

## Logs and diagnostics

Every Aujar program (the daemon, the UI, the CLI, the Mouse Borders agent and service) sets up
logging the same way, through the `aujar-log` crate, and is controlled by the same switches:

| Switch | Effect |
|---|---|
| `RUST_LOG=...` | the standard `tracing` filter, and it wins over the rest: `debug`, `info,aujar_mouse_borders=trace`, `aujar::ipc=debug` |
| `-v`, `-vv` (CLI) | Aujar's debug / trace messages, e.g. `cargo aujar -v search fire`, `cargo aujar -v daemon` |
| `AUJAR_DEBUG=1` | the same as `-v` for programs that take no flags (the UI, the daemon) |
| `AUJAR_IPC_TRACE=1` | one line per request between a client (CLI, UI) and the daemon |
| `AUJAR_LOG_FILE=1` | also write `<state dir>/logs/<program>.log` (`~/.local/state/aujar/logs/` on Linux) |

Notes:

- **Without any switch** the daemon, UI and agent log at `info` to stderr (journald under systemd);
  the CLI stays silent, so command output is never mixed with log lines.
- **`aujar=debug` covers every Aujar crate**: a filter on a target matches by prefix, so it includes
  `aujar_daemon`, `aujar_ui`, `aujar_mouse_borders`, and so on.
- **Log files** are private to you (mode 0600), are never sent anywhere, and are rotated: a file over
  5 MiB becomes `<name>.log.1`. This is what makes the UI debuggable: it is normally started from the
  desktop and has no terminal. Try `AUJAR_LOG_FILE=1 AUJAR_DEBUG=1 cargo run -p aujar-ui` (with the installed UI stopped, or `AUJAR_SOCKET` set), or the Zed task
  *run: UI with a log file*, and follow the files with *logs: follow the Aujar log files*.
- **Panics** are logged too, so they reach the file.
- **GPUI's own messages** (GPU selection, window events) arrive through the same path. Look there
  first when the UI will not open.

### Tracing the IPC

`AUJAR_IPC_TRACE=1` (or `RUST_LOG=aujar::ipc=debug`) logs every request, on both sides: the daemon
logs what it handled and the client (CLI, UI) what it sent.

```text
DEBUG aujar::ipc: handled request=Search response=Search elapsed_ms=12
DEBUG aujar::ipc: sent socket=/tmp/aujar-dev.sock request=Execute response=Executed elapsed_ms=4
DEBUG aujar::ipc: request failed socket=... request=Ping elapsed_ms=0 error=failed to connect ...
DEBUG aujar::ipc: event subscriber connected
```

Only the **kind** of request (`Search`, `Execute`, ...), the outcome and the time are logged, never the
contents: they can hold what you typed into the launcher, file names, clipboard data or key
material. When you add a new request type there is nothing to do; its name appears automatically.
For the dev loop: `./scripts/dev.sh --ipc-trace`, or the Zed task *dev: everything with IPC trace*.

### Which daemon am I talking to?

```bash
cargo aujar doctor --dev
```

On top of the normal checks it prints which socket this command uses **and why** (`AUJAR_SOCKET`,
`XDG_RUNTIME_DIR` or the built-in fallback) and whether it exists, the settings and state
directories, the log-file location, the `AUJAR_*` / `RUST_LOG` / `XDG_*` variables that are set, the
state of the installed `aujar.service` and `aujar-ui.service`, and a note when another daemon is
listening on the default socket. That covers the common development confusion: "my change has no
effect" because the CLI is talking to the installed daemon, or the other way round. (Zed tasks:
*aujar: doctor --dev* and *dev: doctor --dev against the dev daemon*.)

### The other diagnostics

| Need | Use |
|---|---|
| Logs of the installed services | `just logs`, or the task `services: logs of the installed Aujar` |
| Is the daemon up, which session, is the shortcut bound? | `aujar doctor` |
| What can this desktop do? | `aujar platform` |
| See daemon to UI events live | `aujar events`, then press the shortcut |
| Mouse Borders problem report | `aujar mwb report` (paste it into the issue) |
| Installed service state | `just status` |

## Common problems

| Symptom | Fix |
|---|---|
| `pkg-config` or linker errors building `aujar-ui` | `./scripts/setup-dev.sh`; `--check` lists what is missing |
| UI window does not open | `vulkaninfo --summary` (package `vulkan-tools`); see the checklist in [UI_SPIKE](UI_SPIKE.md) |
| `aujar-ui: already running` when starting the UI | the installed `aujar-ui.service` holds the lock: set `AUJAR_SOCKET` (the `dev:` tasks and `just dev` do) or `systemctl --user stop aujar-ui` |
| The daemon refuses to start because another one is running | the installed service owns the default socket: use the `dev:` tasks or stop the service |
| Shortcut does nothing | [TROUBLESHOOTING](TROUBLESHOOTING.md) |
| NexDesk sources missing | `git submodule update --init` (only `scripts/build-nexdesk.sh` needs them) |
| Editor and terminal `cargo` wait for each other | `.zed/settings.json` gives rust-analyzer its own target dir; if you use another editor, set `rust-analyzer.cargo.targetDir` to `true` |

## Dev container

`.devcontainer/devcontainer.json` builds an Ubuntu 24.04 environment and runs the setup script.
It suits building, `just check` and working on any crate. It has no desktop session, GPU,
portals or input devices, so run the launcher UI and Mouse Borders on your host.

## What is planned for the developer experience

Done: setup, faster builds, the daily loop (`just dev`, `just ci`, the Mouse Borders lab) and shared
logging with an IPC trace and `aujar doctor --dev`. Next: offline fixtures for the online launcher
providers, an integration test for daemon plus CLI (the lab smoke test is a first step), and git hooks
for commit messages and sign-off.
