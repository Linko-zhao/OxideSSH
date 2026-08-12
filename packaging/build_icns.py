#!/usr/bin/env python3
"""Build an ICNS container from a complete macOS .iconset directory."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path


ICON_CHUNKS = (
    (b"icp4", "icon_16x16.png"),
    (b"icp5", "icon_32x32.png"),
    (b"icp6", "icon_32x32@2x.png"),
    (b"ic07", "icon_128x128.png"),
    (b"ic08", "icon_128x128@2x.png"),
    (b"ic09", "icon_256x256@2x.png"),
    (b"ic10", "icon_512x512@2x.png"),
)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("iconset", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()

    chunks = []
    for kind, filename in ICON_CHUNKS:
        payload = (args.iconset / filename).read_bytes()
        chunks.append(kind + struct.pack(">I", len(payload) + 8) + payload)

    body = b"".join(chunks)
    args.output.write_bytes(b"icns" + struct.pack(">I", len(body) + 8) + body)


if __name__ == "__main__":
    main()
