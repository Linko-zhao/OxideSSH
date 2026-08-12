#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
source_svg="$repo_root/assets/oxide-ssh-icon.svg"
output_dir="$repo_root/assets"
iconset=$(mktemp -d "${TMPDIR:-/tmp}/oxide-ssh.iconset.XXXXXX")
trap 'rm -rf "$iconset"' EXIT HUP INT TERM

if ! command -v magick >/dev/null 2>&1 && ! command -v convert >/dev/null 2>&1; then
    printf '%s\n' 'ImageMagick (magick or convert) is required.' >&2
    exit 1
fi
if command -v magick >/dev/null 2>&1; then
    image_tool=magick
else
    image_tool=convert
fi

render_png() {
    size=$1
    destination=$2
    "$image_tool" -background none -density 384 "$source_svg" -resize "${size}x${size}" -strip -define png:exclude-chunk=date,time "$destination"
}

render_png 1024 "$output_dir/oxide-ssh.png"
for size in 16 32 128 256 512; do
    render_png "$size" "$iconset/icon_${size}x${size}.png"
    doubled=$((size * 2))
    render_png "$doubled" "$iconset/icon_${size}x${size}@2x.png"
done

"$image_tool" \
    "$iconset/icon_16x16.png" \
    "$iconset/icon_32x32.png" \
    "$iconset/icon_32x32@2x.png" \
    "$iconset/icon_128x128.png" \
    "$iconset/icon_128x128@2x.png" \
    "$iconset/icon_256x256@2x.png" \
    "$output_dir/oxide-ssh.ico"

if command -v iconutil >/dev/null 2>&1; then
    mac_iconset="$iconset/OxideSSH.iconset"
    mkdir "$mac_iconset"
    for icon in "$iconset"/icon_*.png; do
        cp "$icon" "$mac_iconset/$(basename "$icon")"
    done
    iconutil --convert icns --output "$output_dir/oxide-ssh.icns" "$mac_iconset"
else
    python3 "$repo_root/packaging/build_icns.py" "$iconset" "$output_dir/oxide-ssh.icns"
fi
