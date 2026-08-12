#!/usr/bin/env bash
# Development launcher for OxideSSH on Linux (WSL2/WSLg).
#
# Three environment gaps exist on a stock Ubuntu WSL2 system:
#   1. libxkbcommon-x11 and libxcb-xkb are not installed at all. This script
#      extracts them from Ubuntu packages into a per-user cache.
#   2. The system lacks the -dev packages (libxkbcommon-dev, libxcb-dev, ...),
#      so the unversioned *.so symlinks the linker resolves (-lxkbcommon,
#      -lxcb, ...) do not exist. The script mirrors every system shared
#      library into the cache as an unversioned symlink.
#   3. WSLg's Wayland compositor predates the xdg_wm_base version GPUI 0.2.2
#      requires (panic: UnsupportedVersion). WSLg's Xwayland serves :0 and
#      works, so the Wayland backend is disabled for this launch.
#
# Usage: scripts/run-linux.sh [cargo args...]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/oxide-ssh-linux-libs"
STAGE="$CACHE/root/usr/lib/x86_64-linux-gnu"
SYSTEM_LIBS=/usr/lib/x86_64-linux-gnu

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
unset WAYLAND_DISPLAY

cd "$ROOT"
exec cargo run -p oxide-ssh-desktop --locked "$@"
