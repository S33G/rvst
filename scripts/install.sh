#!/bin/sh
set -eu

REPOSITORY="https://github.com/S33G/rvst.git"
BRANCH="main"
GUIDE="https://github.com/S33G/rvst/blob/main/docs/wsl.md"

if [ -t 1 ]; then
    AMBER='\033[38;5;214m'
    MUTED='\033[38;5;244m'
    RESET='\033[0m'
else
    AMBER=''
    MUTED=''
    RESET=''
fi

printf '%b' "$AMBER"
cat <<'BANNER'
 ____  __     ______________
|  _ \ \ \   / / ___|_   _|
| |_) | \ \ / /\___ \ | |
|  _ <   \ V /  ___) || |
|_| \_\   \_/  |____/ |_|
BANNER
printf '%b\n' "$RESET"
printf '%bRust audio tools. One editor. Your plugins.%b\n\n' "$MUTED" "$RESET"

is_wsl() {
    [ -n "${WSL_DISTRO_NAME:-}" ] || {
        [ -r /proc/sys/kernel/osrelease ] &&
            grep -qi microsoft /proc/sys/kernel/osrelease
    }
}

ask() {
    prompt=$1
    default=$2
    if [ ! -r /dev/tty ]; then
        printf '%s\n' "No interactive terminal found. Re-run this installer in a terminal." >&2
        exit 2
    fi
    printf '%s' "$prompt" >/dev/tty
    IFS= read -r answer </dev/tty || answer=
    if [ -z "$answer" ]; then
        answer=$default
    fi
    case "$answer" in
        y|Y|yes|YES|Yes) return 0 ;;
        *) return 1 ;;
    esac
}

if is_wsl; then
    distro=${WSL_DISTRO_NAME:-WSL}
    printf 'WSL detected: %s\n' "$distro"
    cat <<EOF
This installs Linux command-line tools inside this WSL distro. It does not
install a Windows executable or put audio plugins into a Windows DAW.
WSL setup notes: $GUIDE

EOF
    if ! ask "Set up RVST in this WSL distro now? [y/N] " "n"; then
        printf '\nNo changes made. Read the guide above, then re-run this command.\n'
        exit 0
    fi
else
    printf 'This wizard installs the RVST command-line tools with Cargo.\n'
    printf 'It does not install audio plugins, a DAW, or change your shell profile.\n\n'
    if ! ask "Install RVST now? [Y/n] " "y"; then
        printf '\nNo changes made.\n'
        exit 0
    fi
fi

if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
    cat >&2 <<'EOF'
Rust and Cargo are required to build RVST. Install Rust with rustup:
https://rustup.rs/
Then open a new terminal and run this installer again.
EOF
    exit 1
fi

cargo_home=${CARGO_HOME:-"${HOME:?HOME is not set}/.cargo"}
bin_dir="$cargo_home/bin"
printf '\nRust toolchain: '
rustc --version
printf 'Install location: %s\n' "$bin_dir"
printf 'Source: %s (branch %s)\n\n' "$REPOSITORY" "$BRANCH"

cargo install --git "$REPOSITORY" --branch "$BRANCH" --locked --force cargo-rvst

printf '\nRVST is installed.\n'
case ":${PATH:-}:" in
    *":$bin_dir:"*) printf 'Cargo bin directory is on PATH.\n' ;;
    *)
        printf 'Add this directory to PATH to run rvst from any terminal:\n  %s\n' "$bin_dir"
        printf 'For this terminal: export PATH="%s:$PATH"\n' "$bin_dir"
        ;;
esac

if [ -x "$bin_dir/rvst" ]; then
    "$bin_dir/rvst" --version
fi
printf 'Next: rvst --help\n'
