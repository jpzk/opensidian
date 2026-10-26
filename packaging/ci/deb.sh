#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# CI COPY of harness bin/hz/deb-build.sh @ 8e91f13, run by .github/workflows/release.yml on a
# GitHub-hosted ubuntu-24.04 runner. Changes: pins come from packaging/ci/pins.env (same values),
# scratch is $TMPDIR (no /srv), and the deb Version is the tag version with "-" -> "~"
# (0.9.1-attest.1 -> 0.9.1~attest.1, which sorts BEFORE 0.9.1 as a prerelease should).
# DEB STAGE — runs ON THE BOX. Wraps an already-built release binary into
# dist/opensidian-<ver>-x86_64.deb (Debian 13 / Ubuntu 26.04). Never compiles opensidian.
#   deb-build.sh <opensidian-tree> <ver> <slim-AppImage> [outdir]
# Packages <tree>/src-tauri/target/release/opensidian — the file build-appimage.sh copied
# into the AppImages — and FAILS unless the sha256 of usr/bin/opensidian extracted from the
# deb (dpkg-deb -x) equals the one inside <slim-AppImage>: one compile, one sha.
# Container: debian:trixie-20260803 (13.6), pinned by manifest-list digest (pushed
# 2026-08-05, > 6 weeks old). apt reads ONLY the frozen snapshot that image was built
# from (snapshot.debian.org 20260803T000000Z, trixie main); build tools pinned by version.
# Depends = dpkg-shlibdeps over the binary's DT_NEEDED; nothing hand-guessed.
# Files/icons exactly as the rpm: bin, desktop, metainfo, hicolor 48..512 png + scalable svg
# (+ the doc/copyright and changelog Debian policy requires).
set -euo pipefail
T=$(readlink -f "${1:?tree}"); V=${2:?ver}; SLIM=$(readlink -f "${3:?slim AppImage}"); OUTD=$(readlink -f "${4:-$T/dist}")
say(){ echo "[$(date +%H:%M:%S)] [deb] $*"; }
die(){ say "FAIL: $*"; exit 1; }
. "$T/packaging/ci/pins.env"; PKGV=${V//-/\~}; SCR=${TMPDIR:-/tmp}
IMG=$DEBIAN_IMG
SNAP=$DEBIAN_SNAPSHOT
# libwebkit2gtk-4.1-0 is installed only so dpkg-shlibdeps can map sonames -> packages.
TOOLS=$DEBIAN_TOOLS
APPID=dev.koto.opensidian
BIN=$T/src-tauri/target/release/opensidian; PD=$T/packaging/deb
ICON=$T/packaging/linux/opensidian.svg; ICD=$T/packaging/linux/icons
DESK=$T/packaging/flatpak/$APPID.desktop; META=$T/packaging/flatpak/$APPID.metainfo.xml
for f in "$BIN" "$PD/control.in" "$PD/copyright" "$PD/changelog.in" "$ICON" $ICD/{48,64,128,256,512}.png "$DESK" "$META" "$SLIM"; do
  [ -s "$f" ] || die "missing $f"; done
command -v podman >/dev/null || die "no podman"
# changelog date = the packaged commit's date, so the same SHA gives the same changelog
CDATE=$(git -C "$T" log -1 --format=%aD 2>/dev/null || date -R)
say "tree $T  ver $V  out $OUTD"
say "image $IMG  (debian:trixie-20260803)  apt snapshot $SNAP"
podman image exists "$IMG" || podman pull -q "$IMG" >/dev/null || die "pull $IMG"
W=$(mktemp -d "$SCR/deb-XXXXXX"); trap 'rm -rf "$W"' EXIT
umask 022
R=$W/root; mkdir -p $W/out $W/debian
install -Dm755 "$BIN" $R/usr/bin/opensidian
install -Dm644 "$DESK" $R/usr/share/applications/$APPID.desktop
install -Dm644 "$META" $R/usr/share/metainfo/$APPID.metainfo.xml
for s in 48 64 128 256 512; do install -Dm644 $ICD/$s.png $R/usr/share/icons/hicolor/${s}x${s}/apps/$APPID.png; done
install -Dm644 "$ICON" $R/usr/share/icons/hicolor/scalable/apps/$APPID.svg
install -Dm644 "$PD/copyright" $R/usr/share/doc/opensidian/copyright
sed -e "s/@VERSION@/$PKGV/g" -e "s/@DATE@/$CDATE/" "$PD/changelog.in" | gzip -9n > $W/changelog.gz
install -Dm644 $W/changelog.gz $R/usr/share/doc/opensidian/changelog.gz
grep -v '^#' "$PD/control.in" > $W/control.in
printf 'Source: opensidian\n\nPackage: opensidian\nArchitecture: amd64\n' > $W/debian/control
say "binary sha256 $(sha256sum "$BIN" | cut -d' ' -f1)  (src-tauri/target/release/opensidian)"
say "logo svg sha256 $(sha256sum "$ICON" | cut -d' ' -f1)  (packaging/linux/opensidian.svg)"
podman run --rm --network=host -v $W:/w "$IMG" bash -euo pipefail -c '
  say(){ echo "[$(date +%H:%M:%S)] [deb/trixie] $*"; }
  say "debian $(cat /etc/debian_version)"
  rm -f /etc/apt/sources.list.d/*
  echo "deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/'"$SNAP"' trixie main" > /etc/apt/sources.list
  export DEBIAN_FRONTEND=noninteractive
  { apt-get -q update && apt-get -y -q --no-install-recommends install '"$TOOLS"'; } >/w/out/apt.log 2>&1 || { tail -20 /w/out/apt.log; exit 1; }
  say "tools: $(dpkg-query -W -f="\${Package}=\${Version} " dpkg-dev lintian libwebkit2gtk-4.1-0)"
  cd /w
  DEP=$(dpkg-shlibdeps -O -e root/usr/bin/opensidian 2>/w/out/shlibdeps.log | sed -n "s/^shlibs:Depends=//p")
  [ -n "$DEP" ] || { cat /w/out/shlibdeps.log; exit 1; }
  echo "$DEP" > /w/out/depends.txt
  SZ=$(du -sk --apparent-size root | cut -f1)
  mkdir -p root/DEBIAN
  sed -e "s/@VERSION@/'"$PKGV"'/" -e "s/@INSTALLED_SIZE@/$SZ/" -e "s/@DEPENDS@/$DEP/" control.in > root/DEBIAN/control
  (cd root && find usr -type f | LC_ALL=C sort | xargs md5sum) > root/DEBIAN/md5sums
  chmod 644 root/DEBIAN/control root/DEBIAN/md5sums
  P=out/opensidian-'"$V"'-x86_64.deb
  dpkg-deb --root-owner-group -Zxz --build root $P > out/dpkg-deb.log 2>&1 || { cat out/dpkg-deb.log; exit 1; }
  cd out; P=${P#out/}
  dpkg-deb -I $P > info.txt; dpkg-deb -c $P > files.txt; dpkg-deb -f $P Depends > depends-field.txt
  say "lintian:"; lintian --info --display-level ">=pedantic" $P > lintian.log 2>&1 || true; sed "s/^/    /" lintian.log
  dpkg-deb -x $P x
  sha256sum x/usr/bin/opensidian | sed "s| x/| /|" > bin.sha256
  (cd x && sha256sum usr/share/icons/hicolor/*/apps/'"$APPID"'.png usr/share/icons/hicolor/scalable/apps/'"$APPID"'.svg) > icons.sha256
  rm -rf x
