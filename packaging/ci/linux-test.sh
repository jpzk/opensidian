#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# LINUX PR TEST STAGE (.github/workflows/linux.yml): the full cargo test + a build of src-tauri,
# inside the SAME pinned build environment the release AppImages are built in.
#   linux-test.sh <opensidian-tree>        (needs podman + curl on the host; host arch = uname -m)
# Env = packaging/ci/fedora-env.sh (fedport 2026-11-28), exactly as packaging/ci/appimage-build.sh uses
#   it: the digest-pinned Fedora 44 OCI image + EVERY rpm of packaging/ci/fedora-pkgset.lock.tsv
#   (sha256 + rpm -K against the image's Fedora 44 key, rpm -U, no repo, installed set == lock).
#   One lock, one script: this test env cannot drift from the release build env.
# Rust: literal pins here (pin-check.sh checks them statically) that must EQUAL appimage-build.sh's:
#   version + dist manifest sha256 (= scripts/supply-chain/rust-toolchain.sh) -> per-arch xz_hash
#   == pin -> tarball sha256 == pin; installed rustc must be the pinned version.
# Runs:  cargo test --locked --no-fail-fast  (src-tauri, = box T0 scope), then
#        cargo build --locked --profile devfast  (a build that does not reuse the test profile).
# Gate:  cargo test rc must be 0 AND the passed counts must reach the floors (a run that silently
#        lost tests fails): TOTAL_MIN (all test binaries + doctests) and LIB_MIN (the crate unittests: the src/main.rs test binary).
#        Defaults = the coordinator box count for develop 3b46919 (2026-11-23): 669 total / 592 lib.
# Output: $OUT (default <tree>/dist/linux-test) cargo-test.log, cargo-build.log, rpm.log,
#         pkgset.tsv, summary.txt. Nothing is cached: the cargo home lives in $OUT/cargo-home
#         (fresh per run, never restored from or saved to any cache).
set -euo pipefail
T=$(readlink -f "${1:?tree}")
say(){ echo "[$(date +%H:%M:%S)] [linux-test] $*"; }
die(){ say "FAIL: $*"; exit 1; }
TOTAL_MIN=${TOTAL_MIN:-669} LIB_MIN=${LIB_MIN:-592}
case "$TOTAL_MIN$LIB_MIN" in *[!0-9]*) die "TOTAL_MIN/LIB_MIN must be integers";; esac
HERE=$(cd "$(dirname "$0")" && pwd)
AB=$HERE/appimage-build.sh; FE=$HERE/fedora-env.sh
[ -s "$AB" ] && [ -s "$FE" ] || die "missing $AB or $FE"
RUST=1.98.0
RUST_MANIFEST_SHA256=3f7d139b73bbbd0004ef6e58b430831c68cdad2b1f64ee2eb35d54c09199489a
RUST_XZ_SHA256_x86_64=ed8ee2df70909c88cbaf87a6cfa3920dac00b537de12a6abe6906641e0f5952f
RUST_XZ_SHA256_aarch64=ac9283184301aeed06ecc9f5aa4c1be7041e18a1b197b6cb6c5d162d98f566da
# abv <NAME>: the value of appimage-build.sh's top-level NAME= line ("..." or bare, comment cut)
abv(){ sed -n "s/^$1=\"\([^\"]*\)\".*/\1/p; t; s/^$1=\([^ \"]*\).*/\1/p" "$AB" | head -1; }
for n in RUST RUST_MANIFEST_SHA256 RUST_XZ_SHA256_x86_64 RUST_XZ_SHA256_aarch64; do
  [ "${!n}" = "$(abv "$n")" ] || die "$n here != $n in appimage-build.sh (update linux-test.sh with the release pins)"
done
grep -q 'bash "$T/packaging/ci/fedora-env.sh" install' "$AB" || die "appimage-build.sh does not build in fedora-env.sh (one env for both)"
[ "$RUST" = "$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$T/rust-toolchain.toml")" ] || die "rust-toolchain.toml channel != $RUST"
grep -q "^RUST_MANIFEST_SHA256=$RUST_MANIFEST_SHA256 " "$T/scripts/supply-chain/rust-toolchain.sh" || die "rust-toolchain.sh manifest sha256 != $RUST_MANIFEST_SHA256"
ARCH=$(uname -m)
case "$ARCH" in x86_64|aarch64) ;; *) die "unsupported arch $ARCH";; esac
TGT=$ARCH-unknown-linux-gnu
eval "RSHA=\$RUST_XZ_SHA256_$ARCH"
command -v podman >/dev/null || die "no podman"
OUT=$(readlink -f "${OUT:-$T/dist/linux-test}"); rm -rf "$OUT"; mkdir -p "$OUT/cargo-home"
IMG=$(bash "$T/packaging/ci/fedora-env.sh" image) || die "fedora image"   # literal path: pin-check var_pinned
RPMS=${FEDORA_RPM_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/opensidian-appimage/rpms-$ARCH}
bash "$FE" fetch "$RPMS" || die "fedora-env.sh fetch"
say "tree $T  arch $ARCH  image $IMG  lock $(sha256sum "$HERE/fedora-pkgset.lock.tsv" | cut -d' ' -f1)  rust $RUST  floors $TOTAL_MIN/$LIB_MIN"

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

