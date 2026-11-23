#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# LINUX PR TEST STAGE (.github/workflows/linux.yml): the full cargo test + a build of src-tauri,
# inside the SAME pinned build environment the release AppImages are built in.
#   linux-test.sh <opensidian-tree>        (needs podman + curl on the host; host arch = uname -m)
# Pins = the release build env of packaging/ci/appimage-build.sh, as literals here (pin-check.sh checks
#   them statically) that must EQUAL appimage-build.sh's at run time (fail closed, no drift):
#   container image digest, apt snapshot date, the pkg=version list, Rust version + dist manifest
#   sha256 + per-arch tarball sha256. snapshot-bump.sh rewrites both files. The apt set must be in
#   appimage-snapshot.lock.tsv with a .deb sha256 for this arch, and the installed .debs must equal
#   the lock (same checks as the release).
# Rust: dist manifest sha256 (= scripts/supply-chain/rust-toolchain.sh) -> per-arch xz_hash == pin
#   -> tarball sha256 == pin; installed rustc must be the pinned version.
# Runs:  cargo test --locked --no-fail-fast  (src-tauri, = box T0 scope), then
#        cargo build --locked --profile devfast  (a build that does not reuse the test profile).
# Gate:  cargo test rc must be 0 AND the passed counts must reach the floors (a run that silently
#        lost tests fails): TOTAL_MIN (all test binaries + doctests) and LIB_MIN (the lib unittests).
#        Defaults = the coordinator box count for develop 3b46919 (2026-11-23): 669 total / 592 lib.
# Output: $OUT (default <tree>/dist/linux-test) cargo-test.log, cargo-build.log, apt.log,
#         pkgset.tsv, summary.txt. Nothing is cached: the cargo home lives in $OUT/cargo-home
#         (fresh per run, never restored from or saved to any cache).
set -euo pipefail
T=$(readlink -f "${1:?tree}")
say(){ echo "[$(date +%H:%M:%S)] [linux-test] $*"; }
die(){ say "FAIL: $*"; exit 1; }
TOTAL_MIN=${TOTAL_MIN:-669} LIB_MIN=${LIB_MIN:-592}
case "$TOTAL_MIN$LIB_MIN" in *[!0-9]*) die "TOTAL_MIN/LIB_MIN must be integers";; esac
HERE=$(cd "$(dirname "$0")" && pwd)
AB=$HERE/appimage-build.sh; LOCK=$HERE/appimage-snapshot.lock.tsv
[ -s "$AB" ] && [ -s "$LOCK" ] || die "missing $AB or $LOCK"
# ---- pins: literal here (pin-check.sh checks them statically), and each must EQUAL the same
# assignment in appimage-build.sh (checked below, fail closed), so this test env cannot drift from
# the release build env. snapshot-bump.sh rewrites both files together.
DIGEST=sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db
IMG=docker.io/library/ubuntu@$DIGEST
SNAP=20261119T120000Z
PKGS="build-essential=12.12ubuntu2.26.04.2 pkg-config=2.5.1-4 libssl-dev=3.5.5-1ubuntu3.7 libwebkit2gtk-4.1-dev=2.52.6-0ubuntu0.26.04.2 libwebkit2gtk-4.1-0=2.52.6-0ubuntu0.26.04.2 libjavascriptcoregtk-4.1-0=2.52.6-0ubuntu0.26.04.2 libgtk-3-dev=3.24.52-0ubuntu1 libayatana-appindicator3-dev=0.5.94-1build1 librsvg2-dev=2.61.3+dfsg-3ubuntu0.1 patchelf=0.18.0-1.4build1 file=1:5.46-5build2 ca-certificates=20260816ubuntu0.26.04.2 curl=8.18.0-1ubuntu2.8 xz-utils=5.8.3-1ubuntu0.1"
RUST=1.98.0
RUST_MANIFEST_SHA256=3f7d139b73bbbd0004ef6e58b430831c68cdad2b1f64ee2eb35d54c09199489a
RUST_XZ_SHA256_x86_64=ed8ee2df70909c88cbaf87a6cfa3920dac00b537de12a6abe6906641e0f5952f
RUST_XZ_SHA256_aarch64=ac9283184301aeed06ecc9f5aa4c1be7041e18a1b197b6cb6c5d162d98f566da
# abv <NAME>: the value of appimage-build.sh's top-level NAME= line ("..." or bare, comment cut)
abv(){ sed -n "s/^$1=\"\([^\"]*\)\".*/\1/p; t; s/^$1=\([^ \"]*\).*/\1/p" "$AB" | head -1; }
for n in DIGEST SNAP PKGS RUST RUST_MANIFEST_SHA256 RUST_XZ_SHA256_x86_64 RUST_XZ_SHA256_aarch64; do
  [ "${!n}" = "$(abv "$n")" ] || die "$n here != $n in appimage-build.sh (update linux-test.sh with the release pins)"
