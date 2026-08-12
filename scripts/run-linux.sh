#!/usr/bin/env bash
# Development launcher for OxideSSH on Linux (WSL2/WSLg).
#
# Two environment gaps exist on a stock Ubuntu WSL2 system:
#   1. libxkbcommon-x11 and libxcb-xkb are not installed, so the final binary
#      cannot link (undefined xkb_x11_* symbols) or start (missing .so.1).
#      This script extracts them from Ubuntu packages into a per-user cache.
#   2. WSLg's Wayland compositor predates the xdg_wm_base version GPUI 0.2.2
#      requires (panic: UnsupportedVersion). WSLg's Xwayland serves :0 and
#      works, so the Wayland backend is disabled for this launch.
#
# Usage: scripts/run-linux.sh [cargo args...]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/oxide-ssh-linux-libs"
STAGE="$CACHE/root/usr/lib/x86_64-linux-gnu"

if [ ! -e "$STAGE/libxkbcommon-x11.so.0" ] || [ ! -e "$STAGE/libxcb-xkb.so.1" ]; then
    echo "Extracting missing GUI libraries into $STAGE" >&2
    mkdir -p "$CACHE"
    (
        cd "$CACHE"
        apt-get download libxkbcommon-x11-0 libxkbcommon-x11-dev libxcb-xkb1
        for deb in libxkbcommon-x11-0_*.deb libxkbcommon-x11-dev_*.deb libxcb-xkb1_*.deb; do
            dpkg-deb -x "$deb" "$CACHE/root"
        done
        rm -f libxkbcommon-x11-0_*.deb libxkbcommon-x11-dev_*.deb libxcb-xkb1_*.deb
    )
fi

export LIBRARY_PATH="$STAGE"
export LD_LIBRARY_PATH="$STAGE"
unset WAYLAND_DISPLAY

cd "$ROOT"
exec cargo run -p oxide-ssh-desktop "$@"
