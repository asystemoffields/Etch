#!/usr/bin/env bash
# Build and package Etch for FreeBSD (<arch> is x86_64 or aarch64):
#
#   $DIST/etch-<version>-freebsd-<arch>.tar.gz   etch, etch-cli, docs, desktop entry, icons
#
# Usage: packaging/freebsd/package.sh [--dry-run] [--skip-build]
#
# The shared release workflow (storytold/craft-common-actions release.yml) runs this script inside
# its FreeBSD VM when it exists, in place of its built-in steps: those assume the binaries are
# named after the cargo packages (wordcraft, wordcraft-cli), but Etch's binaries are etch and
# etch-cli. The archive has the built-in steps' layout. --dry-run (run by packaging-lint on Linux)
# checks the inputs and prints the layout without building.
set -euo pipefail
# shellcheck source=../env.sh
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
APP_ID=io.github.asystemoffields.Etch

DRY_RUN=0
SKIP_BUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h | --help) sed -n '2,12p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="$(uname -p 2>/dev/null || uname -m)"
case "$ARCH" in
  amd64 | x86_64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=aarch64 ;;
  *)
    if [ "$DRY_RUN" = 0 ]; then echo "unsupported architecture $ARCH" >&2; exit 2; fi
    ARCH=x86_64 # dry run on another platform (e.g. uname -p is "unknown" on Linux)
    ;;
esac
NAME="etch-$VERSION-freebsd-$ARCH"
DESKTOP="$ROOT/packaging/linux/$APP_ID.desktop"
[ -f "$DESKTOP" ] || { echo "error: $DESKTOP missing" >&2; exit 1; }
ICONS=()
for src in "$ROOT"/assets/app-icon/hicolor/*/apps/*.png "$ROOT"/assets/app-icon/hicolor/*/apps/*.svg; do
  if [ -f "$src" ]; then ICONS+=("$src"); fi
done
[ "${#ICONS[@]}" -gt 0 ] || { echo "error: no icons under assets/app-icon/hicolor" >&2; exit 1; }

if [ "$DRY_RUN" = 1 ]; then
  echo "==> dry run: $NAME.tar.gz would hold"
  echo "$NAME/etch"
  echo "$NAME/etch-cli"
  echo "$NAME/$APP_ID.desktop"
  for src in "${ICONS[@]}"; do
    size="$(basename "$(dirname "$(dirname "$src")")")"
    echo "$NAME/icons/hicolor/$size/apps/$APP_ID.${src##*.}"
  done
  for f in README.md LICENSE LICENSE-MIT LICENSE-APACHE COPYRIGHT NOTICE ATTRIBUTION.md; do
    if [ -f "$ROOT/$f" ]; then echo "$NAME/$f"; fi
  done
  exit 0
fi

echo "==> Etch $VERSION for FreeBSD $ARCH"
# The workflow's Cargo cache is for the release profile, so build that (not $PROFILE).
if [ "$SKIP_BUILD" = 0 ]; then
  (cd "$ROOT" && cargo build --release --locked -p wordcraft -p wordcraft-cli)
fi
BIN="$CARGO_TARGET_DIR/release"

OUT="$DIST/$NAME"
rm -rf "$OUT"
mkdir -p "$OUT"
install -m755 "$BIN/etch" "$BIN/etch-cli" "$OUT/"
copy_docs "$OUT"
for f in NOTICE ATTRIBUTION.md; do
  if [ -f "$ROOT/$f" ]; then cp "$ROOT/$f" "$OUT/"; fi
done
cp "$DESKTOP" "$OUT/"
# The hicolor sources keep their own file names; install them under the app id (the desktop
# entry's Icon=).
for src in "${ICONS[@]}"; do
  size="$(basename "$(dirname "$(dirname "$src")")")"
  mkdir -p "$OUT/icons/hicolor/$size/apps"
  cp "$src" "$OUT/icons/hicolor/$size/apps/$APP_ID.${src##*.}"
done
tar -C "$DIST" -czf "$DIST/$NAME.tar.gz" "$NAME"
rm -rf "$OUT"
"$BIN/etch-cli" --version
echo "wrote $DIST/$NAME.tar.gz"
ls -lh "$DIST/$NAME.tar.gz"
