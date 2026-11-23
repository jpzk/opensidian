#!/bin/bash
# box-reach2.sh: source-level evidence (pinned upstream tarballs, sha256 printed) for the code_not_present questions.
set -uo pipefail
W=/var/tmp/appimg298-$(id -un); S=$W/src; mkdir -p $S; cd $S
L=/var/tmp/appimg298/inv-0.9.6-x86_64/squashfs-root/usr/lib
echo "## webkit 2.52.6 skia CMakeLists context"
K=webkitgtk-2.52.6/Source/ThirdParty/skia/CMakeLists.txt
grep -n -E 'SkDeflate|SkEmbossMaskFilter|SkBlurMaskFilterImpl|SkCodec|SkJpegCodec|SkPngCodec|SkWebp|src/codec/|src/pdf/SkPDFDocument|if *\(|endif' $K | head -60
echo "codec files compiled:"; grep -o 'src/codec/[A-Za-z_]*\.cpp' $K | sort -u | tr '\n' ' '; echo
echo "pdf files compiled: $(grep -c 'src/pdf/' $K)"
grep -rn -l 'SkPDF' webkitgtk-2.52.6/Source/cmake 2>/dev/null | head
get() { [ -s "$2" ] || curl -4 -fsSL -m 600 -o "$2.part" "$1" && [ -s "$2.part" ] && mv "$2.part" "$2"; echo "$2 $(sha256sum $2 | cut -c1-64)"; }
echo "## gdk-pixbuf 2.44.5"
get https://download.gnome.org/sources/gdk-pixbuf/2.44/gdk-pixbuf-2.44.5.tar.xz gdk-pixbuf-2.44.5.tar.xz
curl -4 -fsSL -m 60 https://download.gnome.org/sources/gdk-pixbuf/2.44/gdk-pixbuf-2.44.5.sha256sum 2>/dev/null | grep tar.xz
[ -d gdk-pixbuf-2.44.5 ] || tar -xJf gdk-pixbuf-2.44.5.tar.xz
ls gdk-pixbuf-2.44.5/gdk-pixbuf/ | grep -E '^io-' | tr '\n' ' '; echo
cat gdk-pixbuf-2.44.5/meson_options.txt 2>/dev/null | grep -E "option\('(others|builtin_loaders|glycin|png|jpeg|tiff|gif|android)" 
grep -n -E "glycin|loaders_cache|others" gdk-pixbuf-2.44.5/gdk-pixbuf/meson.build | head -30
echo "## libjpeg-turbo 2.1.5"
get https://github.com/libjpeg-turbo/libjpeg-turbo/archive/refs/tags/2.1.5.tar.gz ljt-2.1.5.tar.gz
tar -tzf ljt-2.1.5.tar.gz >/dev/null && { tar -xzf ljt-2.1.5.tar.gz 2>/dev/null; echo "grep jpeg_fdct_16x16: $(grep -rl jpeg_fdct_16x16 libjpeg-turbo-2.1.5 | wc -l) files"; ls libjpeg-turbo-2.1.5/jfdctint.c; }
echo "## libjxl 0.11.1"
get https://github.com/libjxl/libjxl/archive/refs/tags/v0.11.1.tar.gz jxl-0.11.1.tar.gz
tar -xzf jxl-0.11.1.tar.gz 2>/dev/null; grep -rln 'DecodeImageAPNG' libjxl-0.11.1 | head; grep -n -E 'apng|add_library\(jxl_extras' libjxl-0.11.1/lib/jxl_extras.cmake | head
grep -n 'container\|box' libjxl-0.11.1/lib/jxl/decode.cc | grep -n -i 'size_t' | head -5
echo "## gst-plugins-base: rtcp parser location"
for f in $L/libgst*.so*; do printf '%s %s\n' $(basename $f) "$(grep -a -c -E 'gst_rtcp_packet_sdes|GstRTCP' $f)"; done
echo "## webkit: printing path (PDF via skia?)"
for n in SkPDF skia_pdf SkDocument print-to-pdf gtk_print_operation; do printf '%s %s\n' $n "$(grep -a -c -F $n $L/libwebkit2gtk-4.1.so.0)"; done
grep -rn -E 'SkPDF|MakePDF' webkitgtk-2.52.6/Source/WebKit/UIProcess 2>/dev/null | head -3
echo REACH2-DONE
