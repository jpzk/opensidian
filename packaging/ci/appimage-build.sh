#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# APPIMAGE STAGE (#189 item 1): the release binary + both AppImages, built inside a PINNED
# build environment instead of on whatever the ubuntu-26.04 runner image serves that day.
#   appimage-build.sh <opensidian-tree> <ver>        (needs podman; host arch = uname -m)
# The runner is only a host: nothing it ships ends up in an artifact.
# Container: ubuntu:resolute-20261105.1 (26.04), pinned by manifest-list digest (multi-arch index:
# podman picks amd64 or arm64/v8 from the SAME index; pushed 2026-11-16).
# apt reads ONLY snapshot.ubuntu.com 20261123T150000Z (resolute, -updates, -security; main
# universe; amd64 and arm64 are both served under /ubuntu), signatures still checked with the
# image's ubuntu-archive-keyring. OPERATOR DECISION 2026-11-19: this snapshot (and the image it
# sits on) tracks the NEWEST dated snapshot - an explicit exception to the 6-week age rule for
# distro security updates in the AppImage build env only; it is still a fixed dated pin, never
# /latest, and snapshot-bump.yml proposes the next date weekly as a PR. Every requested package is
# pinned pkg=version (the same versions on both arches at that snapshot) AND its .deb sha256 per
# arch is committed in appimage-snapshot.lock.tsv (snapshot-resolve.sh output, checked below and
# in the container); the image's own packages are upgraded to the snapshot (apt-get upgrade) so
# nothing stale from the image ships; everything else is fixed by the frozen snapshot.
# Rust 1.98.0: the dist manifest is checked against its pinned sha256, the per-arch xz_hash is
# read from that manifest AND must equal the pin below, then the tarball is checked against it.
# Writes next to the AppImages (dist/):
#   pkgset-<ver>-<arch>.tsv   every installed package: name, version, arch, sha256 of the .deb
#                             (the snapshot index's SHA256, which apt verified the download against;
#                             "image" for packages that came with the pinned image unchanged)
#   libset-<ver>-<arch>.tsv   sha256 of every library in the portable AppDir (same tag -> same file)
#   sbom/opensidian-<ver>-<arch>.cdx.json (+ .sha256, parts/, logs/)   only with SBOM_TAG=<tag>: the release
#                             SBOM, generated in THIS container after the build (sbom-local.sh --release)
set -euo pipefail
T=$(readlink -f "${1:?tree}"); V=${2:?ver}
say(){ echo "[$(date +%H:%M:%S)] [appimage] $*"; }
die(){ say "FAIL: $*"; exit 1; }
DIGEST=sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db
IMG=docker.io/library/ubuntu@$DIGEST
SNAP=20261123T150000Z
PKGS="build-essential=12.12ubuntu2.26.04.2 pkg-config=2.5.1-4 libssl-dev=3.5.5-1ubuntu3.7 libwebkit2gtk-4.1-dev=2.52.6-0ubuntu0.26.04.2 libwebkit2gtk-4.1-0=2.52.6-0ubuntu0.26.04.2 libjavascriptcoregtk-4.1-0=2.52.6-0ubuntu0.26.04.2 libgtk-3-dev=3.24.52-0ubuntu1 libayatana-appindicator3-dev=0.5.94-1build1 librsvg2-dev=2.61.3+dfsg-3ubuntu0.1 patchelf=0.18.0-1.4build1 file=1:5.46-5build2 ca-certificates=20260816ubuntu0.26.04.2 curl=8.18.0-1ubuntu2.8 xz-utils=5.8.3-1ubuntu0.1"
RUST=1.98.0   # = release.yml RUST_TOOLCHAIN (checked below)
RUST_MANIFEST_SHA256=3f7d139b73bbbd0004ef6e58b430831c68cdad2b1f64ee2eb35d54c09199489a
[ "$RUST" = "$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$(dirname "$0")/../../rust-toolchain.toml")" ] || { echo "rust-toolchain.toml channel != $RUST" >&2; exit 1; }
grep -q "^RUST_MANIFEST_SHA256=$RUST_MANIFEST_SHA256 " "$(dirname "$0")/../../scripts/supply-chain/rust-toolchain.sh" || { echo "rust-toolchain.sh manifest sha256 != $RUST_MANIFEST_SHA256" >&2; exit 1; }
RUST_XZ_SHA256_x86_64=ed8ee2df70909c88cbaf87a6cfa3920dac00b537de12a6abe6906641e0f5952f
RUST_XZ_SHA256_aarch64=ac9283184301aeed06ecc9f5aa4c1be7041e18a1b197b6cb6c5d162d98f566da
ARCH=$(uname -m)
case "$ARCH" in x86_64|aarch64) ;; *) die "unsupported arch $ARCH (x86_64|aarch64)";; esac
TGT=$ARCH-unknown-linux-gnu
[ -z "${RUST_TOOLCHAIN:-}" ] || [ "$RUST_TOOLCHAIN" = "$RUST" ] || die "RUST_TOOLCHAIN=$RUST_TOOLCHAIN but this script pins $RUST"
eval "RSHA=\$RUST_XZ_SHA256_$ARCH"
TOOL=$(readlink -f "${APPIMAGETOOL:?APPIMAGETOOL (sha256-checked appimagetool r13)}")
[ -x "$TOOL" ] || die "missing $TOOL"
command -v podman >/dev/null || die "no podman"
CACHE=${APPIMAGE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/opensidian-appimage}; mkdir -p "$CACHE"
CARGO_CACHE=${CARGO_CACHE:-$HOME/.cargo}; mkdir -p "$CARGO_CACHE"

