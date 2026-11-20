# Aujar development and packaging tasks
# Run `just --list` to see all available commands.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Default task
[default]
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --exclude aujar-ui --all-targets --all-features -- -D warnings
    cargo test --workspace --exclude aujar-ui

# Everything CI runs, locally: fmt, clippy, tests, MSRV check (when the toolchain is installed),
# cargo-deny (when installed) and the Mouse Borders crates. Slower than `check`; run before a PR.
ci: check msrv test-mouse-borders hooks-test _audit-if-installed

# The Mouse Borders crates also run on Windows and macOS in CI; this is their Linux test set.
test-mouse-borders:
    cargo test -p aujar-config -p aujar-mouse-borders -p aujar-mwb-cli -p aujar-mwb

# CI checks the workspace builds on the minimum supported Rust (see rust-version in Cargo.toml).
# Needs that toolchain once: `rustup toolchain install 1.85 --profile minimal`.
msrv:
    #!/usr/bin/env bash
    set -euo pipefail
    if rustup toolchain list | grep -q '^1\.85'; then
        cargo +1.85 check --workspace --exclude aujar-ui
    else
        echo "skipping MSRV check: run 'rustup toolchain install 1.85 --profile minimal' to enable it"
    fi

_audit-if-installed:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v cargo-deny >/dev/null; then
        cargo deny check
    else
        echo "skipping cargo-deny: not installed (scripts/setup-dev.sh installs it)"
    fi

# ---- daily loop ----

# Daemon + UI on a private socket, rebuilt and restarted when you save a file. Ctrl+C stops it.
# More switches (--no-watch, --hotkeys, ...): run ./scripts/dev.sh --help
dev:
    ./scripts/dev.sh

# Same, daemon only: for CLI, launcher-engine and Mouse Borders work (no GPU build).
dev-daemon:
    ./scripts/dev.sh --no-ui

# Same, with its own settings and state, so the UI never writes to your real ~/.config/aujar.
dev-isolated:
    ./scripts/dev.sh --isolate

# Same, with `define`, `weather`, `stock` ... answered from the recorded files in dev/fixtures
# instead of the network: deterministic, and it works without internet.
dev-offline:
    ./scripts/dev.sh --offline

# Record real answers of the online lookups into dev/fixtures (needs the network once).
# Your own queries:  just record-fixtures "define ephemeral" "weather oslo"
record-fixtures *QUERIES:
    ./scripts/record-fixtures.sh {{QUERIES}}

# Re-run `cargo check` for the platform crates on every save (fast feedback in a terminal).
watch:
    cargo watch -q -c -x 'check --workspace --exclude aujar-ui'

# Re-run the tests of one crate on every save:  just watch-test aujar-rename
watch-test crate:
    cargo watch -q -c -x 'test -p {{crate}}'

# Tests of one crate:  just test-crate aujar-rename
test-crate crate *ARGS:
    cargo test -p {{crate}} {{ARGS}}

# The test suite with cargo-nextest: parallel, readable failures (cargo install cargo-nextest).
nextest:
    cargo nextest run --workspace --exclude aujar-ui

# Is the installed or dev daemon healthy? Honors AUJAR_SOCKET (use it for the dev daemon).
doctor:
    cargo run -q -p aujar-launcher-app -- doctor
    cargo run -q -p aujar-launcher-app -- mwb doctor

# ---- git hooks ----

# Turn the git hooks on for this clone: Conventional Commits, DCO sign-off, formatting, clippy.
hooks:
    ./scripts/install-hooks.sh

hooks-off:
    ./scripts/install-hooks.sh --uninstall

# Test the hooks and the commit-message check in a throw-away repository (fast, no cargo).
hooks-test:
    ./scripts/test-hooks.sh

# ---- Mouse Borders lab (two daemons on this machine, paired over loopback) ----

# Start both daemons, pair them, show the status. See docs/DEVELOPMENT.md.
lab-up:
    ./scripts/mwb-lab.sh up

lab-status:
    ./scripts/mwb-lab.sh status

# Pair, connect, send a file, check it arrived, shut down: exit 1 on any failure.
lab-smoke:
    ./scripts/mwb-lab.sh smoke

lab-down:
    ./scripts/mwb-lab.sh down

# Run an aujar command as one of the lab machines:  just lab a mwb status
lab side *ARGS:
    ./scripts/mwb-lab.sh {{side}} {{ARGS}}

# ---- code quality ----

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

lint:
    cargo clippy --workspace --exclude aujar-ui --all-targets --all-features -- -D warnings

test:
    cargo test --workspace --exclude aujar-ui

check-ui:
    cargo clippy -p aujar-ui --all-targets -- -D warnings

test-ui:
    cargo test -p aujar-ui

audit:
    cargo deny check

# ---- setup ----

# Install system packages, Rust components and helper tools (asks before each step).
setup:
    ./scripts/setup-dev.sh

# Report what is missing in the development environment without changing anything.
setup-check:
    ./scripts/setup-dev.sh --check

# ---- development ----

# Remove only rust-analyzer's separate build directory (editor checks); your own builds stay.
clean-ra:
    rm -rf target/rust-analyzer

# Build everything but the UI and write an HTML timing report (target/cargo-timings/): shows
# which crates and which build steps (compiling, linking, build scripts) cost the most.
timings:
    cargo build --timings --workspace --exclude aujar-ui

# Disk used by the build directory, per profile (debug, debugging, release, rust-analyzer).
target-size:
    du -sh target target/* 2>/dev/null | sort -h

run-daemon:
    cargo run -p aujar-launcher-app -- daemon

run-ui:
    cargo run -p aujar-ui --release

# ---- builds ----

build:
    cargo build --workspace --exclude aujar-ui

build-ui:
    cargo build --release -p aujar-ui

build-daemon:
    cargo build --release -p aujar-launcher-app

# Separate Cargo invocations avoid conflicting async runtime features.
release:
    cargo build --release -p aujar-launcher-app
    cargo build --release -p aujar-ui

# ---- packaging ----

deb: release
    cargo deb -p aujar-launcher-app --no-build

rpm: release
    cargo generate-rpm -p apps/aujar-launcher

packages: release
    cargo deb -p aujar-launcher-app --no-build
    cargo generate-rpm -p apps/aujar-launcher

# ---- local installation ----

install-local:
    ./scripts/install-local.sh

uninstall-local:
    ./scripts/uninstall-local.sh

# ---- diagnostics ----

status:
    systemctl --user status aujar.service aujar-ui.service

logs:
    journalctl --user -u aujar.service -u aujar-ui.service -n 100 --no-pager

restart:
    systemctl --user restart aujar.service aujar-ui.service

# ---- cleanup ----

clean:
    cargo clean
