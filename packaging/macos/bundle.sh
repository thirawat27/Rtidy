#!/bin/bash
# Build rtidy.app and a drag-to-Applications .dmg.
# usage: packaging/macos/bundle.sh <path-to-universal-binary> <version>
set -euo pipefail
BIN="$1"; VERSION="$2"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/dist"; WORK="$(mktemp -d)"
APP="$WORK/stage/rtidy.app"

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$OUT"
cp "$BIN" "$APP/Contents/MacOS/rtidy"
chmod +x "$APP/Contents/MacOS/rtidy"

# icon
ICONSET="$WORK/rtidy.iconset"; mkdir -p "$ICONSET"
SRC="$ROOT/assets/logo-1024.png"
for s in 16 32 128 256 512; do
  sips -z $s $s "$SRC" --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2)); sips -z $d $d "$SRC" --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/rtidy.icns"

sed "s/@VERSION@/$VERSION/g" "$ROOT/packaging/macos/Info.plist" > "$APP/Contents/Info.plist"

# ad-hoc signature so Apple Silicon will run it (not notarized)
codesign --force --deep --sign - "$APP"

ln -s /Applications "$WORK/stage/Applications"
hdiutil create -volname "rtidy" -srcfolder "$WORK/stage" -ov -format UDZO "$OUT/rtidy-macos-universal.dmg"
echo "built $OUT/rtidy-macos-universal.dmg"
