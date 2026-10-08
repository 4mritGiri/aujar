# Contributing

Keep domain logic in modules/crates and keep UI/platform-specific code at the boundary. Prefer small public APIs, typed errors, deterministic tests, and reusable services over copy-pasted utility implementations.

Before submitting a change:

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
```
