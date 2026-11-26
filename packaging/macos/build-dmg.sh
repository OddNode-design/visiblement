#!/usr/bin/env bash
# Builds Visiblement.app (with libmpv and its dependencies inside) and a drag-to-Applications DMG.
# Runs on macOS with Homebrew's mpv and dylibbundler installed (see .github/workflows/macos.yml).
# Usage: packaging/macos/build-dmg.sh   → dist/Visiblement-<version>-macos-<arch>.dmg
set -euo pipefail
cd "$(dirname "$0")/../.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
ARCH=$(uname -m)                     # arm64 or x86_64
MIN_MACOS=$(sw_vers -productVersion | cut -d. -f1).0   # Homebrew's libraries target this runner's macOS
APP=dist/Visiblement.app
C=$APP/Contents

cargo build --release -p visiblement

rm -rf dist && mkdir -p "$C/MacOS" "$C/Resources" "$C/Frameworks"
cp target/release/visiblement "$C/MacOS/visiblement"
cp -R ui "$C/Resources/ui"
sed -e "s/__VERSION__/$VERSION/g" -e "s/__MIN_MACOS__/$MIN_MACOS/g" packaging/macos/Info.plist > "$C/Info.plist"

# Icon: the 512 px PNG at every size macOS asks for.
ICONSET=dist/Visiblement.iconset
mkdir -p "$ICONSET"
for s in 16 32 128 256 512; do
  sips -z $s $s assets/icon.png --out "$ICONSET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2)); [ $d -le 512 ] && sips -z $d $d assets/icon.png --out "$ICONSET/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$C/Resources/Visiblement.icns"
rm -rf "$ICONSET"

# libmpv and everything it links to (FFmpeg, libplacebo, ...), rewritten to load from Frameworks/.
cp "$(brew --prefix mpv)/lib/libmpv.2.dylib" "$C/Frameworks/"
chmod u+w "$C/Frameworks/libmpv.2.dylib"
install_name_tool -id @loader_path/libmpv.2.dylib "$C/Frameworks/libmpv.2.dylib"
dylibbundler -of -b -x "$C/Frameworks/libmpv.2.dylib" -d "$C/Frameworks" -p @loader_path/ \
  -s "$(brew --prefix)/lib" >/dev/null
# Anything still pointing into Homebrew would only work on a Mac that has it.
if otool -L "$C"/Frameworks/*.dylib | grep -E "$(brew --prefix)|/usr/local/(opt|Cellar)"; then
  echo "error: Homebrew paths left in bundled libraries" >&2
  exit 1
fi

# Ad-hoc signature: required for arm64 code to run at all. Not notarized (no Apple Developer account).
codesign --force --sign - "$C"/Frameworks/*.dylib
codesign --force --sign - "$APP"

# DMG: the app next to a shortcut to /Applications.
STAGE=dist/dmg
mkdir -p "$STAGE"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
DMG=dist/Visiblement-$VERSION-macos-$ARCH.dmg
hdiutil create -volname "Visiblement" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
echo "$DMG"