'
P=$OUTD/opensidian-$V-x86_64.deb; mkdir -p "$OUTD"; cp $W/out/opensidian-$V-x86_64.deb "$P"
L=$OUTD/deb-$V; rm -rf "$L"; mkdir -p "$L"; cp $W/out/*.log $W/out/*.txt $W/out/*.sha256 "$L"/
say "deb $P  $(stat -c %s "$P") bytes  sha256 $(sha256sum "$P" | cut -d' ' -f1)"
say "files:"; sed 's/^/    /' $L/files.txt
say "Depends: $(cat $L/depends-field.txt)"
say "icons (installed paths):"; sed 's/^/    /' $L/icons.sha256
E=$W/slim; mkdir -p $E; (cd $E && "$SLIM" --appimage-extract usr/bin/opensidian >/dev/null)
DS=$(cut -d' ' -f1 $L/bin.sha256); AS=$(sha256sum $E/squashfs-root/usr/bin/opensidian | cut -d' ' -f1)
say "sha256 /usr/bin/opensidian from deb (dpkg-deb -x): $DS"
say "sha256 usr/bin/opensidian in ${SLIM##*/}: $AS"
[ "$DS" = "$AS" ] || die "deb binary != slim AppImage binary"
say "binary identical: deb == slim AppImage"
# Known lintian false positive (v0.5, pdfembed): embedded-library freetype matches the string
# "HuaTianSongTi?" (lintian 2.122.0 data/binaries/embedded-libs). That string is skrifa 0.42.1's
# src/outline/hint_reliant.rs list of hint-reliant fonts (pure Rust, pulled in by hayro) — no
# FreeType code is linked. Tolerated ONLY this exact line, and only while the binary carries none
# of FreeType's own API names (a real static copy exports/strings FT_Init_FreeType, FT_Load_Glyph).
FPFT='E: opensidian: embedded-library freetype [usr/bin/opensidian]'
if grep -qxF "$FPFT" $L/lintian.log; then
  NFT=$(strings -a $E/squashfs-root/usr/bin/opensidian | grep -cE 'FT_(Init_FreeType|Load_Glyph|New_Face)' || true)
  [ "$NFT" = 0 ] || die "lintian freetype hit AND $NFT FreeType API strings in the binary — real embedded copy?"
  say "lintian: tolerated known false positive ($FPFT): skrifa hint_reliant font list, 0 FreeType API strings"
fi
NE=$(grep '^E: ' $L/lintian.log | grep -cvxF "$FPFT" || true); NW=$(grep -c '^W: ' $L/lintian.log || true)
say "lintian: $NE errors (E:) beyond the tolerated list, $NW warnings (W:)"
[ "$NE" = 0 ] || die "lintian reports $NE E: lines ($L/lintian.log)"
grep -qE '(^|, )libwebkit2gtk-4\.1-0( |,|$)' $L/depends-field.txt || die "Depends lacks libwebkit2gtk-4.1-0"
for f in usr/bin/opensidian usr/share/applications/$APPID.desktop usr/share/metainfo/$APPID.metainfo.xml \
         usr/share/icons/hicolor/{48x48,64x64,128x128,256x256,512x512}/apps/$APPID.png usr/share/icons/hicolor/scalable/apps/$APPID.svg; do
  grep -qE " \./$f\$" $L/files.txt || die "deb lacks /$f"
done
say "DEB STAGE OK $(sha256sum "$P" | cut -d' ' -f1)  ${P##*/}"
