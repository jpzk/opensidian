#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# CI COPY of harness bin/hz/rpm-build.sh @ 8e91f13, run by .github/workflows/release.yml on a
# GitHub-hosted ubuntu-24.04 runner. Changes: pins come from packaging/ci/pins.env (same values),
# scratch is $TMPDIR, cache $CI_CACHE (no /srv), and the rpm Version is the tag version with
# "-" -> "~" (a test tag 0.9.1-attest.1 becomes Version 0.9.1~attest.1; rpm forbids "-").
# RPM STAGE — runs ON THE BOX. Wraps an already-built release binary into
# dist/opensidian-<ver>-x86_64.rpm. Never compiles opensidian.
#   rpm-build.sh <opensidian-tree> <ver> <slim-AppImage> [outdir]
# The binary packaged is <tree>/src-tauri/target/release/opensidian — the same file
# build-appimage.sh copied into the AppImages. The stage FAILS unless the sha256 of
# /usr/bin/opensidian extracted from the rpm (rpm2cpio) equals the sha256 of
# usr/bin/opensidian inside <slim-AppImage>: one compile, one sha.
# Container: Fedora 44 GA base image, pinned by manifest digest, loaded from the
# signed-checksum OCI archive on dl.fedoraproject.org (2026-04-22, > 6 weeks old).
# Build tools come from the frozen GA repo only (dnf --repo=fedora): no package
# newer than the release is ever installed. Pins are repeated in the spec header.
set -euo pipefail
T=$(readlink -f "${1:?tree}"); V=${2:?ver}; SLIM=$(readlink -f "${3:?slim AppImage}"); OUTD=$(readlink -f "${4:-$T/dist}")
say(){ echo "[$(date +%H:%M:%S)] [rpm] $*"; }
die(){ say "FAIL: $*"; exit 1; }
. "$T/packaging/ci/pins.env"; OCI_URL=$FEDORA_OCI_URL
OCI_SHA=$FEDORA_OCI_SHA256
DIGEST=$FEDORA_DIGEST
IMG=localhost/fedora@$DIGEST
TOOLS=$FEDORA_TOOLS; PKGV=${V//-/\~}; SCR=${TMPDIR:-/tmp}; CACHE=${CI_CACHE:-$HOME/.cache/opensidian-ci}
BIN=$T/src-tauri/target/release/opensidian
SPEC=$T/packaging/rpm/opensidian.spec; ICON=$T/packaging/linux/opensidian.svg; ICD=$T/packaging/linux/icons
DESK=$T/packaging/flatpak/dev.koto.opensidian.desktop; META=$T/packaging/flatpak/dev.koto.opensidian.metainfo.xml
for f in "$BIN" "$SPEC" "$ICON" $ICD/{48,64,128,256,512}.png "$DESK" "$META" "$T/LICENSE" "$SLIM"; do [ -s "$f" ] || die "missing $f"; done
command -v podman >/dev/null || die "no podman (the ubuntu-24.04 runner image ships podman 4.9.3)"
say "tree $T  ver $V  out $OUTD"
say "image $IMG"
if ! podman image exists "$IMG"; then
  C=$CACHE/fedora44; mkdir -p $C; A=$C/${OCI_URL##*/}
  [ -s "$A" ] || curl -4 -fsSL -o "$A" "$OCI_URL"
  echo "$OCI_SHA  $A" | sha256sum -c - || die "OCI archive sha256 mismatch"
  rm -rf $C/oci && mkdir $C/oci && tar xJf "$A" -C $C/oci
  ID=$(podman pull -q oci:$C/oci) && podman tag "$ID" localhost/fedora:44
  podman image exists "$IMG" || die "loaded image does not carry $DIGEST"
fi
W=$(mktemp -d "$SCR/rpm-XXXXXX"); trap 'rm -rf "$W"' EXIT
mkdir -p $W/src $W/out
cp -p "$BIN" $W/src/opensidian; cp "$SPEC" "$DESK" "$META" "$ICON" "$T/LICENSE" $W/src/
for s in 48 64 128 256 512; do cp $ICD/$s.png $W/src/icon-$s.png; done
say "binary sha256 $(sha256sum "$BIN" | cut -d' ' -f1)  (src-tauri/target/release/opensidian)"
say "logo svg sha256 $(sha256sum "$ICON" | cut -d" " -f1)  (packaging/linux/opensidian.svg)"
podman run --rm --network=host -v $W:/w "$IMG" bash -euo pipefail -c '
  say(){ echo "[$(date +%H:%M:%S)] [rpm/fc44] $*"; }
  say "$(cat /etc/fedora-release)"
  dnf -y -q --repo=fedora --setopt=install_weak_deps=False install '"$TOOLS"' >/w/out/dnf.log 2>&1 || { tail -20 /w/out/dnf.log; exit 1; }
  say "tools: $(rpm -q rpm-build rpmlint cpio | tr "\n" " ")"
  rpmbuild -bb --define "_topdir /w/top" --define "_sourcedir /w/src" --define "ver '"$PKGV"'" /w/src/opensidian.spec > /w/out/rpmbuild.log 2>&1 || { tail -40 /w/out/rpmbuild.log; exit 1; }
  grep -E "^Wrote" /w/out/rpmbuild.log | sed "s/^/    /"
  R=$(ls /w/top/RPMS/x86_64/opensidian-'"$PKGV"'-*.x86_64.rpm) || { tail -30 /w/out/rpmbuild.log; exit 1; }
  cp "$R" /w/out/opensidian-'"$V"'-x86_64.rpm
  cd /w/out; P=opensidian-'"$V"'-x86_64.rpm
  say "rpmlint:"; rpmlint $P > rpmlint.log 2>&1 || true; sed "s/^/    /" rpmlint.log
  rpm -qlp $P > files.txt; rpm -qp --requires $P > requires.txt; rpm -qip $P > info.txt
  rpm2cpio $P > payload.cpio; mkdir x && (cd x && cpio -idm --quiet < ../payload.cpio); rm -f payload.cpio
  sha256sum x/usr/bin/opensidian | sed "s| x/| /|" > bin.sha256
  (cd x && sha256sum usr/share/icons/hicolor/*/apps/dev.koto.opensidian.png usr/share/icons/hicolor/scalable/apps/dev.koto.opensidian.svg) > icons.sha256
  rm -rf x
'
P=$OUTD/opensidian-$V-x86_64.rpm; mkdir -p "$OUTD"; cp $W/out/opensidian-$V-x86_64.rpm "$P"
L=$OUTD/rpm-$V; rm -rf "$L"; mkdir -p "$L"; cp $W/out/*.log $W/out/*.txt $W/out/*.sha256 "$L"/
say "rpm $P  $(stat -c %s "$P") bytes  sha256 $(sha256sum "$P" | cut -d' ' -f1)"
say "files:"; sed 's/^/    /' $L/files.txt
say "requires:"; sed 's/^/    /' $L/requires.txt
say "icons (installed paths):"; sed 's/^/    /' $L/icons.sha256
# ONE COMPILE, ONE SHA: the rpm's binary vs the slim AppImage's, extracted from the files themselves.
E=$W/slim; mkdir -p $E; (cd $E && "$SLIM" --appimage-extract usr/bin/opensidian >/dev/null)
RS=$(cut -d' ' -f1 $L/bin.sha256); AS=$(sha256sum $E/squashfs-root/usr/bin/opensidian | cut -d' ' -f1)
say "sha256 /usr/bin/opensidian from rpm (rpm2cpio):   $RS"
say "sha256 usr/bin/opensidian in ${SLIM##*/}: $AS"
[ "$RS" = "$AS" ] || die "rpm binary != slim AppImage binary"
say "binary identical: rpm == slim AppImage"
LINE=$(grep -E '[0-9]+ errors?, [0-9]+ warnings?' $L/rpmlint.log | tail -1)
NE=$(echo "$LINE" | grep -oE '[0-9]+ errors?' | grep -oE '[0-9]+')
say "rpmlint: $LINE"
[ "${NE:-x}" = 0 ] || die "rpmlint reports ${NE:-?} errors ($L/rpmlint.log)"
grep -q 'libwebkit2gtk-4.1.so.0' $L/requires.txt || die "requires lacks libwebkit2gtk-4.1.so.0"
for f in /usr/bin/opensidian /usr/share/applications/dev.koto.opensidian.desktop /usr/share/metainfo/dev.koto.opensidian.metainfo.xml \
         /usr/share/icons/hicolor/{48x48,64x64,128x128,256x256,512x512}/apps/dev.koto.opensidian.png /usr/share/icons/hicolor/scalable/apps/dev.koto.opensidian.svg; do
  grep -qx "$f" $L/files.txt || die "rpm lacks $f"
done
say "RPM STAGE OK $(sha256sum "$P" | cut -d' ' -f1)  ${P##*/}"
