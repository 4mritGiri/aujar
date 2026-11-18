# Contributing to Aujar

Thanks for helping! Please read this and the [Code of Conduct](CODE_OF_CONDUCT.md).

## Getting started

```bash
git clone --recurse-submodules https://github.com/4mritGiri/aujar && cd aujar
./scripts/setup-dev.sh          # system packages, Rust, just, cargo-deny, nextest, watch
just check                      # fmt + clippy + tests (the quick gate)
just ci                         # everything CI runs; run it before opening a PR
just dev                        # daemon + UI, rebuilt on every save
```

Requires Rust 1.85+ (edition 2024). [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) covers the editor
setup (Zed config is in `.zed/`), running a development copy next to an installed one,
debugging, logs and a map of the repository.

## Workflow

1. Open an issue (or comment on one) before large changes.
2. Branch from `main`, keep PRs focused.
3. Add tests; update docs and [FEATURE_MATRIX](docs/FEATURE_MATRIX.md) when behaviour changes.
4. Sign off commits (`git commit -s`) to certify the [DCO](https://developercertificate.org/).
5. Use [Conventional Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, `docs:` ...).

## Code guidelines

- Domain logic lives in `modules/` or `crates/`; UI/platform glue stays at the edge.
- Typed errors (`thiserror`) in libraries, `anyhow` in binaries.
- No `unwrap()` in non-test code; no unsafe without a justification comment.
- Anything that mutates the filesystem needs an explicit-intent API and tests.
- Compositor-specific code goes behind a trait and a capability report.

## Testing across sessions

Please state which distro, session type (X11/Wayland) and compositor you tested on.

## Security

Do not file public issues for vulnerabilities; see [SECURITY.md](SECURITY.md).
