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
| Hotkey portal prompts or rebinding abuse | Shortcut is only *requested*; the desktop owns confirmation. Policy can deny `global_hotkeys`. | ✅ |
| Rename escapes target directory | Names with `/`, `\0`, `.`, `..` rejected | ✅ |
| Rename partially applied after failure | Two-phase execution; full rollback | 🚧 |
| Clipboard history leaks secrets | Exclude password managers, encrypt at rest, opt-in | 📅 |
| Malicious plugin | Manifest capabilities, user approval, no in-process native plugins | 📅 |
| Another machine on the network connects and injects input | Noise `XXpsk3` handshake keyed by a generated 160-bit key; machines outside the layout refused; same-subnet filter before the handshake; off by default ([MOUSE_BORDERS](MOUSE_BORDERS.md)) | ✅ |
| Network attacker modifies, replays or floods Mouse Borders traffic | AEAD with strict message counter; unauthenticated frames capped at 512 B; 8 concurrent handshakes, 10 s timeout | ✅ |
| A paired machine misuses access (types, locks, sends files) and nobody notices | Audit log of connections, refusals, control hand-offs, transfers, locks and key changes; contents and keys never logged | ✅ detection, not prevention |
| Paired machine sends a hostile file | Single-component names, no overwrite, `.part` then SHA-256 check, 100 MB cap, never executable | ✅ |
| Local malware reads keystrokes through input devices | Requires the `input` group, which Mouse Borders needs. Mitigation: run only the least-privilege system service and keep your user out of the group | ⚠️ inherent to Linux input access |
| Supply chain | `cargo-deny`, Dependabot, locked builds, signed releases, SBOM | 🚧 |

Report vulnerabilities per [SECURITY.md](../SECURITY.md).
