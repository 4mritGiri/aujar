#!/usr/bin/env bash
# Set up a machine for developing Aujar.
#
#   scripts/setup-dev.sh              # interactive: asks before each install step
#   scripts/setup-dev.sh --yes        # no prompts (CI, dev containers)
#   scripts/setup-dev.sh --dry-run    # print what would be run, change nothing
#   scripts/setup-dev.sh --check      # only report what is missing (exit 1 if required tools are)
#
# Options:
#   -y, --yes             do not ask for confirmation
#   -n, --dry-run         print commands instead of running them
#       --check           verify the environment and exit
#       --no-system       skip distro packages (build headers for the UI, curl, git, ...)
#       --no-rust         skip the Rust toolchain step
#       --no-cargo-tools  skip just / cargo-deny / cargo-nextest / cargo-watch
#       --no-submodule    skip fetching the NexDesk submodule
#       --no-hooks        skip turning on the git hooks (Conventional Commits, DCO sign-off,
#                         formatting; see scripts/install-hooks.sh)
#       --optional        also install runtime helpers the launcher can use
#                         (wmctrl, xclip, wl-clipboard, ffmpegthumbnailer, poppler)
#       --fast-linker     also install the mold linker (much faster linking of the big
#                         binaries) and print how to enable it for your user
#   -h, --help            show this help
#
# Supported package managers: apt (Debian/Ubuntu), dnf (Fedora), pacman (Arch). On anything
# else the script prints the packages you need. Safe to re-run: it only installs what is missing.
set -euo pipefail

cd "$(dirname "$0")/.."

MIN_RUST_MINOR=85 # rust-version = "1.85" in Cargo.toml (edition 2024)

YES=0
DRY_RUN=0
CHECK_ONLY=0
DO_SYSTEM=1
DO_RUST=1
DO_CARGO_TOOLS=1
DO_SUBMODULE=1
DO_HOOKS=1
DO_OPTIONAL=0
DO_FAST_LINKER=0

for arg in "$@"; do
    case "$arg" in
        -y | --yes) YES=1 ;;
        -n | --dry-run) DRY_RUN=1 ;;
        --check) CHECK_ONLY=1 ;;
        --no-system) DO_SYSTEM=0 ;;
        --no-rust) DO_RUST=0 ;;
        --no-cargo-tools) DO_CARGO_TOOLS=0 ;;
        --no-submodule) DO_SUBMODULE=0 ;;
        --no-hooks) DO_HOOKS=0 ;;
        --optional) DO_OPTIONAL=1 ;;
        --fast-linker) DO_FAST_LINKER=1 ;;
        -h | --help)
            sed -n '2,26p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "unknown option: $arg (try --help)" >&2
            exit 2
            ;;
    esac
done

# ---------------------------------------------------------------- helpers

if [ -t 1 ]; then
    BOLD=$'\033[1m'
    GREEN=$'\033[32m'
    RED=$'\033[31m'
    YELLOW=$'\033[33m'
    RESET=$'\033[0m'
else
    BOLD="" GREEN="" RED="" YELLOW="" RESET=""
fi

step() { printf '\n%s==> %s%s\n' "$BOLD" "$*" "$RESET"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '    %s!%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }

have() { command -v "$1" >/dev/null 2>&1; }

# Run a command, or only print it with --dry-run.
run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '    [dry-run] %s\n' "$*"
    else
        printf '    $ %s\n' "$*"
        "$@"
    fi
}

# Ask a yes/no question. --yes answers yes; with no terminal the answer is no.
confirm() {
    if [ "$YES" -eq 1 ] || [ "$DRY_RUN" -eq 1 ]; then
        return 0
    fi
    if [ ! -t 0 ]; then
        warn "not a terminal; skipping: $1 (pass --yes to accept all steps)"
        return 1
    fi
    local reply
    read -r -p "    $1 [y/N] " reply
    [ "$reply" = "y" ] || [ "$reply" = "Y" ]
}

OS="$(uname -s)"
SUDO=""
if [ "$(id -u)" -ne 0 ]; then
    if have sudo; then
        SUDO="sudo"
    else
        SUDO="__no_sudo__"
    fi
fi

package_manager() {
    [ "$OS" = "Linux" ] || {
        echo none
        return
    }
    if have apt-get; then
        echo apt
    elif have dnf; then
        echo dnf
    elif have pacman; then
        echo pacman
    else
        echo unknown
    fi
}
PM="$(package_manager)"

