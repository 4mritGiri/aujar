# Packaging and Installation

Aujar ships a small CLI + daemon (`aujar`). Every package installs:

| File | Destination |
|---|---|
| `aujar` binary | `/usr/bin/aujar` |
| `packaging/systemd/aujar.service` | `/usr/lib/systemd/user/aujar.service` |
| `packaging/desktop/dev.aujar.Aujar.desktop` | `/usr/share/applications/dev.aujar.Aujar.desktop` (file name must equal the portal app id, see [HOTKEYS](HOTKEYS.md)) |
| `config/policy.example.toml` | `/usr/share/doc/aujar/policy.example.toml` |
| `LICENSE` | `/usr/share/licenses/aujar/` (or `/usr/share/doc/aujar/`) |

NexDesk (optional) is built and packaged separately from `integrations/nexdesk`; install `nexdesk` and
`nexdesk-rdp` together, beside `aujar`, so `aujar nexdesk open` finds them (see [NEXDESK](NEXDESK.md)).

The GPUI launcher UI (`apps/aujar-ui`) is still a spike and is **not** packaged yet.

## Installing

After installing any package, enable the per-user daemon (no root needed):

```bash
systemctl --user enable --now aujar
```

| Distro family | Command |
|---|---|
| Debian, Ubuntu, Mint, Pop!_OS | `sudo apt install ./aujar_<version>_<arch>.deb` |
| Fedora, RHEL-likes | `sudo dnf install ./aujar-<version>.<arch>.rpm` |
| openSUSE | `sudo zypper install ./aujar-<version>.<arch>.rpm` |
| Arch, Manjaro | build `packaging/arch/PKGBUILD` with `makepkg -si` (AUR package planned) |
| Any distro (tarball) | extract, then run `./install-local.sh` for a per-user install |
| From source | `scripts/install-local.sh` (builds and installs under `~/.local`); undo with `scripts/uninstall-local.sh` |

Release assets (built for `x86_64` and `aarch64`) and `SHA256SUMS` are attached to each GitHub
release: `.deb`, `.rpm` and a `.tar.gz`.

## Building packages yourself

```bash
cargo install cargo-deb cargo-generate-rpm --locked
just packages            # or: just deb / just rpm
# outputs: target/debian/*.deb and target/generate-rpm/*.rpm
```

Metadata lives in `apps/aujar-launcher/Cargo.toml` (`[package.metadata.deb]` and
`[package.metadata.generate-rpm]`).

## Automation

- Every PR builds the `.deb` and `.rpm`, lists their contents, and installs the `.deb` on Ubuntu
  (`.github/workflows/ci.yml`, job `package`).
- Pushing a tag `vX.Y.Z` (it must match the crate version) builds both architectures and
  publishes a GitHub release (`.github/workflows/release.yml`).

## Status

| Format | Status | Notes |
|---|---|---|
| `.deb` (x86_64, aarch64) | ✅ configured | `cargo-deb`; not yet verified on a published release |
| `.rpm` (x86_64, aarch64) | ✅ configured | `cargo-generate-rpm`; asset-path handling to be confirmed on the first CI run |
| Tarball + `SHA256SUMS` | ✅ configured | |
| Arch `PKGBUILD` | ✅ provided | replace `sha256sums`; AUR submission 📅 |
| Flatpak | 📅 | needs the UI; daemon-to-host permissions to be designed |
| AppImage | 📅 | needs the UI |
| Nix flake | 📅 | |
| Signed releases, SBOM, provenance attestations | 📅 | v1.0 |

## Notes for packagers

- The systemd unit is a **user** unit started with the graphical session; it is not enabled
  automatically by packages. Consider a systemd user preset if your distro policy allows it.
- Replace the placeholder maintainer address in `Cargo.toml` before submitting to a distro archive.
- `Cargo.lock` must be committed; release builds use `--locked`.
