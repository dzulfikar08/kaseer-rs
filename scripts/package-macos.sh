#!/usr/bin/env bash
# package-macos.sh BINARY VERSION OUT_DIR   (run on macOS)
#   OUT_DIR/Kaseer-VERSION.dmg                     Kaseer.app + an Applications link
#   OUT_DIR/kaseer-VERSION-universal-macos.tar.gz  Kaseer.app
# BINARY should be universal (lipo of the aarch64 and x86_64 builds). The app is ad-hoc
# signed, not notarised: on first launch, right-click Kaseer.app and choose Open.
set -euo pipefail
bin="$1"; version="$2"; out="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$out"; out="$(cd "$out" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "${work:?}"' EXIT

app="$work/Kaseer.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m755 "$bin" "$app/Contents/MacOS/kaseer"

iconset="$work/kaseer.iconset"
mkdir -p "$iconset"
src="$root/packaging/macos/kaseer-1024.png"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" "$src" --out "$iconset/icon_${s}x${s}.png" >/dev/null
  sips -z $((s * 2)) $((s * 2)) "$src" --out "$iconset/icon_${s}x${s}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/kaseer.icns"
install -m644 "$root/LICENSE" "$app/Contents/Resources/"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Kaseer</string>
  <key>CFBundleDisplayName</key><string>Kaseer</string>
  <key>CFBundleIdentifier</key><string>site.kaseer.terminal</string>
  <key>CFBundleExecutable</key><string>kaseer</string>
  <key>CFBundleIconFile</key><string>kaseer</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.business</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>AGPL-3.0-or-later</string>
</dict>
</plist>
PLIST
plutil -lint "$app/Contents/Info.plist" >/dev/null
codesign --force --sign - "$app"

tar -C "$work" -czf "$out/kaseer-${version}-universal-macos.tar.gz" Kaseer.app

dmgroot="$work/dmg"
mkdir -p "$dmgroot"
cp -R "$app" "$dmgroot/"
ln -s /Applications "$dmgroot/Applications"
hdiutil create -volname "Kaseer ${version}" -srcfolder "$dmgroot" -ov -format UDZO \
  "$out/Kaseer-${version}.dmg" >/dev/null
ls -l "$out"
