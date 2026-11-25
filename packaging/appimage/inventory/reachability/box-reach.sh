#!/bin/bash
# box-reach.sh: static reachability evidence for the alerted packages in the v0.9.6 x86_64 bundle.
# Uses grep -a on .dynstr/.rodata (box has no binutils) + WebKitGTK 2.52.6 source CMake lists.
set -uo pipefail
W=/var/tmp/appimg298-$(id -un); mkdir -p $W; I=/var/tmp/appimg298/inv-0.9.6-x86_64; R=$I/squashfs-root; L=$R/usr/lib
:
[ -d $R ] || { echo NO-ROOT; exit 1; }
c() { printf '%s\t%s\t%s\n' "$(basename $1)" "$2" "$(grep -a -c -F -- "$2" "$1")"; }
echo "## imports/strings (file  needle  count-of-lines)"
for n in gss_init_sec_context Negotiate NTLM sqlite3_open_v2 soup_cookie_jar_db_new soup_hsts_enforcer_db_new soup_server_new; do c $L/libsoup-3.0.so.0 $n; done
for n in sqlite3_open_v2 localstorage.sqlite3 IndexedDB soup_session_new gst_init_check gst_element_factory_make rtpbin gst_rtcp xsltApplyStylesheet JxlDecoderCreate gdk_pixbuf_new_from soup_server_new soup_auth_domain_digest webkit_web_context_set_automation_allowed; do c $L/libwebkit2gtk-4.1.so.0 $n; done
for n in SkCanvas SkEmbossMaskFilter SkBlurMaskFilter SkDeflate SkBmpStandardCodec SkCodec dng_ ANGLE; do c $L/libwebkit2gtk-4.1.so.0 $n; done
for n in mnt_context_mount mnt_context_ mnt_table_parse mnt_new_monitor mnt_monitor blkid_; do c $L/libgio-2.0.so.0 $n; done
for n in blkid_ mnt_context_mount; do c $L/libmount.so.1 $n; done
for n in dbus_connection_open dbus_bus_get dbus_server_listen nonce-tcp; do c $L/libdbus-1.so.3 $n; done
for n in dbus_bus_get dbus_connection libdbus webkit_web_context_set_automation_allowed webkit_settings_set_enable_developer_extras soup_session_new gss_ sqlite3_ webkit_cookie_manager_set_persistent_storage webkit_website_data_manager; do c $R/usr/bin/opensidian $n; done
for n in sd_journal_send sd_journal_print gpt-auto journald; do c $L/libsystemd.so.0 $n; done
for n in sd_journal_send sd_journal_print; do c $L/libwebkit2gtk-4.1.so.0 $n; c $L/libjavascriptcoregtk-4.1.so.0 $n; done
for n in DecodeImageAPNG APNG acTL; do c $L/libjxl.so.0.11 $n; done
for n in gst_rtcp gst_rtp; do for f in $L/libgst*.so*; do c $f $n; done; done
for n in io-ico io-icns ico icns gdk_pixbuf__ glycin loaders.cache gdk-pixbuf-2.0/2.10.0/loaders; do c $L/libgdk_pixbuf-2.0.so.0 $n; done
for n in xsltApplyStylesheet xsltParseStylesheetDoc; do c $L/libxslt.so.1 $n; done
for n in jpeg_fdct_16x16 jpeg_CreateCompress jpeg_start_compress; do c $L/libjpeg.so.8 $n; c $L/libwebkit2gtk-4.1.so.0 $n; done
echo "## compiled-in plugin/loader dirs"
grep -a -o -E '/usr/lib/[a-z0-9_-]*-linux-gnu/(gstreamer-1.0|gdk-pixbuf-2.0[^"]*|glycin[^"]*)' $L/libgstreamer-1.0.so.0 $L/libgdk_pixbuf-2.0.so.0 $L/libglycin-2.so.0 2>/dev/null | sort -u
grep -a -o -E '/usr/(libexec|share)/glycin[-a-z0-9/._]*' $L/libglycin-2.so.0 | sort -u | head
echo "## bundled tools/binaries (non-.so)"; (cd $R && find . -type f ! -name '*.so*' | sort)
echo "## host (Fedora 44) providers for exclusion candidates"
for s in libgssapi_krb5.so.2 libkrb5.so.3 libk5crypto.so.3 libkrb5support.so.0 libsqlite3.so.0 libdbus-1.so.3 libmount.so.1 libblkid.so.1 libsystemd.so.0 libudev.so.1 libharfbuzz.so.0 libharfbuzz-icu.so.0 libfribidi.so.0; do
  h=$(ls /lib64/$s 2>/dev/null); printf '%s\t%s\t%s\n' $s "${h:-MISSING}" "$(readlink -f $h 2>/dev/null)"; done
ls -d /usr/lib64/gstreamer-1.0 /usr/lib64/gdk-pixbuf-2.0 /usr/libexec/glycin-loaders 2>&1 | sed 's/^/hostdir: /'
echo "## WebKitGTK 2.52.6 source: which Skia files are compiled"
S=$W/src; mkdir -p $S; cd $S
for v in 2.52.6 2.54.3; do
  t=webkitgtk-$v.tar.xz
  [ -s $t ] || curl -4 -fsSL -m 900 -o $t.part https://webkitgtk.org/releases/$t && [ -s $t.part ] && mv $t.part $t
  [ -s $t.sums ] || curl -4 -fsSL -m 60 -o $t.sums https://webkitgtk.org/releases/$t.sums
  echo "$v: $(sha256sum $t | cut -c1-64)  sums: $(grep -i -A0 'sha256\|^[0-9a-f]\{64\}' $t.sums | grep -o '[0-9a-f]\{64\}' | head -1)"
  [ -d webkitgtk-$v/Source/ThirdParty/skia ] || tar -xJf $t --wildcards "webkitgtk-$v/Source/ThirdParty/skia/*" "webkitgtk-$v/Source/cmake/*"
  ls webkitgtk-$v/Source/ThirdParty/skia | tr "\n" " "; echo; find webkitgtk-$v/Source/ThirdParty/skia -name "*.cpp" | wc -l
  for f in SkEmbossMaskFilter.cpp SkBlurMaskFilterImpl.cpp SkDeflate.cpp SkBmpStandardCodec.cpp dng_misc_opcodes.cpp; do printf "%s\tfile-present\t%s\t%s\n" $v $f "$(find webkitgtk-$v/Source/ThirdParty/skia -name $f | head -1)"; done
  ls webkitgtk-$v/Source/ThirdParty/skia/ 2>&1 | head
  for f in SkEmbossMaskFilter SkBlurMaskFilterImpl SkDeflate SkBmpStandardCodec SkBmpCodec SkCodec dng_ SkPDF SkJpegCodec SkPngCodec SkWebp; do
    printf '%s\tCMake-mentions\t%s\t%s\n' $v $f "$(grep -c $f webkitgtk-$v/Source/ThirdParty/skia/CMakeLists.txt 2>/dev/null)"; done
  grep -n -iE 'skia|USE_SKIA|ENABLE_VIDEO|USE_GSTREAMER|USE_LIBSECRET|USE_JPEGXL|ENABLE_XSLT|USE_SYSTEM|ANGLE' webkitgtk-$v/Source/cmake/OptionsGTK.cmake | head -40
done
echo REACH-DONE
