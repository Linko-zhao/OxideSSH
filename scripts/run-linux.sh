#!/usr/bin/env bash
# Development launcher for OxideSSH on Linux (WSL2/WSLg).
#
# Three environment gaps exist on a stock Ubuntu WSL2 system:
#   1. libxkbcommon-x11 and libxcb-xkb are not installed, so the final binary
#      cannot link (undefined xkb_x11_* symbols) or start (missing .so.1).
#      This script extracts them from Ubuntu packages into a per-user cache.
#   2. The system lacks the -dev packages (libxkbcommon-dev, libxcb-dev, ...),
#      so the unversioned *.so symlinks the linker resolves (-lxkbcommon,
#      -lxcb, ...) do not exist. The script mirrors every system shared
#      library into the cache as an unversioned symlink.
#   3. No Secret Service provider (gnome-keyring) is installed or running, so
#      the OS credential store ("Remember password") cannot be used. The
#      script extracts gnome-keyring, prompts for a non-empty password, and
#      starts a single daemon with the default keyring created or unlocked.
#      The daemon keeps the keyring unlocked in memory for its lifetime;
#      later launches reuse a running daemon as-is (its unlock state is not
#      probed) and otherwise prompt again.
#   4. WSLg's Wayland compositor predates the xdg_wm_base version GPUI 0.2.2
#      requires (panic: UnsupportedVersion). WSLg's Xwayland serves :0 and
#      works, so the Wayland backend is disabled for this launch.
#
# Usage: scripts/run-linux.sh [cargo args...]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/oxide-ssh-linux-libs"
STAGE="$CACHE/root/usr/lib/x86_64-linux-gnu"
SYSTEM_LIBS=/usr/lib/x86_64-linux-gnu
KEYRING_DAEMON="$CACHE/root/usr/bin/gnome-keyring-daemon"

if [ ! -e "$STAGE/libxkbcommon-x11.so.0" ] || [ ! -e "$STAGE/libxcb-xkb.so.1" ]; then
    echo "Extracting missing GUI libraries into $STAGE" >&2
    mkdir -p "$STAGE"
    (
        cd "$CACHE"
        if ! apt-get download libxkbcommon-x11-0 libxkbcommon-x11-dev libxcb-xkb1; then
            echo "error: could not download libxkbcommon-x11/libxcb-xkb packages." >&2
            echo "       Run 'sudo apt-get update' once, or install them with" >&2
            echo "       'sudo apt-get install libxkbcommon-x11-dev libxcb-xkb1'." >&2
            exit 1
        fi
        for deb in libxkbcommon-x11-0_*.deb libxkbcommon-x11-dev_*.deb libxcb-xkb1_*.deb; do
            dpkg-deb -x "$deb" "$CACHE/root"
        done
        rm -f libxkbcommon-x11-0_*.deb libxkbcommon-x11-dev_*.deb libxcb-xkb1_*.deb
    )
fi

# Mirror every system shared library into the stage as the unversioned
# symlink the linker needs. Idempotent; keeps the stage complete even when
# the system gains libraries later.
if [ -d "$SYSTEM_LIBS" ]; then
    for lib in "$SYSTEM_LIBS"/lib*.so.*; do
        [ -e "$lib" ] || continue
        name="${lib##*/}"
        base="${name%%.so.*}"
        ln -sf "$lib" "$STAGE/$base.so"
    done
fi

export LIBRARY_PATH="$STAGE"
export LD_LIBRARY_PATH="$STAGE"

secrets_service_running() {
    dbus-send --session --dest=org.freedesktop.DBus --type=method_call \
        --print-reply /org/freedesktop/DBus org.freedesktop.DBus.NameHasOwner \
        string:org.freedesktop.secrets 2>/dev/null \
        | grep -q "boolean true"
}

ensure_keyring() {
    if secrets_service_running; then
        return 0
    fi
    if [ ! -x "$KEYRING_DAEMON" ]; then
        echo "Extracting gnome-keyring (Secret Service provider) into $CACHE" >&2
        if ! (
            cd "$CACHE"
            apt-get download gnome-keyring libgck-1-0 libgcr-base-3-1 gcr p11-kit pinentry-gnome3 \
                && for deb in gnome-keyring_*.deb libgck-1-0_*.deb libgcr-base-3-1_*.deb gcr_*.deb \
                    p11-kit_*.deb pinentry-gnome3_*.deb; do
                    dpkg-deb -x "$deb" "$CACHE/root"
                done \
                && rm -f gnome-keyring_*.deb libgck-1-0_*.deb libgcr-base-3-1_*.deb gcr_*.deb \
                    p11-kit_*.deb pinentry-gnome3_*.deb
        ); then
            echo "error: could not download/extract gnome-keyring packages." >&2
            echo "       Install them with 'sudo apt-get install gnome-keyring' and start the daemon." >&2
            return 1
        fi
    fi
    if [ ! -x "$KEYRING_DAEMON" ]; then
        echo "error: gnome-keyring-daemon is not available." >&2
        return 1
    fi
    if [ ! -t 0 ]; then
        echo "error: no Secret Service is running and there is no terminal to prompt" >&2
        echo "       for a keyring password. Start your keyring daemon first" >&2
        echo "       (e.g. 'gnome-keyring-daemon --unlock'), then run this script" >&2
        echo "       from an interactive terminal." >&2
        return 1
    fi
    if [ -f "$HOME/.local/share/keyrings/login.keyring" ]; then
        read -rsp "Keyring password for OxideSSH credentials: " KEYRING_PASSWORD
    else
        read -rsp "Choose a password to protect OxideSSH credentials: " KEYRING_PASSWORD
    fi
    echo >&2
    if [ -z "$KEYRING_PASSWORD" ]; then
        echo "error: an empty keyring password is not allowed." >&2
        return 1
    fi
    # --unlock starts a single daemon (if needed) and creates or unlocks the
    # default keyring with the given password, read from stdin (never argv).
    # Run it under setsid so the daemon survives this shell/pty exiting.
    if command -v setsid >/dev/null 2>&1; then
        printf '%s' "$KEYRING_PASSWORD" | setsid "$KEYRING_DAEMON" --unlock >/dev/null 2>&1 || true
    else
        printf '%s' "$KEYRING_PASSWORD" | "$KEYRING_DAEMON" --unlock >/dev/null 2>&1 || true
    fi
    unset KEYRING_PASSWORD
    # Wait (bounded) for the daemon to take ownership of the secrets name.
    for _ in $(seq 1 20); do
        if secrets_service_running; then
            return 0
        fi
        sleep 0.5
    done
    echo "error: the keyring daemon did not start; 'Remember password' needs a" >&2
    echo "       running Secret Service. Check the gnome-keyring setup above." >&2
    return 1
}

if ! ensure_keyring; then
    exit 1
fi

unset WAYLAND_DISPLAY

cd "$ROOT"
exec cargo run -p oxide-ssh-desktop --locked "$@"
