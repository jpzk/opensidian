#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# openssl-fips.sh — build OpenSSL 3.5.9 core + FIPS provider (CMVP #5586, docs/sync/cmvp.md) for the
# sync CLIs (.github/workflows/sync-cli.yml). Same recipe as the build box (deps.md M2a), except
# no-shared: libcrypto is linked STATICALLY into opensidian-sync-server / opensidian-syncd /
# openssl; the FIPS provider stays the separate, loadable lib/ossl-modules/fips.so by design.
#   openssl-fips.sh PREFIX WORKDIR
# 1. openssl-3.5.9.tar.gz  sha256 pinned (OSSL_SHA256)
# 2. its .asc               sha256 pinned (OSSL_ASC_SHA256) AND gpgv against the COMMITTED keyring
#                           supply-chain/keys/openssl-release-keyring.gpg (sha256 re-checked against
#                           supply-chain/keyrings.tsv first), VALIDSIG must chain to primary OSSL_FPR
# 3. ./Configure enable-fips no-shared, make, make install_sw install_fips
# 4. for the CI tests on THIS host: fipsinstall -pedantic -> PREFIX/ssl/fipsmodule.cnf + openssl.cnf
#    (approved-only default properties); the shipped tarball does NOT carry these (fipsinstall
#    must run on the target machine: opensidian-sync-fipsinstall).
# Fails closed on any mismatch. Prints the sha256 of fips.so and the openssl binary.
set -euo pipefail
OSSL_VER=3.5.9
OSSL_SHA256=603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a      # release 2026-09-29
OSSL_ASC_SHA256=bdccf2057a7c4ae377b40e832cb8c632ae8b532d4fb64cc7b843066eee748f89  # signature file
OSSL_FPR=B146647E45A7B33947AB226B2A2C87D161692D40                                 # OpenSSL release key (primary)
URL=https://github.com/openssl/openssl/releases/download/openssl-$OSSL_VER/openssl-$OSSL_VER.tar.gz
HERE=$(cd "$(dirname -- "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
KR=$ROOT/supply-chain/keys/openssl-release-keyring.gpg; KT=$ROOT/supply-chain/keyrings.tsv
die() { echo "[openssl-fips] FAIL: $*" >&2; exit 1; }
say() { echo "[openssl-fips $(date -u +%H:%M:%S)] $*"; }
[ $# -eq 2 ] || die "usage: openssl-fips.sh PREFIX WORKDIR"
P=$1; W=$2; mkdir -p "$W"; W=$(cd "$W" && pwd); mkdir -p "$P"; P=$(cd "$P" && pwd)
for t in curl gpgv sha256sum perl make cc tar; do command -v $t >/dev/null || die "missing $t"; done
kw=$(awk -F'\t' '$1=="supply-chain/keys/openssl-release-keyring.gpg"{print $2}' "$KT")
[[ "$kw" =~ ^[0-9a-f]{64}$ ]] || die "no sha256 pin for the keyring in $KT"
[ "$(sha256sum < "$KR" | cut -d' ' -f1)" = "$kw" ] || die "keyring $KR sha256 != $KT"
T=$W/openssl-$OSSL_VER.tar.gz
say "fetch $URL"
curl -4 -fsSL --proto '=https' --retry 3 -m 300 -o "$T" "$URL" || die "fetch tarball"
echo "$OSSL_SHA256  $T" | sha256sum -c - || die "tarball sha256"
curl -4 -fsSL --proto '=https' --retry 3 -m 60 -o "$T.asc" "$URL.asc" || die "fetch signature"
echo "$OSSL_ASC_SHA256  $T.asc" | sha256sum -c - || die "signature file sha256"
gpgv --status-fd 3 --keyring "$KR" "$T.asc" "$T" 3>"$W/gpgv.status" 2>"$W/gpgv.log" || { cat "$W/gpgv.log" >&2; die "gpgv"; }
cat "$W/gpgv.log"
grep -Eq "^\[GNUPG:\] VALIDSIG [0-9A-F]{40} .* $OSSL_FPR\$" "$W/gpgv.status" || die "signature does not chain to primary $OSSL_FPR"
say "SIGNATURE OK: $(grep '^\[GNUPG:\] VALIDSIG' "$W/gpgv.status" | cut -d' ' -f3) -> primary $OSSL_FPR; tarball sha256 $OSSL_SHA256"
rm -rf "$W/src"; mkdir -p "$W/src"; tar xzf "$T" -C "$W/src"; cd "$W/src/openssl-$OSSL_VER"
say "host $(uname -srm); $(. /etc/os-release && echo "$PRETTY_NAME"); perl $(perl -e 'print $^V'); $(cc --version | head -1)"
say "./Configure enable-fips no-shared --prefix=$P --openssldir=$P/ssl --libdir=lib"
./Configure enable-fips no-shared --prefix="$P" --openssldir="$P/ssl" --libdir=lib > "$W/configure.log" 2>&1 || { tail -40 "$W/configure.log"; die configure; }
say "make -j$(nproc)"
make -j"$(nproc)" > "$W/make.log" 2>&1 || { tail -60 "$W/make.log"; die make; }
say "make install_sw install_fips"
make install_sw install_fips > "$W/install.log" 2>&1 || { tail -40 "$W/install.log"; die install; }
O=$P/bin/openssl; M=$P/lib/ossl-modules/fips.so
[ -f "$M" ] || die "no $M"; [ -f "$P/lib/libcrypto.a" ] || die "no static libcrypto"
ls "$P"/lib/libcrypto.so* >/dev/null 2>&1 && die "shared libcrypto was built (expected no-shared)"
say "fipsinstall -pedantic (this host only, for the tests)"
OPENSSL_CONF=/dev/null "$O" fipsinstall -pedantic -module "$M" -out "$P/ssl/fipsmodule.cnf"
cat > "$P/ssl/openssl.cnf" <<CEOF
# sync CI: OpenSSL $OSSL_VER with the FIPS provider (CMVP #5586) and approved-only default properties
config_diagnostics = 1
openssl_conf = openssl_init
.include $P/ssl/fipsmodule.cnf
[openssl_init]
providers = provider_sect
alg_section = algorithm_sect
[provider_sect]
fips = fips_sect
base = base_sect
[base_sect]
activate = 1
[algorithm_sect]
default_properties = fips=yes
CEOF
export OPENSSL_CONF=$P/ssl/openssl.cnf OPENSSL_MODULES=$P/lib/ossl-modules
"$O" version -a | head -3
"$O" fipsinstall -pedantic -in "$P/ssl/fipsmodule.cnf" -module "$M" -verify
"$O" list -providers -verbose
printf abc | "$O" dgst -sha256
if printf abc | "$O" dgst -md5 >/dev/null 2>&1; then die "MD5 available under fips=yes"; fi
say "md5 refused under fips=yes (expected)"
cp "$W/src/openssl-$OSSL_VER/LICENSE.txt" "$P/LICENSE.txt"
sha256sum "$M" "$O" "$P/lib/libcrypto.a"
rm -rf "$W/src"
say "done: prefix $P"
