#!/usr/bin/env bash
# Regenerates every app icon from icon.svg, which is the one source of truth.
#
# macOS is why this is a script rather than a single `tauri icon` call. Its icon
# grid insets the tile to 824 of a 1024 canvas and draws the system shadow in
# the margin, so a tile that bleeds to the edge sits about a fifth larger than
# every neighbour in the Dock. Windows and Linux have no such grid and draw what
# they are given, where the same margin would just make the icon small. One
# artwork, two framings: the .icns gets the margin, nothing else does.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
command -v rsvg-convert >/dev/null || { echo "need rsvg-convert (brew install librsvg)" >&2; exit 1; }
command -v magick >/dev/null || { echo "need magick (brew install imagemagick)" >&2; exit 1; }

# Every platform but macOS: the tile fills the canvas.
(cd "$root" && npx tauri icon "$here/icon.svg" >/dev/null)
rm -rf "$here/android" "$here/ios" # desktop-only app; tauri icon emits these unasked

# macOS: the same tile, inset per Apple's grid, the margin left transparent.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
rsvg-convert -w 824 -h 824 "$here/icon.svg" -o "$tmp/tile.png"
magick -size 1024x1024 xc:none "$tmp/tile.png" -gravity center -composite "$tmp/macos.png"
(cd "$root" && npx tauri icon "$tmp/macos.png" -o "$tmp/out" >/dev/null)
cp "$tmp/out/icon.icns" "$here/icon.icns"

# The dev server's favicon, so `npm run dev` is not branded Vite. Copied because
# public/ is served verbatim and cannot reach into src-tauri.
cp "$here/icon.svg" "$root/public/icon.svg"

echo "icons regenerated from icon.svg (.icns padded for the macOS grid, the rest full-bleed)"