# Build requirements. The GPUI UI needs windowing/GPU headers; the platform crates only need a
# C toolchain. clang is needed by bindgen-based crates in the UI's dependency tree.
system_packages() {
    case "$PM" in
        apt)
            echo build-essential pkg-config clang curl git \
                libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
                libx11-xcb-dev libxcb1-dev libvulkan-dev mesa-vulkan-drivers \
                libfontconfig-dev
            ;;
        dnf)
            echo gcc gcc-c++ make pkgconf-pkg-config clang curl git \
                libxkbcommon-devel libxkbcommon-x11-devel wayland-devel \
                libX11-devel libxcb-devel vulkan-loader-devel mesa-vulkan-drivers \
                fontconfig-devel
            ;;
        pacman)
            echo base-devel pkgconf clang curl git \
                libxkbcommon libxkbcommon-x11 wayland libx11 libxcb \
                vulkan-icd-loader vulkan-headers mesa fontconfig
            ;;
    esac
}

# Programs the launcher and clipboard use at runtime when present (see docs/LOOKUPS_AND_WINDOWS.md).
optional_packages() {
    case "$PM" in
        apt | dnf) echo wmctrl xclip wl-clipboard ffmpegthumbnailer poppler-utils ;;
        pacman) echo wmctrl xclip wl-clipboard ffmpegthumbnailer poppler ;;
    esac
}

# pkg-config modules that the UI build looks for.
PKGCONFIG_MODULES=(xkbcommon xkbcommon-x11 wayland-client xcb x11-xcb fontconfig vulkan)

rust_minor() {
    # "rustc 1.97.0 (...)" -> 97
    rustc --version 2>/dev/null | sed -n 's/^rustc 1\.\([0-9]*\)\..*/\1/p'
}

# ---------------------------------------------------------------- check

FAILED_REQUIRED=0

ok() { printf '    %s✓%s %s\n' "$GREEN" "$RESET" "$*"; }
missing_required() {
    printf '    %s✗%s %s\n' "$RED" "$RESET" "$*"
    FAILED_REQUIRED=$((FAILED_REQUIRED + 1))
}
missing_optional() { printf '    %s~%s %s\n' "$YELLOW" "$RESET" "$*"; }

check_environment() {
    step "Checking the environment"

    if have rustc && have cargo; then
        local minor
        minor="$(rust_minor)"
        if [ -n "$minor" ] && [ "$minor" -ge "$MIN_RUST_MINOR" ]; then
            ok "Rust $(rustc --version | cut -d' ' -f2)"
        else
            missing_required "Rust is older than 1.$MIN_RUST_MINOR (run: rustup update stable)"
        fi
        if cargo fmt --version >/dev/null 2>&1; then ok "rustfmt"; else missing_required "rustfmt (rustup component add rustfmt)"; fi
        if cargo clippy --version >/dev/null 2>&1; then ok "clippy"; else missing_required "clippy (rustup component add clippy)"; fi
    else
        missing_required "Rust toolchain (https://rustup.rs)"
    fi

    if have just; then ok "just"; else missing_required "just (task runner; used by the justfile)"; fi
    if have cargo-deny; then ok "cargo-deny"; else missing_optional "cargo-deny (just audit)"; fi
    if have cargo-nextest; then ok "cargo-nextest"; else missing_optional "cargo-nextest (faster test runner)"; fi
    if have cargo-watch; then ok "cargo-watch"; else missing_optional "cargo-watch (rebuild on save)"; fi

    if have git; then ok "git"; else missing_required "git"; fi
    if have mold; then ok "mold (fast linker)"; else missing_optional "mold (fast linker; scripts/setup-dev.sh --fast-linker)"; fi
    if have curl; then ok "curl (online launcher lookups)"; else missing_optional "curl (online launcher lookups)"; fi
    if have cc || have gcc; then ok "C compiler"; else missing_required "C compiler (build-essential / gcc)"; fi

    if [ "$OS" = "Linux" ]; then
        if have pkg-config; then
            local mod missing_modules=()
            for mod in "${PKGCONFIG_MODULES[@]}"; do
                pkg-config --exists "$mod" 2>/dev/null || missing_modules+=("$mod")
            done
            if [ "${#missing_modules[@]}" -eq 0 ]; then
                ok "UI build headers (${PKGCONFIG_MODULES[*]})"
            else
                # Only the UI needs these; the platform crates build without them.
                missing_optional "UI build headers missing: ${missing_modules[*]} (only needed for aujar-ui)"
            fi
        else
            missing_required "pkg-config"
        fi
        if have vulkaninfo; then ok "vulkaninfo (GPU diagnostics)"; else missing_optional "vulkaninfo (vulkan-tools; handy when the UI will not start)"; fi
    fi

    if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        if [ "$(git config --local --get core.hooksPath || true)" = ".githooks" ]; then
            ok "git hooks (Conventional Commits, DCO sign-off, formatting)"
        else
            missing_optional "git hooks are off (scripts/install-hooks.sh)"
        fi
    fi

    if [ -f integrations/nexdesk/Cargo.toml ]; then
        ok "NexDesk sources (integrations/nexdesk)"
    else
        missing_optional "NexDesk sources missing (git submodule update --init; only needed for NexDesk)"
    fi
}