# ---- rust toolchain: manifest sha256 -> per-arch hash -> tarball sha256 ----
M=$CACHE/channel-rust-$RUST.toml
curl -4 -fsSL -m 120 -o "$M" "https://static.rust-lang.org/dist/channel-rust-$RUST.toml"
echo "$RUST_MANIFEST_SHA256  $M" | sha256sum -c - || die "rust $RUST dist manifest sha256 mismatch"
MH=$(awk -v H="[pkg.rust.target.$TGT]" '$0==H{f=1;next} /^\[/{f=0} f&&/^xz_hash /{gsub(/"/,"",$3);print $3}' "$M")
MU=$(awk -v H="[pkg.rust.target.$TGT]" '$0==H{f=1;next} /^\[/{f=0} f&&/^xz_url /{gsub(/"/,"",$3);print $3}' "$M")
[ "$MH" = "$RSHA" ] || die "rust manifest xz_hash for $TGT is '$MH', pinned $RSHA"
RT=$CACHE/rust-$RUST-$TGT.tar.xz
[ -s "$RT" ] && echo "$RSHA  $RT" | sha256sum -c --status - || curl -4 -fsSL -m 900 -o "$RT" "$MU"
echo "$RSHA  $RT" | sha256sum -c - || die "rust $RUST $TGT tarball sha256 mismatch"
say "rust $RUST $TGT sha256 $RSHA (manifest $RUST_MANIFEST_SHA256)"

say "tree $T  ver $V  arch $ARCH  image $IMG (ubuntu:resolute-20261105.1)  apt snapshot $SNAP"
podman image exists "$IMG" || podman pull -q "$IMG" >/dev/null || die "pull $IMG"
mkdir -p "$T/dist"
CA=${HOST_CA_BUNDLE:-/etc/ssl/certs/ca-certificates.crt}; [ -s "$CA" ] || die "no host CA bundle $CA"
# SBOM_TAG=<tag> (release.yml): also write dist/sbom/opensidian-<ver>-<arch>.cdx.json in the container.
# commit / dirty / epoch come from the host checkout (the container has no git).
SBOM_TAG=${SBOM_TAG:-} SBOM_COMMIT= SBOM_DIRTY= SBOM_EPOCH=
SBOM_PKGS="jq=1.8.1-4ubuntu2 libjq1=1.8.1-4ubuntu2"   # = sbom-local.sh PKGS extras (same snapshot)
# appimage-snapshot.lock.tsv (scripts/supply-chain/snapshot-resolve.sh $SNAP <names>): every pin of
# PKGS + SBOM_PKGS must be in it for amd64 AND arm64 with a sha256, at this SNAP.
LOCK=$(readlink -f "$(dirname "$0")/appimage-snapshot.lock.tsv"); [ -s "$LOCK" ] || die "no $LOCK"
[ "$(sed -n '1s/^# snapshot.ubuntu.com \([0-9TZ]*\) .*/\1/p' "$LOCK")" = "$SNAP" ] || die "$LOCK is not for snapshot $SNAP"
for p in $PKGS $SBOM_PKGS; do for da in amd64 arm64; do
  awk -F'\t' -v n="${p%%=*}" -v v="${p#*=}" -v a="$da" '!/^#/ && $1==n && $2==v && $3==a && $4 ~ /^[0-9a-f]{64}$/{f=1} END{exit !f}' "$LOCK" || die "$p ($da) not in $LOCK with a sha256"
done; done
if [ -n "$SBOM_TAG" ]; then
  SBOM_COMMIT=$(git -C "$T" rev-parse HEAD) SBOM_EPOCH=$(git -C "$T" log -1 --format=%ct)
  SBOM_DIRTY=false; [ -z "$(git -C "$T" status --porcelain --untracked-files=no)" ] || SBOM_DIRTY=true
