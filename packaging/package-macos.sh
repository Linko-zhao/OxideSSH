#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

# No signing identity or notarization credentials are consumed: these outputs are intentionally unsigned.
unset APPLE_SIGNING_IDENTITY APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_API_ISSUER APPLE_API_KEY APPLE_API_KEY_PATH

target=aarch64-apple-darwin
cargo build -p oxide-ssh-desktop --release --target "$target" --locked
cargo packager --release --target "$target" --formats app,dmg