if [ "$CHECK_ONLY" -eq 1 ]; then
    check_environment
    echo
    if [ "$FAILED_REQUIRED" -gt 0 ]; then
        echo "${RED}$FAILED_REQUIRED required item(s) missing.${RESET} Run scripts/setup-dev.sh to fix."
        exit 1
    fi
    echo "${GREEN}Environment is ready.${RESET}"
    exit 0
fi

# ---------------------------------------------------------------- system packages

install_system_packages() {
    step "System packages ($PM)"
    if [ "$PM" = "none" ] || [ "$PM" = "unknown" ]; then
        warn "No supported package manager (apt, dnf, pacman) found on this system."
        info "Install by hand: a C compiler, pkg-config, clang, curl, git and the development"
        info "packages for xkbcommon, xkbcommon-x11, wayland, xcb, x11, vulkan and fontconfig."
        return 0
    fi
    if [ "$SUDO" = "__no_sudo__" ]; then
        warn "Not root and sudo is missing; install these packages as root:"
        info "$(system_packages)"
        return 0
    fi

    local packages
    # shellcheck disable=SC2207
    packages=($(system_packages))
    if [ "$DO_OPTIONAL" -eq 1 ]; then
        # shellcheck disable=SC2207
        packages+=($(optional_packages))
    fi
    info "packages: ${packages[*]}"
    confirm "Install them with $PM?" || {
        info "skipped"
        return 0
    }

    # $SUDO is empty for root, so it is deliberately unquoted.
    case "$PM" in
        apt)
            run $SUDO apt-get update
            run $SUDO apt-get install -y "${packages[@]}"
            ;;
        dnf) run $SUDO dnf install -y "${packages[@]}" ;;
        pacman) run $SUDO pacman -S --needed --noconfirm "${packages[@]}" ;;
    esac
}

# ---------------------------------------------------------------- rust

