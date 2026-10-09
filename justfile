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