rc=0
podman run --rm --network=host -e ARCH="$ARCH" -e TGT="$TGT" -e RUST="$RUST" -e IMGREF="$IMG" \
  -v "$RPMS":/rpms:ro -v "$T":/w -v "$OUT":/out -v "$RT":/rust.tar.xz:ro -w /w "$IMG" bash -euo pipefail -c '
  say(){ echo "[$(date +%H:%M:%S)] [linux-test/fc44] $*"; }
  say "$(cat /etc/fedora-release) $(uname -m)"
  bash packaging/ci/fedora-env.sh install /rpms /out/pkgset.tsv > /out/rpm.log 2>&1 || { cat /out/rpm.log; exit 1; }
  tail -3 /out/rpm.log
  say "pkgset: $(grep -vc "^#" /out/pkgset.tsv) packages, sha256 $(sha256sum /out/pkgset.tsv | cut -d" " -f1)"
  mkdir /tmp/rust && tar -xJf /rust.tar.xz -C /tmp/rust --strip-components=1
  /tmp/rust/install.sh --prefix=/opt/rust --components=rustc,cargo,rust-std-$TGT >/dev/null
  export PATH=/opt/rust/bin:$PATH CARGO_HOME=/out/cargo-home CARGO_TARGET_DIR=/out/target TMPDIR=/tmp CARGO_TERM_COLOR=never
  rustc --version | grep -q "^rustc $RUST " || { say "FAIL: $(rustc --version) != $RUST"; exit 1; }
  # cargo test runs UNPRIVILEGED: several tests pin non-root semantics (a 0555 vault refuses
  # writes, /root is not the user home) and fail as container root; the build stays as root
  useradd -m -U -s /bin/bash tst
  mkdir -p /out/cargo-home /out/target && chown -R tst:tst /w /out
  cd /w/src-tauri
  say "cargo test --locked --no-fail-fast (as $(id tst))"
  trc=0; runuser -u tst -- env HOME=/home/tst PATH="$PATH" CARGO_HOME=/out/cargo-home CARGO_TARGET_DIR=/out/target TMPDIR=/tmp CARGO_TERM_COLOR=never cargo test --locked --no-fail-fast > /out/cargo-test.log 2>&1 || trc=$?
  chown -R 0:0 /w /out
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
# counts: per test binary (each "Running ..."/"Doc-tests ..." section) only its LAST "test result:" line
# is the binary's own summary; earlier ones in a section are re-exec'd test subprocesses (filtered
# single-test runs) whose output lands in the parent's log. unit = the src/main.rs unittests binary
# (the crate's suite lives there; src/lib.rs holds only the few shared modules)
sect() { awk '/^ *(Running|Doc-tests) /{if(n!="")print n" "p; n=$0; p=0; next}
  /^test result:/{p=0; for(i=1;i<=NF;i++) if($(i+1)=="passed;") p=$i}
  END{if(n!="")print n" "p}' "$1"; }
TOTAL=$(sect "$L" | awk '{s+=$NF} END{print s+0}')
FAILED=$(awk '/^test result:/{for(i=1;i<=NF;i++) if($(i+1)=="failed;") s+=$i} END{print s+0}' "$L")
LIB=$(sect "$L" | awk '/Running unittests src\/main.rs /{print $NF; exit}')
LIB=${LIB:-0}
{ echo "arch=$ARCH image=$IMG lock=$(sha256sum "$HERE/fedora-pkgset.lock.tsv" | cut -c1-16) rust=$RUST"
  echo "cargo_test_rc=$TRC passed_total=$TOTAL failed_total=$FAILED passed_unit_main=$LIB floors=$TOTAL_MIN/$LIB_MIN"
  echo "cargo_build_devfast_rc=$BRC"
  grep -E '^test .* FAILED$|^failures:$|panicked at' "$L" | head -40 || true; } | tee "$OUT/summary.txt"
grep -E '^test result:|Running ' "$L" | sed 's/^/    /' | head -80
ok=1
[ "$TRC" = 0 ] || { say "FAIL: cargo test rc=$TRC ($FAILED failed)"; ok=0; }
[ "$TOTAL" -ge "$TOTAL_MIN" ] || { say "FAIL: $TOTAL tests passed < floor $TOTAL_MIN"; ok=0; }
[ "$LIB" -ge "$LIB_MIN" ] || { say "FAIL: unit (src/main.rs) $LIB passed < floor $LIB_MIN"; ok=0; }
[ "$BRC" = 0 ] || { say "FAIL: cargo build --profile devfast rc=$BRC"; tail -40 "$OUT/cargo-build.log"; ok=0; }
[ "$ok" = 1 ] || exit 1
say "PASS: $TOTAL passed ($LIB lib), 0 failed, devfast build ok"