done
grep -q "^IMG=docker.io/library/ubuntu@\$DIGEST\$" "$AB" || die "appimage-build.sh IMG is not docker.io/library/ubuntu@\$DIGEST"
[ "$RUST" = "$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$T/rust-toolchain.toml")" ] || die "rust-toolchain.toml channel != $RUST"
grep -q "^RUST_MANIFEST_SHA256=$RUST_MANIFEST_SHA256 " "$T/scripts/supply-chain/rust-toolchain.sh" || die "rust-toolchain.sh manifest sha256 != $RUST_MANIFEST_SHA256"
ARCH=$(uname -m)
case "$ARCH" in x86_64) DA=amd64;; aarch64) DA=arm64;; *) die "unsupported arch $ARCH";; esac
TGT=$ARCH-unknown-linux-gnu
eval "RSHA=\$RUST_XZ_SHA256_$ARCH"
[ "$(sed -n '1s/^# snapshot.ubuntu.com \([0-9TZ]*\) .*/\1/p' "$LOCK")" = "$SNAP" ] || die "$LOCK is not for snapshot $SNAP"
for p in $PKGS; do
  awk -F'\t' -v n="${p%%=*}" -v v="${p#*=}" -v a="$DA" '!/^#/ && $1==n && $2==v && $3==a && $4 ~ /^[0-9a-f]{64}$/{f=1} END{exit !f}' "$LOCK" || die "$p ($DA) not in $LOCK with a sha256"
done
command -v podman >/dev/null || die "no podman"
OUT=$(readlink -f "${OUT:-$T/dist/linux-test}"); rm -rf "$OUT"; mkdir -p "$OUT/cargo-home"
say "tree $T  arch $ARCH  image $IMG  apt snapshot $SNAP  rust $RUST  floors $TOTAL_MIN/$LIB_MIN"

# ---- rust toolchain: manifest sha256 -> per-arch hash -> tarball sha256 (as appimage-build.sh) ----
M=$OUT/channel-rust-$RUST.toml
curl -4 -fsSL --proto '=https' -m 120 -o "$M" "https://static.rust-lang.org/dist/channel-rust-$RUST.toml"
echo "$RUST_MANIFEST_SHA256  $M" | sha256sum -c - || die "rust $RUST dist manifest sha256 mismatch"
MH=$(awk -v H="[pkg.rust.target.$TGT]" '$0==H{f=1;next} /^\[/{f=0} f&&/^xz_hash /{gsub(/"/,"",$3);print $3}' "$M")
MU=$(awk -v H="[pkg.rust.target.$TGT]" '$0==H{f=1;next} /^\[/{f=0} f&&/^xz_url /{gsub(/"/,"",$3);print $3}' "$M")
[ "$MH" = "$RSHA" ] || die "rust manifest xz_hash for $TGT is '$MH', pinned $RSHA"
RT=$OUT/rust-$RUST-$TGT.tar.xz
curl -4 -fsSL --proto '=https' -m 900 -o "$RT" "$MU"
echo "$RSHA  $RT" | sha256sum -c - || die "rust $RUST $TGT tarball sha256 mismatch"
say "rust $RUST $TGT sha256 $RSHA (manifest $RUST_MANIFEST_SHA256)"

