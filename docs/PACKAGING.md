# Packaging

Artifacts installed by every package:

| File | Destination |
|---|---|
| `nexora` binary | `/usr/bin/nexora` |
| `packaging/systemd/nexora.service` | `/usr/lib/systemd/user/nexora.service` |
| `packaging/desktop/nexora.desktop` | `/usr/share/applications/nexora.desktop` |
| `LICENSE` | `/usr/share/licenses/nexora/` |

The systemd unit's `ExecStart` is `%h/.local/bin/nexora daemon` for source installs; system
packages must override it to `/usr/bin/nexora daemon`.

| Format | Status | Plan |
|---|---|---|
| Source tarball + `SHA256SUMS` | ✅ release workflow | |
| `.deb` | 📅 | `cargo-deb` |
| `.rpm` | 📅 | `cargo-generate-rpm` |
| AUR | 📅 | `nexora-git`, `nexora` |
| Flatpak | 📅 | Daemon talks to the host via portals; window management needs `--talk-name` permissions |
| AppImage | 📅 | |
| Nix flake | 📅 | |

Packagers: see [SECURITY](../SECURITY.md) for how releases will be signed.
