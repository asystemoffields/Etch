#!/usr/bin/env bash
# Regenerate every app icon from assets/app-icon/wordcraft.svg.
#
# Needs: resvg (brew install resvg / cargo install resvg). On macOS, iconutil writes the
# .icns. The outputs are committed, so builds and packaging never need these tools.
#
#   packaging/icons.sh
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/assets/app-icon"
SVG="$DIR/wordcraft.svg"
# Source file name of the hicolor icons. Packaging installs them under the app id
# (io.github.asystemoffields.Etch) whatever they are called here: packaging/linux/package.sh and
# the Flatpak manifest loop over hicolor/*/apps/*.{png,svg}.
ID="ai.storyteller.wordcraft"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

command -v resvg >/dev/null || { echo "error: resvg not found (brew install resvg)" >&2; exit 1; }

# The artwork is a full-bleed 512 tile (rx=112). Windows and Linux use it as is. macOS icons sit on
# Apple's grid: an 824/1024 body with a transparent margin, so widen the viewBox to 512*1024/824.
MAC="$TMP/mac.svg"
sed 's/viewBox="0 0 512 512"/viewBox="-62.136 -62.136 636.272 636.272"/' "$SVG" >"$MAC"
grep -q 'viewBox="-62.136' "$MAC" || { echo "error: expected viewBox=\"0 0 512 512\" in $SVG" >&2; exit 1; }

render() { resvg -w "$2" -h "$2" "$1" "$3" </dev/null; }

render "$SVG" 1024 "$DIR/wordcraft-1024.png"
# Runtime window/Dock icon on macOS (embedded by apps/wordcraft); other platforms use hicolor 256.
render "$MAC" 512 "$DIR/wordcraft-macos-512.png"

# Linux hicolor theme.
for s in 16 24 32 48 64 128 256 512; do
  mkdir -p "$DIR/hicolor/${s}x${s}/apps"
  render "$SVG" "$s" "$DIR/hicolor/${s}x${s}/apps/$ID.png"
done
mkdir -p "$DIR/hicolor/scalable/apps"
cp "$SVG" "$DIR/hicolor/scalable/apps/$ID.svg"

# Windows .ico.
ICO_PNGS=()
for s in 16 20 24 32 40 48 64 128 256; do
  render "$SVG" "$s" "$TMP/ico-$s.png"
  ICO_PNGS+=("$TMP/ico-$s.png")
done
(cd "$ROOT" && cargo run -q -p xtask -- ico "$DIR/wordcraft.ico" "${ICO_PNGS[@]}")

# macOS .icns.
if command -v iconutil >/dev/null; then
  SET="$TMP/wordcraft.iconset"
  mkdir -p "$SET"
  for s in 16 32 128 256 512; do
    render "$MAC" "$s" "$SET/icon_${s}x${s}.png"
    render "$MAC" $((s * 2)) "$SET/icon_${s}x${s}@2x.png"
  done
  iconutil -c icns -o "$DIR/wordcraft.icns" "$SET"
else
  # Elsewhere: an .icns is PNG entries under a small header (Apple's OSTypes per pixel size).
  for s in 32 64 128 256 512 1024; do render "$MAC" "$s" "$TMP/icns-$s.png"; done
  python3 - "$TMP" "$DIR/wordcraft.icns" <<'PY'
import struct, sys
tmp, out = sys.argv[1], sys.argv[2]
body = b""
for kind, size in [("ic11", 32), ("ic12", 64), ("ic07", 128), ("ic13", 256), ("ic08", 256), ("ic14", 512), ("ic09", 512), ("ic10", 1024)]:
    data = open(f"{tmp}/icns-{size}.png", "rb").read()
    body += kind.encode() + struct.pack(">I", len(data) + 8) + data
open(out, "wb").write(b"icns" + struct.pack(">I", len(body) + 8) + body)
PY
fi
echo "icons written to $DIR"