podman image exists "$IMG" || podman pull -q "$IMG" >/dev/null || die "pull $IMG"
CA=${HOST_CA_BUNDLE:-/etc/ssl/certs/ca-certificates.crt}; [ -s "$CA" ] || die "no host CA bundle $CA"
rc=0
podman run --rm --network=host -e ARCH="$ARCH" -e TGT="$TGT" -e RUST="$RUST" -e SNAP="$SNAP" -e PKGS="$PKGS" -e IMGREF="$IMG" -e DA="$DA" \
  -v "$CA":/host-ca.crt:ro -v "$LOCK":/snapshot.lock:ro -v "$T":/w -v "$OUT":/out -v "$RT":/rust.tar.xz:ro -w /w "$IMG" bash -euo pipefail -c '
  say(){ echo "[$(date +%H:%M:%S)] [linux-test/resolute] $*"; }
  say "ubuntu $(. /etc/os-release; echo $VERSION_ID $VERSION_CODENAME) $(uname -m)"
  rm -f /etc/apt/sources.list /etc/apt/sources.list.d/* /etc/apt/apt.conf.d/docker-clean
  printf "Types: deb\nURIs: https://snapshot.ubuntu.com/ubuntu/%s/\nSuites: resolute resolute-updates resolute-security\nComponents: main universe\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\nCheck-Valid-Until: no\n" "$SNAP" > /etc/apt/sources.list.d/snapshot.sources
  export DEBIAN_FRONTEND=noninteractive
  echo "Acquire::https::CAInfo \"/host-ca.crt\";" > /etc/apt/apt.conf.d/99host-ca
  apt-get -q update > /out/apt.log 2>&1 || { cat /out/apt.log; exit 1; }
  if grep -E "^(Err:|W:|E:|Error:|Warning:)" /out/apt.log; then say "FAIL: apt update against the snapshot was not clean"; exit 1; fi
  apt-get -y -q --no-install-recommends upgrade >> /out/apt.log 2>&1 || { tail -30 /out/apt.log; exit 1; }
  apt-get -y -q --no-install-recommends install $PKGS >> /out/apt.log 2>&1 || { tail -30 /out/apt.log; exit 1; }
  for p in $PKGS; do n=${p%%=*}; want=${p#*=}; got=$(dpkg-query -W -f="\${Version}" "$n"); [ "$got" = "$want" ] || { say "FAIL: $n is $got, pinned $want"; exit 1; }; done
  for p in $PKGS; do n=${p%%=*}; want=$(awk -F"\t" -v n="$n" -v v="${p#*=}" -v a="$DA" "!/^#/ && \$1==n && \$2==v && \$3==a {print \$4}" /snapshot.lock)
    f=$(ls /var/cache/apt/archives/${n}_*.deb 2>/dev/null | head -1 || true)
    if [ -n "$f" ]; then got=$(sha256sum "$f" | cut -d" " -f1)
    else got=$(apt-cache show --no-all-versions "$n=${p#*=}" 2>/dev/null | sed -n "s/^SHA256: //p" | head -1); fi
    [ -n "$want" ] && [ "$got" = "$want" ] || { say "FAIL: $n $DA deb sha256 ${got:-none} != lock ${want:-none}"; exit 1; }; done
  say "lock: $(echo $PKGS | wc -w) requested .debs == appimage-snapshot.lock.tsv ($DA)"
  dpkg-query -W -f="\${Package}\t\${Version}\t\${Architecture}\n" | LC_ALL=C sort > /out/pkgset.tsv
  say "pkgset: $(wc -l < /out/pkgset.tsv) packages, sha256 $(sha256sum /out/pkgset.tsv | cut -d" " -f1)"
  mkdir /tmp/rust && tar -xJf /rust.tar.xz -C /tmp/rust --strip-components=1
  /tmp/rust/install.sh --prefix=/opt/rust --components=rustc,cargo,rust-std-$TGT >/dev/null
  export PATH=/opt/rust/bin:$PATH CARGO_HOME=/out/cargo-home CARGO_TARGET_DIR=/out/target TMPDIR=/tmp CARGO_TERM_COLOR=never
  rustc --version | grep -q "^rustc $RUST " || { say "FAIL: $(rustc --version) != $RUST"; exit 1; }
  cd /w/src-tauri
  say "cargo test --locked --no-fail-fast"
  trc=0; cargo test --locked --no-fail-fast > /out/cargo-test.log 2>&1 || trc=$?
  echo "$trc" > /out/test.rc; say "cargo test rc=$trc"
  say "cargo build --locked --profile devfast"
  brc=0; cargo build --locked --profile devfast > /out/cargo-build.log 2>&1 || brc=$?
  echo "$brc" > /out/build.rc; say "cargo build rc=$brc"; tail -3 /out/cargo-build.log
  [ "$brc" = 0 ] && ls -l /out/target/devfast/opensidian && sha256sum /out/target/devfast/opensidian || true
  rm -rf /out/target /out/cargo-home   # nothing of this run survives it
' || rc=$?
[ "$rc" = 0 ] || die "container stage rc=$rc"
L=$OUT/cargo-test.log; [ -s "$L" ] || die "no cargo test log"
TRC=$(cat "$OUT/test.rc"); BRC=$(cat "$OUT/build.rc")
# counts: sum of "N passed" over every "test result:" line; lib = the result right after "Running unittests src/lib.rs"
TOTAL=$(awk '/^test result:/{for(i=1;i<=NF;i++) if($(i+1)=="passed;") s+=$i} END{print s+0}' "$L")
FAILED=$(awk '/^test result:/{for(i=1;i<=NF;i++) if($(i+1)=="failed;") s+=$i} END{print s+0}' "$L")
LIB=$(awk '/Running unittests src\/lib.rs/{f=1;next} f&&/^test result:/{for(i=1;i<=NF;i++) if($(i+1)=="passed;") print $i; exit}' "$L")
LIB=${LIB:-0}
{ echo "arch=$ARCH image=$IMG snapshot=$SNAP rust=$RUST"
  echo "cargo_test_rc=$TRC passed_total=$TOTAL failed_total=$FAILED passed_lib=$LIB floors=$TOTAL_MIN/$LIB_MIN"
  echo "cargo_build_devfast_rc=$BRC"
  grep -E '^test .* FAILED$|^failures:$|panicked at' "$L" | head -40 || true; } | tee "$OUT/summary.txt"
grep -E '^test result:|Running ' "$L" | sed 's/^/    /' | head -80
ok=1
[ "$TRC" = 0 ] || { say "FAIL: cargo test rc=$TRC ($FAILED failed)"; ok=0; }
[ "$TOTAL" -ge "$TOTAL_MIN" ] || { say "FAIL: $TOTAL tests passed < floor $TOTAL_MIN"; ok=0; }
[ "$LIB" -ge "$LIB_MIN" ] || { say "FAIL: lib $LIB passed < floor $LIB_MIN"; ok=0; }
[ "$BRC" = 0 ] || { say "FAIL: cargo build --profile devfast rc=$BRC"; tail -40 "$OUT/cargo-build.log"; ok=0; }
[ "$ok" = 1 ] || exit 1
say "PASS: $TOTAL passed ($LIB lib), 0 failed, devfast build ok"