install_rust() {
    step "Rust toolchain"
    if ! have rustup; then
        if have cargo; then
            warn "cargo is installed without rustup (a distro Rust?). Aujar needs Rust 1.$MIN_RUST_MINOR+"
            warn "with rustfmt and clippy. rustup is recommended: https://rustup.rs"
            return 0
        fi
        info "Rust is not installed."
        if confirm "Install rustup now (downloads and runs https://sh.rustup.rs)?"; then
            run bash -c "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"
            # shellcheck disable=SC1091
            [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
        else
            info "skipped. Install it from https://rustup.rs and re-run this script."
            return 0
        fi
    fi

    # rust-toolchain.toml pins "stable" with rustfmt + clippy; rust-src helps editors navigate std.
    if have rustup; then
        run rustup toolchain install stable --profile minimal \
            --component rustfmt --component clippy --component rust-src
    fi
}

# ---------------------------------------------------------------- cargo tools

install_cargo_tool() {
    local crate="$1" bin="$2"
    if have "$bin"; then
        info "$bin already installed"
        return 0
    fi
    if ! have cargo && [ "$DRY_RUN" -eq 0 ]; then
        warn "cargo not available; cannot install $crate"
        return 0
    fi
    if have cargo-binstall; then
        run cargo binstall -y --locked "$crate"
    else
        run cargo install --locked "$crate"
    fi
}

install_cargo_tools() {
    step "Cargo tools (just, cargo-deny, cargo-nextest, cargo-watch)"
    if ! have cargo-binstall; then
        info "tip: 'cargo install cargo-binstall' first and these install as prebuilt binaries"
        info "     in seconds instead of compiling for several minutes."
    fi
    confirm "Install the missing ones?" || {
        info "skipped"
        return 0
    }
    install_cargo_tool just just
    install_cargo_tool cargo-deny cargo-deny
    install_cargo_tool cargo-nextest cargo-nextest
    install_cargo_tool cargo-watch cargo-watch
}

# ---------------------------------------------------------------- submodule

fetch_submodule() {
    step "NexDesk submodule"
    if [ -f integrations/nexdesk/Cargo.toml ]; then
        info "sources already present"
        return 0
    fi
    if [ -d .git ] && [ -f .gitmodules ]; then
        confirm "Fetch the NexDesk submodule (large)?" || {
            info "skipped"
            return 0
        }
        run git submodule update --init integrations/nexdesk
    else
        info "not a git checkout (or no .gitmodules): skipping. Aujar builds without NexDesk;"
        info "only 'scripts/build-nexdesk.sh' and the NexDesk workflow need it."
    fi
}

# ---------------------------------------------------------------- git hooks

install_git_hooks() {
    step "Git hooks"
    if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
        info "not a git checkout (a zip download has no .git): skipping"
        return 0
    fi

    local current
    current="$(git config --local --get core.hooksPath || true)"

    if [ "$current" = ".githooks" ]; then
        info "already on"
        return 0
    fi

    if [ -n "$current" ]; then
        warn "core.hooksPath is '$current' (another hook manager?): leaving it alone."
        info "To use Aujar's hooks instead: scripts/install-hooks.sh --force"
        return 0
    fi

    info "commit-msg: Conventional Commits + the DCO sign-off (git commit -s)"
    info "pre-commit: no build output or big files in commits; Rust formatted"
    info "pre-push:   cargo clippy, the gate CI runs"
    confirm "Turn the hooks on for this clone?" || {
        info "skipped (later: scripts/install-hooks.sh)"
        return 0
    }

    run ./scripts/install-hooks.sh
}

# ---------------------------------------------------------------- fast linker

install_fast_linker() {
    step "Fast linker (mold)"
    if [ "$PM" = "none" ] || [ "$PM" = "unknown" ]; then
        warn "Install mold and clang with your package manager (https://github.com/rui314/mold)."
    elif [ "$SUDO" = "__no_sudo__" ]; then
        warn "Not root and sudo is missing; install the packages 'mold' and 'clang' as root."
    elif have mold; then
        info "mold already installed"
    elif confirm "Install mold with $PM?"; then
        # $SUDO is empty for root, so it is deliberately unquoted.
        case "$PM" in
            apt) run $SUDO apt-get install -y mold clang ;;
            dnf) run $SUDO dnf install -y mold clang ;;
            pacman) run $SUDO pacman -S --needed --noconfirm mold clang ;;
        esac
    else
        info "skipped"
    fi
    info ""
    info "Enable it for your user (not committed to the repository, so teammates and CI"
    info "without mold are unaffected). Add this to ~/.cargo/config.toml:"
    info ""
    info "    [target.x86_64-unknown-linux-gnu]"
    info "    linker = \"clang\""
    info "    rustflags = [\"-C\", \"link-arg=-fuse-ld=mold\"]"
    info ""
    info "(aarch64 machines: use the target name aarch64-unknown-linux-gnu.)"
}

# ---------------------------------------------------------------- run

[ "$DRY_RUN" -eq 1 ] && info "dry run: nothing will be changed"

[ "$DO_SYSTEM" -eq 1 ] && install_system_packages
[ "$DO_RUST" -eq 1 ] && install_rust
[ "$DO_CARGO_TOOLS" -eq 1 ] && install_cargo_tools
[ "$DO_SUBMODULE" -eq 1 ] && fetch_submodule
[ "$DO_HOOKS" -eq 1 ] && install_git_hooks
[ "$DO_FAST_LINKER" -eq 1 ] && install_fast_linker

if [ "$DRY_RUN" -eq 0 ]; then
    check_environment
    echo
    if [ "$FAILED_REQUIRED" -gt 0 ]; then
        echo "${RED}$FAILED_REQUIRED required item(s) still missing${RESET} (see above)."
        exit 1
    fi
    echo "${GREEN}Ready.${RESET} Next:"
    echo "    just check         # fmt + clippy + tests, the same gate CI uses"
    echo "    cargo run -p aujar-launcher-app -- --help"
    echo "See docs/DEVELOPMENT.md (Zed tasks, debugging, crate map)."
fi
