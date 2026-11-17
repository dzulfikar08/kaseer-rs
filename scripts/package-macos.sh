#!/usr/bin/env bash
# package-macos.sh BINARY VERSION OUT_DIR   (run on macOS)
#   OUT_DIR/Kaseer-VERSION.dmg                     Kaseer.app + an Applications link
#   OUT_DIR/kaseer-VERSION-universal-macos.tar.gz  Kaseer.app
# BINARY should be universal (lipo of the aarch64 and x86_64 builds).
#
# Signing: with a "Developer ID Application" identity in the keychain (CI: ci-keychain.sh),
# the app and DMG are signed with the hardened runtime; with MACOS_NOTARIZE=1 and
# ASC_KEY_PATH / ASC_KEY_ID / ASC_ISSUER_ID set, the DMG is notarised and the ticket stapled
# to both, so Gatekeeper opens them without a warning. Without an identity (forks, local
# builds) the app is ad-hoc signed: right-click Kaseer.app and choose Open the first time.
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

identity="${MACOS_SIGN_IDENTITY:-$(security find-identity -v -p codesigning \
  | awk '/"Developer ID Application/ { print $2; exit }')}"
if [ -n "$identity" ]; then
  codesign --force --options runtime --timestamp \
    --entitlements "$root/packaging/macos/Kaseer.entitlements" --sign "$identity" "$app"
  codesign --verify --strict --verbose=2 "$app"
else
  echo "::notice::no Developer ID Application identity: ad-hoc signing"
  codesign --force --sign - "$app"
fi

dmgroot="$work/dmg"
mkdir -p "$dmgroot"
cp -R "$app" "$dmgroot/"
ln -s /Applications "$dmgroot/Applications"
hdiutil create -volname "Kaseer ${version}" -srcfolder "$dmgroot" -ov -format UDZO \
  "$out/Kaseer-${version}.dmg" >/dev/null
dmg="$out/Kaseer-${version}.dmg"

if [ -n "$identity" ]; then
  codesign --force --timestamp --sign "$identity" "$dmg"
fi
if [ -n "$identity" ] && [ "${MACOS_NOTARIZE:-0}" = 1 ]; then
  result="$work/notary.json"
  # notarytool exits 0 for a finished but rejected submission, so read the status.
  xcrun notarytool submit "$dmg" --key "$ASC_KEY_PATH" --key-id "$ASC_KEY_ID" \
    --issuer "$ASC_ISSUER_ID" --wait --timeout 1h --output-format json > "$result" || true
  cat "$result"; echo
  status="$(plutil -extract status raw -o - "$result" 2>/dev/null || true)"
  sub_id="$(plutil -extract id raw -o - "$result" 2>/dev/null || true)"
  if [ "$status" != Accepted ]; then
    echo "error: notarisation status '${status:-unknown}'" >&2
    [ -z "$sub_id" ] || xcrun notarytool log "$sub_id" --key "$ASC_KEY_PATH" \
      --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" >&2 || true
    exit 1
  fi
  xcrun stapler staple "$dmg"
  xcrun stapler validate "$dmg"
  spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
  # The DMG's ticket covers the app inside it too; staple it for the tarball.
  xcrun stapler staple "$app"
  spctl --assess --type execute --verbose=2 "$app"
fi

tar -C "$work" -czf "$out/kaseer-${version}-universal-macos.tar.gz" Kaseer.app
ls -l "$out"
