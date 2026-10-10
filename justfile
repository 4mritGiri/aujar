default: check

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace

fmt:
    cargo fmt --all

audit:
    cargo deny check

run-daemon:
    cargo run -p aujar-launcher-app -- daemon

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
