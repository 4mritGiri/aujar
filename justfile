default: check

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --exclude aujar-ui --all-targets --all-features -- -D warnings
    cargo test --workspace --exclude aujar-ui

# Needs the GPU/windowing headers listed in docs/UI_SPIKE.md.
check-ui:
    cargo clippy -p aujar-ui --all-targets -- -D warnings

fmt:
    cargo fmt --all

audit:
    cargo deny check

run-daemon:
    cargo run -p aujar-launcher-app -- daemon

run-ui:
    cargo run -p aujar-ui --release

# ---- packaging (needs: cargo install cargo-deb cargo-generate-rpm) ----
release:
    cargo build --release -p aujar-launcher-app

deb: release
    cargo deb -p aujar-launcher-app --no-build

rpm: release
    cargo generate-rpm -p apps/aujar-launcher

packages: deb rpm

install-local:
    ./scripts/install-local.sh

uninstall-local:
    ./scripts/uninstall-local.sh
