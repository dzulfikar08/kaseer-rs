#!/usr/bin/env bash
# package-linux.sh BINARY VERSION OUT_DIR
#   OUT_DIR/kaseer-VERSION-ARCH-linux.tar.gz   bin/kaseer, desktop entry, icon, licence
#   OUT_DIR/Kaseer-VERSION-ARCH.AppImage       single-file app (needs FUSE 3, or
#                                              --appimage-extract-and-run)
# ARCH is the build host's (x86_64 or aarch64). Only the binary is bundled: everything it
# links is on the AppImage excludelist or belongs to the host's desktop stack (xkbcommon,
# Vulkan, Wayland, X11), which GPUI opens at run time. The ldd check fails on anything new,
# so a new system dependency is a decision, not an accident.
set -euo pipefail
bin="$1"; version="$2"; out="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$out"; out="$(cd "$out" && pwd)"

case "$(uname -m)" in
  x86_64)
    arch=x86_64
    tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
  aarch64|arm64)
    arch=aarch64
    tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
  *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

allowed='linux-vdso.so.1 linux-gate.so.1 ld-linux-x86-64.so.2 ld-linux-aarch64.so.1
libc.so.6 libm.so.6 libgcc_s.so.1 libdl.so.2 libpthread.so.0 librt.so.1
libfontconfig.so.1 libfreetype.so.6 libexpat.so.1 libz.so.1 libbz2.so.1.0 libpng16.so.16
libbrotlidec.so.1 libbrotlicommon.so.1
libxcb.so.1 libXau.so.6 libXdmcp.so.6 libbsd.so.0 libmd.so.0
libxkbcommon.so.0 libxkbcommon-x11.so.0 libxcb-xkb.so.1'
unexpected=""
for lib in $(ldd "$bin" | awk '{ print $1 }' | xargs -n1 basename); do
  case " $(echo "$allowed" | tr '\n' ' ') " in *" $lib "*) ;; *) unexpected="$unexpected $lib" ;; esac
done
[ -z "$unexpected" ] || { echo "error: kaseer links libraries the packages don't plan for:$unexpected" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "${work:?}"' EXIT

# ── tarball ─────────────────────────────────────────────────────────────
name="kaseer-${version}-${arch}-linux"
stage="$work/$name"
mkdir -p "$stage/bin" "$stage/share/applications" "$stage/share/icons/hicolor/scalable/apps"
install -m755 "$bin" "$stage/bin/kaseer"
install -m644 "$root/packaging/kaseer.desktop" "$stage/share/applications/"
install -m644 "$root/assets/icons/kaseer.svg" "$stage/share/icons/hicolor/scalable/apps/"
install -m644 "$root/LICENSE" "$root/README.md" "$stage/"
tar -C "$work" -czf "$out/$name.tar.gz" "$name"

# ── AppImage ────────────────────────────────────────────────────────────
fetch() { # url sha256 dest
  curl -fsSL --retry 3 -o "$3" "$1"
  echo "$2  $3" | sha256sum -c --quiet - || { echo "error: checksum mismatch for $1" >&2; exit 1; }
}
fetch "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-${arch}.AppImage" \
  "$tool_sha" "$work/appimagetool"
fetch "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-${arch}" \
  "$runtime_sha" "$work/runtime"
chmod +x "$work/appimagetool"

app="$work/Kaseer.AppDir"
mkdir -p "$app/usr/bin" "$app/usr/share/applications" "$app/usr/share/icons/hicolor/scalable/apps" \
  "$app/usr/share/doc/kaseer"
install -m755 "$bin" "$app/usr/bin/kaseer"
install -m644 "$root/packaging/kaseer.desktop" "$app/usr/share/applications/"
install -m644 "$root/assets/icons/kaseer.svg" "$app/usr/share/icons/hicolor/scalable/apps/"
install -m644 "$root/LICENSE" "$root/README.md" "$app/usr/share/doc/kaseer/"
cp "$root/packaging/kaseer.desktop" "$app/kaseer.desktop"
echo "X-AppImage-Version=$version" >> "$app/kaseer.desktop"
cp "$root/assets/icons/kaseer.svg" "$app/kaseer.svg"
ln -s kaseer.svg "$app/.DirIcon"
cat > "$app/AppRun" <<'RUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/kaseer" "$@"
RUN
chmod 755 "$app/AppRun"
appimage="$out/Kaseer-${version}-${arch}.AppImage"
ARCH="$arch" "$work/appimagetool" --appimage-extract-and-run --no-appstream \
  --runtime-file "$work/runtime" "$app" "$appimage"
chmod 755 "$appimage"
ls -l "$out/$name.tar.gz" "$appimage"
