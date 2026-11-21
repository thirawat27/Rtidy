#!/bin/bash
# Build .deb, AppImage and .tar.gz for Linux x86_64.
# usage: packaging/linux/package.sh <path-to-binary> <version>
set -euo pipefail
BIN="$1"; VERSION="$2"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="$ROOT/dist"; WORK="$(mktemp -d)"
mkdir -p "$OUT"
DESKTOP="$ROOT/packaging/linux/rtidy.desktop"
ICON="$ROOT/assets/logo.png"

# ---- .deb ----
DEB="$WORK/deb"
install -Dm755 "$BIN" "$DEB/usr/bin/rtidy"
install -Dm644 "$DESKTOP" "$DEB/usr/share/applications/rtidy.desktop"
install -Dm644 "$ICON" "$DEB/usr/share/icons/hicolor/256x256/apps/rtidy.png"
mkdir -p "$DEB/DEBIAN"
cat > "$DEB/DEBIAN/control" <<EOF
Package: rtidy
Version: $VERSION
Section: devel
Priority: optional
Architecture: amd64
Depends: libc6, libgl1, libxkbcommon0
Recommends: xdg-desktop-portal
Maintainer: thirawat27
Description: Keeps your Rust target folders tidy
 One shared target folder, lean builds and a daily clean-up,
 set up in one click.
EOF
cat > "$DEB/DEBIAN/prerm" <<'EOF'
#!/bin/sh
# remove the per-user daily task for the user running the uninstall (if any)
[ -n "$SUDO_USER" ] && su "$SUDO_USER" -c "/usr/bin/rtidy uninstall" >/dev/null 2>&1 || true
exit 0
EOF
chmod 755 "$DEB/DEBIAN/prerm"
dpkg-deb --root-owner-group --build "$DEB" "$OUT/rtidy_${VERSION}_amd64.deb"

# ---- AppImage ----
APPDIR="$WORK/rtidy.AppDir"
install -Dm755 "$BIN" "$APPDIR/usr/bin/rtidy"
cp "$DESKTOP" "$APPDIR/rtidy.desktop"
cp "$ICON" "$APPDIR/rtidy.png"
ln -s usr/bin/rtidy "$APPDIR/AppRun"
TOOL="$WORK/appimagetool"
curl -fsSL -o "$TOOL" https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
chmod +x "$TOOL"
ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUT/rtidy-x86_64.AppImage"

# ---- tar.gz ----
TAR="$WORK/rtidy-linux-x86_64"
mkdir -p "$TAR"
cp "$BIN" "$TAR/rtidy"
cp "$ROOT/README.md" "$TAR/"
tar -C "$WORK" -czf "$OUT/rtidy-linux-x86_64.tar.gz" rtidy-linux-x86_64
ls -la "$OUT"
