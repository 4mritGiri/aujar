# Aujar development and packaging tasks
# Run `just --list` to see all available commands.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Default task
[default]
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --exclude aujar-ui --all-targets --all-features -- -D warnings
    cargo test --workspace --exclude aujar-ui

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

# ---- development ----

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
