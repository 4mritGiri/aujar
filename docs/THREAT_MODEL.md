# Threat Model (initial)

**Assets:** user files (rename), clipboard contents, screen contents, window titles.

**Trust boundary:** the daemon runs as the user and listens on a Unix socket under
`$XDG_RUNTIME_DIR`. Anything that can connect can act as the user.

| Threat | Mitigation | Status |
|---|---|---|
| Other local users connect to the socket | Socket mode `0600` | ✅ |
| Other processes of the same user abuse the daemon | Peer-uid check ✅; per-client capability grants 📅 | 🚧 |
| Oversized / malformed IPC input | 64 KB request cap, JSON parsing, protocol version check | ✅ (5 s read timeout) |
| Client asks the daemon to run arbitrary commands | `Execute` accepts only indexed result ids; `Exec` parsed without a shell; field codes dropped | ✅ |
| Admin needs to restrict features | `/etc/aujar/policy.toml` denied capabilities, fail-closed parsing ([POLICY](POLICY.md)) | ✅ (user can run own daemon; see POLICY limits) |
| Rename escapes target directory | Names with `/`, `\0`, `.`, `..` rejected | ✅ |
| Rename partially applied after failure | Two-phase execution; full rollback | 🚧 |
| Clipboard history leaks secrets | Exclude password managers, encrypt at rest, opt-in | 📅 |
| Malicious plugin | Manifest capabilities, user approval, no in-process native plugins | 📅 |
| Supply chain | `cargo-deny`, Dependabot, locked builds, signed releases, SBOM | 🚧 |

Report vulnerabilities per [SECURITY.md](../SECURITY.md).
