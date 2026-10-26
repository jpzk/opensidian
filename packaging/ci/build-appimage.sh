#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# CI COPY of harness scripts/build-appimage.sh @ 8e91f13 (harness is local-only, never pushed),
# used by packaging/ci/appimage.sh inside the pinned ubuntu container. Only changes:
# repo root is ../.. and the appimagetool path is $APPIMAGETOOL (default: the harness
# path). Keep in lockstep: `git diff --no-index` against the harness copy shows only those.
# Build opensidian AppImages via manual AppDir.
# Usage: build-appimage.sh [portable|slim] [version]
#   portable (default) — full webkit/gtk ldd closure, runs anywhere (~110MB)
#   slim               — binary + AppRun only, needs system libwebkit2gtk-4.1 (~6MB)
# Prereqs: src-tauri/target/release/opensidian built;
#          /workspace/tools/appimagetool-x86_64.AppImage (AppImageKit r13).
set -euo pipefail
cd "$(dirname "$0")/../.."
FLAVOR="${1:-portable}"
VERSION="${2:-0.2}"
BIN=src-tauri/target/release/opensidian
TOOL=${APPIMAGETOOL:-/workspace/tools/appimagetool-x86_64.AppImage}
[ -x "$BIN" ]  || { echo "missing $BIN (cargo build --release first)"; exit 1; }
[ -x "$TOOL" ] || { echo "missing $TOOL"; exit 1; }
case "$FLAVOR" in portable|slim) ;; *) echo "usage: $0 [portable|slim] [version]"; exit 1;; esac

APPDIR="dist/AppDir-$FLAVOR"
OUT="dist/opensidian-$VERSION-x86_64-$FLAVOR.AppImage"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"

echo "[appdir:$FLAVOR] binary"
cp "$BIN" "$APPDIR/usr/bin/opensidian"

