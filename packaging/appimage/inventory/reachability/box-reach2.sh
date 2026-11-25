#!/bin/bash
# box-reach2.sh: source-level evidence (pinned upstream tarballs, sha256 printed) for the code_not_present questions.
# Fail closed: the source probes may legitimately match nothing (so no -e), but a failed/empty download, a sha256
# other than the pin, or a failed unpack aborts the run (REACH2-FAIL, rc 1) before any probe reads a missing tree.
set -uo pipefail
W=/var/tmp/appimg298-$(id -un); S=$W/src; mkdir -p $S; cd $S
L=/var/tmp/appimg298/inv-0.9.6-x86_64/squashfs-root/usr/lib
echo "## webkit 2.52.6 skia CMakeLists context"
K=webkitgtk-2.52.6/Source/ThirdParty/skia/CMakeLists.txt
[ -s "$K" ] || { echo "REACH2-FAIL: $K missing (unpack webkitgtk-2.52.6 first)" >&2; exit 1; }
grep -n -E 'SkDeflate|SkEmbossMaskFilter|SkBlurMaskFilterImpl|SkCodec|SkJpegCodec|SkPngCodec|SkWebp|src/codec/|src/pdf/SkPDFDocument|if *\(|endif' $K | head -60
echo "codec files compiled:"; grep -o 'src/codec/[A-Za-z_]*\.cpp' $K | sort -u | tr '\n' ' '; echo
echo "pdf files compiled: $(grep -c 'src/pdf/' $K)"
grep -rn -l 'SkPDF' webkitgtk-2.52.6/Source/cmake 2>/dev/null | head
die() { echo "REACH2-FAIL: $*" >&2; exit 1; }
get() { # <url> <file> <sha256>: fetch once, verify the pinned sha256; non-zero on ANY failure (#307)
  local url=$1 f=$2 want=${3:-} got
  [[ $want =~ ^[0-9a-f]{64}$ ]] || { echo "get: no pinned sha256 for $f" >&2; return 1; }
  if [ ! -s "$f" ]; then
    rm -f "$f.part"
    curl -4 -fsSL -m 600 -o "$f.part" "$url" || { rm -f "$f.part"; echo "get: download failed: $url" >&2; return 1; }
    [ -s "$f.part" ] || { rm -f "$f.part"; echo "get: empty download: $url" >&2; return 1; }
    got=$(sha256sum "$f.part" | cut -c1-64) || { rm -f "$f.part"; echo "get: sha256sum failed: $f.part" >&2; return 1; }
    [ "$got" = "$want" ] || { rm -f "$f.part"; echo "get: sha256 mismatch $url: got $got want $want" >&2; return 1; }
    mv "$f.part" "$f" || return 1
  fi
  got=$(sha256sum "$f" | cut -c1-64) || { echo "get: sha256sum failed: $f" >&2; return 1; }
  [ "$got" = "$want" ] || { echo "get: sha256 mismatch on cached $f: got $got want $want" >&2; return 1; }
  echo "$f $got"
}
echo "## gdk-pixbuf 2.44.5"
get https://download.gnome.org/sources/gdk-pixbuf/2.44/gdk-pixbuf-2.44.5.tar.xz gdk-pixbuf-2.44.5.tar.xz 69b93e09139b80c0ee661503d60deb5a5874a31772b5184b9cd5462a4100ab68 || die "gdk-pixbuf-2.44.5"
curl -4 -fsSL -m 60 https://download.gnome.org/sources/gdk-pixbuf/2.44/gdk-pixbuf-2.44.5.sha256sum 2>/dev/null | grep tar.xz
[ -d gdk-pixbuf-2.44.5 ] || tar -xJf gdk-pixbuf-2.44.5.tar.xz || die "extract gdk-pixbuf-2.44.5"
ls gdk-pixbuf-2.44.5/gdk-pixbuf/ | grep -E '^io-' | tr '\n' ' '; echo
cat gdk-pixbuf-2.44.5/meson_options.txt 2>/dev/null | grep -E "option\('(others|builtin_loaders|glycin|png|jpeg|tiff|gif|android)" 
grep -n -E "glycin|loaders_cache|others" gdk-pixbuf-2.44.5/gdk-pixbuf/meson.build | head -30
echo "## libjpeg-turbo 2.1.5"
get https://github.com/libjpeg-turbo/libjpeg-turbo/archive/refs/tags/2.1.5.tar.gz ljt-2.1.5.tar.gz 254f3642b04e309fee775123133c6464181addc150499561020312ec61c1bf7c || die "libjpeg-turbo-2.1.5"
tar -xzf ljt-2.1.5.tar.gz || die "extract ljt-2.1.5"; { echo "grep jpeg_fdct_16x16: $(grep -rl jpeg_fdct_16x16 libjpeg-turbo-2.1.5 | wc -l) files"; ls libjpeg-turbo-2.1.5/jfdctint.c; }
echo "## libjxl 0.11.1"
get https://github.com/libjxl/libjxl/archive/refs/tags/v0.11.1.tar.gz jxl-0.11.1.tar.gz 1492dfef8dd6c3036446ac3b340005d92ab92f7d48ee3271b5dac1d36945d3d9 || die "libjxl-0.11.1"
tar -xzf jxl-0.11.1.tar.gz || die "extract jxl-0.11.1"; grep -rln 'DecodeImageAPNG' libjxl-0.11.1 | head; grep -n -E 'apng|add_library\(jxl_extras' libjxl-0.11.1/lib/jxl_extras.cmake | head
grep -n 'container\|box' libjxl-0.11.1/lib/jxl/decode.cc | grep -n -i 'size_t' | head -5
echo "## gst-plugins-base: rtcp parser location"
for f in $L/libgst*.so*; do printf '%s %s\n' $(basename $f) "$(grep -a -c -E 'gst_rtcp_packet_sdes|GstRTCP' $f)"; done
echo "## webkit: printing path (PDF via skia?)"
for n in SkPDF skia_pdf SkDocument print-to-pdf gtk_print_operation; do printf '%s %s\n' $n "$(grep -a -c -F $n $L/libwebkit2gtk-4.1.so.0)"; done
grep -rn -E 'SkPDF|MakePDF' webkitgtk-2.52.6/Source/WebKit/UIProcess 2>/dev/null | head -3
echo REACH2-DONE
