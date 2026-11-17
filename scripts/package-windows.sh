#!/usr/bin/env bash
# package-windows.sh KASEER_EXE VERSION ARCH OUT_DIR   (Git Bash on a Windows runner)
#   OUT_DIR/Kaseer-Setup-VERSION-ARCH.exe   per-user Inno Setup installer (no admin rights)
#   OUT_DIR/Kaseer-VERSION-ARCH.zip         portable: unzip anywhere, run kaseer.exe
# Unsigned: SmartScreen warns on first run ("More info", then "Run anyway").
set -euo pipefail
exe="$1"; version="$2"; arch="$3"; out="$4"
case "$arch" in x64|arm64) ;; *) echo "error: ARCH must be x64 or arm64" >&2; exit 2 ;; esac
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$out"; out="$(cd "$out" && pwd)"
stage="$(mktemp -d)/Kaseer"
mkdir -p "$stage"
cp "$exe" "$stage/kaseer.exe"
cp "$root/LICENSE" "$stage/LICENSE.txt"
cp "$root/README.md" "$stage/README.md"

iscc="$(command -v iscc || true)"
[ -n "$iscc" ] || iscc="/c/Program Files (x86)/Inno Setup 6/ISCC.exe"
if [ ! -x "$iscc" ]; then
  echo "Inno Setup not found; installing it with choco"
  choco install innosetup -y --no-progress >/dev/null
fi
setup="Kaseer-Setup-${version}-${arch}"
MSYS2_ARG_CONV_EXCL='*' "$iscc" /Q "/DAppVersion=${version}" "/DArch=${arch}" \
  "/DSourceDir=$(cygpath -w "$stage")" "/DOutputDir=$(cygpath -w "$out")" "/DOutputName=${setup}" \
  "$(cygpath -w "$root/packaging/windows/kaseer.iss")"
test -f "$out/${setup}.exe"

zip="Kaseer-${version}-${arch}.zip"
( cd "$stage" && 7z a -tzip -mx=9 "$out/$zip" ./* >/dev/null )
ls -l "$out"