if [ "$FLAVOR" = portable ]; then
  # WHERE WEBKIT PUTS ITS HELPERS IS A DISTRO DECISION, NOT A CONSTANT.
  # These paths were /usr/libexec/... and /usr/lib64/... — correct on the
  # Fedora-family machine every release up to v0.9 was cut on, and absent on the
  # Debian-family build box, where the same files live under
  # /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/. The v0.10 cut died here with
  # "cp: cannot stat '/usr/libexec/webkit2gtk-4.1/WebKitWebProcess'".
  # Search the known layouts and FAIL LOUDLY if none has them: a portable
  # AppImage silently missing its web process is a 110 MB download that shows a
  # blank window, which is worse than not building it.
  # (The AppDir's OWN layout is unchanged — AppRun exports
  # WEBKIT_EXEC_PATH=$HERE/usr/libexec/webkit2gtk-4.1 either way.)
  WKEXEC=
  for d in /usr/libexec/webkit2gtk-4.1 /usr/lib64/webkit2gtk-4.1 \
           /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1 /usr/lib/webkit2gtk-4.1; do
    [ -x "$d/WebKitWebProcess" ] && { WKEXEC=$d; break; }
  done
  [ -n "$WKEXEC" ] || { echo "FAIL: no WebKitWebProcess in any known webkit2gtk-4.1 dir — install libwebkit2gtk-4.1 or add this distro's path"; exit 1; }
  WKBUNDLE=
  for d in /usr/lib64/webkit2gtk-4.1 /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1 \
           /usr/lib/webkit2gtk-4.1 /usr/libexec/webkit2gtk-4.1; do
    [ -f "$d/injected-bundle/libwebkit2gtkinjectedbundle.so" ] && { WKBUNDLE=$d/injected-bundle/libwebkit2gtkinjectedbundle.so; break; }
  done
  [ -n "$WKBUNDLE" ] || { echo "FAIL: no libwebkit2gtkinjectedbundle.so found — the portable build would ship a webview that cannot load a page"; exit 1; }
  echo "[appdir] webkit helpers from $WKEXEC, injected bundle $WKBUNDLE"
  # Helpers AND injected bundle live in ONE AppDir dir, usr/lib/webkit2gtk-4.1/ (portfix, v0.2).
  # Release webkit2gtk IGNORES WEBKIT_EXEC_PATH / WEBKIT_INJECTED_BUNDLE_PATH (DEVELOPER_MODE only)
  # and spawns from its compiled-in absolute dirs — so the v0.1 portable needed a system webkit.
  # The bundled libwebkit2gtk is binary-patched below to relative dirs that resolve here from
  # AppRun's cwd ($HERE).
  WKDIR="$APPDIR/usr/lib/webkit2gtk-4.1"
  mkdir -p "$WKDIR/injected-bundle" "$APPDIR/usr/share/glib-2.0/schemas"
  cp "$WKEXEC/WebKitNetworkProcess" \
     "$WKEXEC/WebKitWebProcess" \
     "$WKEXEC/WebKitGPUProcess" \
     "$WKDIR/"
  cp "$WKBUNDLE" "$WKDIR/injected-bundle/"
  cp /usr/share/glib-2.0/schemas/gschemas.compiled "$APPDIR/usr/share/glib-2.0/schemas/"

  # linuxdeploy-style excludelist: libs guaranteed on any stock distro.
  # libwayland-* belongs to the GL class: the host mesa libEGL_mesa (dlopen'd via glvnd) links the HOST
  # libwayland-client and needs its newer symbols (wl_display_create_queue_with_name, mesa 26); a bundled
  # older copy on LD_LIBRARY_PATH shadows it, libEGL_mesa fails to load, and WebKitWebProcess aborts with
  # "Could not create default EGL display: EGL_BAD_PARAMETER" (measured on fresh F44/U26.04, 2026-10-03).
  EXCL='^(ld-linux|libc\.so|libm\.so|libdl\.so|libpthread\.so|librt\.so|libutil\.so|libresolv\.so|libmvec\.so|libnss_|libanl\.so|libthread_db|libstdc\+\+|libgcc_s|libGL\.so|libGLX|libGLdispatch|libOpenGL|libEGL|libdrm|libgbm|libglapi|libwayland-|libX|libxcb|libfontconfig|libfreetype|libexpat|libz\.so|libasound|libcom_err|libcrypt\.so|libkeyutils|libgpg-error)'

  echo "[appdir] ldd closure (minus excludelist)"
  { ldd "$BIN"
    ldd "$WKDIR/WebKitWebProcess"
    ldd "$WKDIR/WebKitNetworkProcess"
    ldd "$WKDIR/WebKitGPUProcess"
    ldd "$WKBUNDLE"
  } | awk '/=>/ {print $3}' | sort -u | while read -r lib; do
    [ -f "$lib" ] || continue
    base=$(basename "$lib")
    echo "$base" | grep -qE "$EXCL" && continue
    cp -n "$lib" "$APPDIR/usr/lib/" || true
  done
  echo "[appdir] bundled $(ls "$APPDIR/usr/lib" | grep -c '\.so') libs"

  # ---- brotli 1.2.1: replace the build box's pre-fix libbrotli (CVE-2026-111497) ----
  # WebKitGTK decodes HTTP Content-Encoding br (network process) and WOFF2 fonts
  # through libbrotlidec.so.1. The ldd closure above copied the BUILD BOX's copy
  # (Ubuntu 24.04: 1.1.0-2build2, pre-fix). Build upstream 1.2.1 from a PINNED
  # tarball (reviewed: docs/brotli-121/REVIEW.md) and overwrite the bundled
  # copies, keeping the sonames. Refuse on any sha256 mismatch. The box has no
  # cmake, so the upstream CMake recipe is reproduced with gcc directly
  # (same sources = c/<lib>/*.c, same -DBROTLI*_SHARED_COMPILATION defines,
  # PIC, soname libbrotli<lib>.so.1, -lm).
  BROTLI_VER=1.2.1
  BROTLI_SHA256=98d35d11a78dc453aebb7d10c3748f3f73ff78c8267c0dc697b8631f67bf7e8f
  BROTLI_URL="https://github.com/google/brotli/archive/refs/tags/v$BROTLI_VER.tar.gz"
  BROTLI_CACHE="${BROTLI_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/opensidian-brotli}"
  BROTLI_TGZ="$BROTLI_CACHE/brotli-$BROTLI_VER.tar.gz"
  mkdir -p "$BROTLI_CACHE"
  if [ ! -s "$BROTLI_TGZ" ]; then
    echo "[brotli] fetch $BROTLI_URL"
    curl -4 -fsSL -m 120 -o "$BROTLI_TGZ.part" "$BROTLI_URL"
    mv "$BROTLI_TGZ.part" "$BROTLI_TGZ"
  fi
  got=$(sha256sum "$BROTLI_TGZ" | awk '{print $1}')
  if [ "$got" != "$BROTLI_SHA256" ]; then
    echo "FAIL: brotli $BROTLI_VER tarball sha256 mismatch: got $got, pinned $BROTLI_SHA256 — refusing (delete $BROTLI_TGZ only after re-reviewing upstream)" >&2
    exit 1
  fi
  echo "[brotli] sha256 OK $BROTLI_SHA256"
  BROTLI_SRC=$(mktemp -d)
  tar -xzf "$BROTLI_TGZ" -C "$BROTLI_SRC" --strip-components=1
  BROTLI_LIBS=$(mktemp -d)
  bcc() { # bcc <lib> <extra link args...>
    _l=$1; shift
    _up=$(echo "$_l" | tr a-z A-Z)
    gcc -O2 -fPIC -shared -I"$BROTLI_SRC/c/include" \
      -DBROTLI_SHARED_COMPILATION -DBROTLI${_up}_SHARED_COMPILATION \
      -Wl,-soname,libbrotli$_l.so.1 -Wl,-z,relro,-z,now \
      -o "$BROTLI_LIBS/libbrotli$_l.so.1" "$BROTLI_SRC"/c/$_l/*.c "$@" -lm
  }
  echo "[brotli] gcc build common/dec/enc $BROTLI_VER"
  bcc common
  bcc dec -L"$BROTLI_LIBS" -l:libbrotlicommon.so.1
  bcc enc -L"$BROTLI_LIBS" -l:libbrotlicommon.so.1
  for l in common dec; do   # the two WebKit needs: MUST be in the closure, MUST be replaced
    [ -f "$APPDIR/usr/lib/libbrotli$l.so.1" ] || { echo "FAIL: libbrotli$l.so.1 not in the ldd closure — the brotli replacement would be a no-op" >&2; exit 1; }
  done
  for l in common dec enc; do  # enc only if something pulled it in (keeps common/enc in lockstep)
    f="$APPDIR/usr/lib/libbrotli$l.so.1"
    [ -f "$f" ] || continue
    # ABI: every symbol the replaced copy exports (what webkit linked against) must survive.
    bsyms() { objdump -T "$1" | awk '$4 != "*UND*" && /DF|DO/ {print $NF}' | grep '^Brotli' | sort -u; }
    miss=$(comm -23 <(bsyms "$f") <(bsyms "$BROTLI_LIBS/libbrotli$l.so.1"))
    [ -z "$miss" ] || { echo "FAIL: built libbrotli$l.so.1 lacks symbols of the bundled copy: $miss" >&2; exit 1; }
    rm -f "$f"; install -m 0755 "$BROTLI_LIBS/libbrotli$l.so.1" "$f"
    objdump -p "$f" | grep -q "SONAME *libbrotli$l.so.1" || { echo "FAIL: $f soname" >&2; exit 1; }
    echo "[brotli] bundled $f ($BROTLI_VER)"
  done
  rm -rf "$BROTLI_SRC" "$BROTLI_LIBS"

  # ---- portfix: point the BUNDLED libwebkit2gtk at the AppDir copies (Tauri's AppImage trick) ----
  # The two compiled-in dirs (helper dir = $WKEXEC, injected-bundle dir = dirname $WKBUNDLE + "/")
  # are rewritten IN PLACE to SAME-LENGTH relative paths ("./"-padded, keeping the tail
  # "webkit2gtk-4.1[/injected-bundle/]" so a suffix-merged string still reads the same bytes).
  # They resolve against the cwd, which AppRun sets to $HERE. Only the AppDir copy is touched;
  # matched with the trailing NUL so "/usr/lib/x86_64-linux-gnu" alone etc. are left alone.
  WKLIB="$APPDIR/usr/lib/libwebkit2gtk-4.1.so.0"
  [ -f "$WKLIB" ] || { echo "FAIL: $WKLIB not bundled — nothing to patch"; exit 1; }
  relpad() { # <compiled-in abs path> <AppDir-relative target> -> same-length relative path
    local old=$1 tgt=$2 d
    d=$(( ${#old} - ${#tgt} ))
    [ $d -ge 0 ] || { echo "FAIL: compiled-in '$old' shorter than '$tgt' — cannot patch same-length" >&2; return 1; }
    [ $((d % 2)) = 0 ] || { tgt="usr//${tgt#usr/}"; d=$((d - 1)); }
    local pad=; while [ ${#pad} -lt $d ]; do pad="$pad./"; done
    printf "%s" "$pad$tgt"
  }
  OLD_EXEC="$WKEXEC"; OLD_BUND="$(dirname "$WKBUNDLE")/"
  NEW_EXEC=$(relpad "$OLD_EXEC" usr/lib/webkit2gtk-4.1) || exit 1
  NEW_BUND=$(relpad "$OLD_BUND" usr/lib/webkit2gtk-4.1/injected-bundle/) || exit 1
  [ ${#NEW_EXEC} = ${#OLD_EXEC} ] && [ ${#NEW_BUND} = ${#OLD_BUND} ] || { echo "FAIL: relpad length mismatch"; exit 1; }
  SZ0=$(stat -c %s "$WKLIB")
  chmod u+w "$WKLIB"
  PATCHED=$(O1="$OLD_EXEC" N1="$NEW_EXEC" O2="$OLD_BUND" N2="$NEW_BUND" perl -0777 -i -pe '
      $a = s/\Q$ENV{O1}\E\0/$ENV{N1}\0/g; $b = s/\Q$ENV{O2}\E\0/$ENV{N2}\0/g;
      print STDERR "exec=" . ($a||0) . " bundle=" . ($b||0) . "\n";' "$WKLIB" 2>&1) || { echo "FAIL: perl patch: $PATCHED"; exit 1; }
  echo "[appdir] patched $WKLIB: '$OLD_EXEC' -> '$NEW_EXEC', '$OLD_BUND' -> '$NEW_BUND' ($PATCHED)"
  case "$PATCHED" in *"exec=0"*|*"bundle=0"*|"") echo "FAIL: 0 replacements in $WKLIB ($PATCHED) — webkit would spawn from system paths"; exit 1;; esac
  [ "$(stat -c %s "$WKLIB")" = "$SZ0" ] || { echo "FAIL: $WKLIB size changed by patch"; exit 1; }
  LEFT=$(O1="$OLD_EXEC" O2="$OLD_BUND" perl -0777 -ne '$n = () = /\Q$ENV{O1}\E\0|\Q$ENV{O2}\E\0/g; print $n' "$WKLIB")
  [ "$LEFT" = 0 ] || { echo "FAIL: $LEFT unpatched compiled-in webkit dirs remain in $WKLIB"; exit 1; }
  for f in WebKitNetworkProcess WebKitWebProcess WebKitGPUProcess; do
    [ -x "$APPDIR/${NEW_EXEC}/$f" ] || { echo "FAIL: patched exec dir does not resolve from \$HERE to $f"; exit 1; }
  done
  [ -f "$APPDIR/${NEW_BUND}libwebkit2gtkinjectedbundle.so" ] || { echo "FAIL: patched bundle dir does not resolve from \$HERE"; exit 1; }

  cat > "$APPDIR/AppRun" <<'RUN'
#!/bin/bash
HERE="$(dirname "$(readlink -f "$0")")"
# The bundled libwebkit2gtk spawns its helpers from RELATIVE dirs (patched by build-appimage.sh),
# resolved against the cwd — so we cd into the AppDir. Before that, every relative path the
# user handed us must become absolute against THEIR cwd: non-flag args (the vault argument,
# `opensidian ./notes`) and VAULT_DIR. Absolute args, flags and no-arg launches pass unchanged.
ARGS=()
for a in "$@"; do
  case "$a" in ''|-*|/*) ARGS+=("$a") ;; *) ARGS+=("$PWD/$a") ;; esac
done
case "${VAULT_DIR-}" in ''|/*) ;; *) export VAULT_DIR="$PWD/$VAULT_DIR" ;; esac
export LD_LIBRARY_PATH="$HERE/usr/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GSETTINGS_SCHEMA_DIR="$HERE/usr/share/glib-2.0/schemas"
export XDG_DATA_DIRS="$HERE/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
cd "$HERE" || { echo "opensidian: cannot cd into $HERE" >&2; exit 1; }
exec "$HERE/usr/bin/opensidian" "${ARGS[@]}"
RUN
else
  # slim: rely on system webkit2gtk-4.1; fail early with an install hint.
  cat > "$APPDIR/AppRun" <<'RUN'
#!/bin/bash
HERE="$(dirname "$(readlink -f "$0")")"
if ! ldconfig -p 2>/dev/null | grep -q 'libwebkit2gtk-4\.1\.so'; then
  echo "opensidian: system libwebkit2gtk-4.1 not found — this is the slim build." >&2
  echo "Install it:" >&2
  echo "  Debian/Ubuntu: sudo apt install libwebkit2gtk-4.1-0" >&2
  echo "  Fedora/RHEL:   sudo dnf install webkit2gtk4.1" >&2
  echo "Or grab the portable AppImage (no dependencies) from the releases page." >&2
  exit 1
fi
exec "$HERE/usr/bin/opensidian" "$@"
RUN
fi
chmod +x "$APPDIR/AppRun"

cat > "$APPDIR/opensidian.desktop" <<'DESK'
[Desktop Entry]
Name=opensidian
Exec=opensidian
Icon=opensidian
Type=Application
Categories=Utility;TextEditor;
StartupWMClass=dev.koto.opensidian
DESK
# launcher icon = the official logo, 256 px render (packaging/linux/render-icons.sh)
IC=packaging/linux/icons/256.png
[ -s "$IC" ] || { echo "[appimage:$FLAVOR] FATAL: $IC missing (official logo render)" >&2; exit 1; }
cp "$IC" "$APPDIR/opensidian.png"
cp "$IC" "$APPDIR/.DirIcon"

echo "[appimage:$FLAVOR] packing -> $OUT"
ARCH=x86_64 "$TOOL" --appimage-extract-and-run "$APPDIR" "$OUT"
chmod +x "$OUT"
ls -la "$OUT"
echo "[appimage:$FLAVOR] OK"