fi
podman run --rm --network=host -e SBOM_TAG="$SBOM_TAG" -e SBOM_COMMIT="$SBOM_COMMIT" -e SBOM_DIRTY="$SBOM_DIRTY" -e SBOM_EPOCH="$SBOM_EPOCH" -e SBOM_PKGS="$SBOM_PKGS" -e V="$V" -e ARCH="$ARCH" -e TGT="$TGT" -e RUST="$RUST" -e SNAP="$SNAP" -e PKGS="$PKGS" -e IMGREF="$IMG" \
  -v "$CA":/host-ca.crt:ro -v "$LOCK":/snapshot.lock:ro -v "$T":/w -v "$RT":/rust.tar.xz:ro -v "$TOOL":/appimagetool:ro -v "$CARGO_CACHE":/cargo-home -w /w "$IMG" bash -euo pipefail -c '
  say(){ echo "[$(date +%H:%M:%S)] [appimage/resolute] $*"; }
  say "ubuntu $(. /etc/os-release; echo $VERSION_ID $VERSION_CODENAME)"
  rm -f /etc/apt/sources.list /etc/apt/sources.list.d/* /etc/apt/apt.conf.d/docker-clean
  printf "Types: deb\nURIs: https://snapshot.ubuntu.com/ubuntu/%s/\nSuites: resolute resolute-updates resolute-security\nComponents: main universe\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\nCheck-Valid-Until: no\n" "$SNAP" > /etc/apt/sources.list.d/snapshot.sources
  export DEBIAN_FRONTEND=noninteractive
  # snapshot.ubuntu.com is https-only and the bare image has no CA store yet: apt uses the host CA
  # bundle (mounted read-only) for TLS only; package integrity is the archive signature + index sha256.
  echo "Acquire::https::CAInfo \"/host-ca.crt\";" > /etc/apt/apt.conf.d/99host-ca
  cat /etc/apt/sources.list.d/snapshot.sources; ls -la /etc/apt/sources.list.d /etc/apt/apt.conf.d /usr/share/keyrings
  apt-get -q update > /w/dist/apt-$ARCH.log 2>&1 || { cat /w/dist/apt-$ARCH.log; exit 1; }
  cat /w/dist/apt-$ARCH.log; apt-cache policy > /w/dist/apt-policy-$ARCH.txt; head -20 /w/dist/apt-policy-$ARCH.txt
  if grep -E "^(Err:|W:|E:|Error:|Warning:)" /w/dist/apt-$ARCH.log; then say "FAIL: apt update against the snapshot was not clean"; exit 1; fi
  # the image packages are brought to the snapshot too (fixed snapshot -> deterministic), so the
  # bundle never carries an image library older than the pinned snapshot
  apt-get -y -q --no-install-recommends upgrade >> /w/dist/apt-$ARCH.log 2>&1 || { tail -30 /w/dist/apt-$ARCH.log; exit 1; }
  apt-get -y -q --no-install-recommends install $PKGS >> /w/dist/apt-$ARCH.log 2>&1 || { tail -30 /w/dist/apt-$ARCH.log; exit 1; }
  # every requested pin really is what got installed
  for p in $PKGS; do n=${p%%=*}; want=${p#*=}; got=$(dpkg-query -W -f="\${Version}" "$n"); [ "$got" = "$want" ] || { say "FAIL: $n is $got, pinned $want"; exit 1; }; done
  O=/w/dist/pkgset-$V-$ARCH.tsv
  printf "# image %s  snapshot %s\n# package\tversion\tarch\tdeb-sha256\n" "$IMGREF" "$SNAP" > $O
  dpkg-query -W -f="\${Package}\t\${Version}\t\${Architecture}\n" | LC_ALL=C sort | while IFS="	" read -r n v a; do
    h=$( { apt-cache show --no-all-versions "$n:$a=$v" 2>/dev/null || apt-cache show --no-all-versions "$n=$v" 2>/dev/null; } | sed -n "s/^SHA256: //p" | head -1)
    f=$(ls /var/cache/apt/archives/${n}_*.deb 2>/dev/null | head -1 || true)
    if [ -n "$f" ]; then d=$(sha256sum "$f" | cut -d" " -f1); [ -z "$h" ] || [ "$d" = "$h" ] || { say "FAIL: $f sha256 $d != index $h"; exit 1; }; h=$d; fi
    printf "%s\t%s\t%s\t%s\n" "$n" "$v" "$a" "${h:-image}"
  done >> $O
  say "pkgset: $(grep -vc "^#" $O) packages ($(grep -c "	image$" $O) unchanged from the image), sha256 $(sha256sum $O | cut -d" " -f1)"
  # every requested pin is the .deb committed in appimage-snapshot.lock.tsv for this arch
  DA=$(dpkg --print-architecture)
  for p in $PKGS; do n=${p%%=*}; want=$(awk -F"\t" -v n="$n" -v v="${p#*=}" -v a="$DA" "!/^#/ && \$1==n && \$2==v && \$3==a {print \$4}" /snapshot.lock)
    got=$(awk -F"\t" -v n="$n" "\$1==n {print \$4}" $O); [ -n "$want" ] && [ "$got" = "$want" ] || { say "FAIL: $n $DA deb sha256 ${got:-none} != lock ${want:-none}"; exit 1; }; done
  say "lock: $(echo $PKGS | wc -w) requested .debs == appimage-snapshot.lock.tsv ($DA)"
  grep -E "^(libwebkit2gtk-4.1-0|libjavascriptcoregtk-4.1-0|libgtk-3-0t64|libssl3t64|libglib2.0-0t64)	" $O | sed "s/^/    /"
  mkdir /tmp/rust && tar -xJf /rust.tar.xz -C /tmp/rust --strip-components=1
  /tmp/rust/install.sh --prefix=/opt/rust --components=rustc,cargo,rust-std-$TGT >/dev/null
  export PATH=/opt/rust/bin:$PATH CARGO_HOME=/cargo-home
  rustc --version | grep -q "^rustc $RUST " || { say "FAIL: $(rustc --version) != $RUST"; exit 1; }
  rustc -vV | grep -E "^host:"
  cd /w/src-tauri
  # ui/ is not a cargo input (release.sh): force our crate to re-embed it.
  cargo clean -p opensidian --release
  cargo build --release --locked
  B=target/release/opensidian; ls -l $B; sha256sum $B
  case "$ARCH" in x86_64) W="x86-64" ;; aarch64) W="ARM aarch64" ;; esac
  file -b $B; file -b $B | grep -q "$W" || { say "FAIL: binary is not $W"; exit 1; }
  echo "glibc floor ($ARCH): $(objdump -T $B | grep -oE "GLIBC_[0-9.]+" | sort -uV | tail -1)"; ldd --version | head -1
  cd /w
  export APPIMAGETOOL=/appimagetool APPIMAGE_EXTRACT_AND_RUN=1
  for F in slim portable; do bash packaging/ci/build-appimage.sh "$F" "$V"; done
  L=/w/dist/libset-$V-$ARCH.tsv
  (cd dist/AppDir-portable && find usr/lib -type f | LC_ALL=C sort | xargs sha256sum) > $L
  say "libset: $(wc -l < $L) files, sha256 $(sha256sum $L | cut -d" " -f1)"
  # ---- SBOM, IN THIS SAME CONTAINER, from the binary just built (G1/G6) ----
  # The dpkg database here IS the one the portable AppImage was bundled from. jq is the only
  # extra tool (same snapshot, pinned); it is installed AFTER pkgset was recorded and is not in the
  # WebKit ldd closure, so it changes neither the pkgset nor the SBOM.
  if [ -n "$SBOM_TAG" ]; then
    say "SBOM: sbom-local.sh --release in the build container ($SBOM_TAG, $ARCH, commit $SBOM_COMMIT dirty=$SBOM_DIRTY)"
    apt-get -y -q --no-install-recommends install $SBOM_PKGS >> /w/dist/apt-$ARCH.log 2>&1 || { tail -30 /w/dist/apt-$ARCH.log; exit 1; }
    for p in $SBOM_PKGS; do n=${p%%=*}; want=${p#*=}; got=$(dpkg-query -W -f="\${Version}" "$n"); [ "$got" = "$want" ] || { say "FAIL: $n is $got, pinned $want"; exit 1; }; done
    rm -rf /w/dist/sbom
    SBOM_COMMIT="$SBOM_COMMIT" SBOM_DIRTY="$SBOM_DIRTY" SOURCE_DATE_EPOCH="$SBOM_EPOCH" \
      bash scripts/supply-chain/sbom-local.sh --release --webkit native -o /w/dist/sbom -v "$V" -t "$SBOM_TAG" --arch "$ARCH" \
        --bin /w/src-tauri/target/release/opensidian --pkgset "$O"
    ls -l /w/dist/sbom; cat /w/dist/sbom/opensidian-$V-$ARCH.cdx.json.sha256
  fi
'
if [ -n "${SBOM_TAG:-}" ]; then ls -l "$T/dist/sbom/opensidian-$V-$ARCH.cdx.json" "$T/dist/sbom/opensidian-$V-$ARCH.cdx.json.sha256"; fi
ls -l "$T/dist/opensidian-$V-$ARCH-slim.AppImage" "$T/dist/opensidian-$V-$ARCH-portable.AppImage" "$T/dist/pkgset-$V-$ARCH.tsv" "$T/dist/libset-$V-$ARCH.tsv"
