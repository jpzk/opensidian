#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# cli-package.sh — assemble opensidian-sync-<ver>-linux-<arch>.tar.gz (docs/sync/release-cli.md).
#   cli-package.sh VERSION ARCH OSSL_PREFIX BIN_DIR OUT_DIR
# BIN_DIR holds the release opensidian-sync-server + opensidian-syncd (cargo build --release --locked,
# libcrypto static). Layout (one top dir, runs from wherever it is extracted):
#   opensidian-sync-server, opensidian-syncd   sh launchers: point OpenSSL at the bundled FIPS provider
#   opensidian-sync-fipsinstall                run ONCE on the target machine (fipsinstall -pedantic)
#   libexec/{opensidian-sync-server,opensidian-syncd,openssl}   the ELF binaries (libcrypto static)
#   lib/ossl-modules/fips.so                   OpenSSL 3.5.9 FIPS provider (CMVP #5586), separate by design
#   ssl/                                       fipsmodule.cnf + openssl.cnf are written here by fipsinstall
#   licenses/  README.md  BUILDINFO.txt
# Refuses unless every ELF needs no libcrypto/libssl shared object. Writes OUT_DIR/<name>.tar.gz and
# OUT_DIR/glibc-<arch>.txt (max GLIBC_ symbol version per ELF, objdump -T).
set -euo pipefail
die() { echo "[cli-package] FAIL: $*" >&2; exit 1; }
[ $# -eq 5 ] || die "usage: cli-package.sh VERSION ARCH OSSL_PREFIX BIN_DIR OUT_DIR"
V=$1; A=$2; P=$3; B=$4; OUT=$5
HERE=$(cd "$(dirname -- "$0")" && pwd); ROOT=$(cd "$HERE/../.." && pwd)
N=opensidian-sync-$V-linux-$A; mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)
S=$OUT/stage/$N; rm -rf "$OUT/stage"; mkdir -p "$S/libexec" "$S/lib/ossl-modules" "$S/ssl" "$S/licenses"
install -m 0755 "$B/opensidian-sync-server" "$B/opensidian-syncd" "$P/bin/openssl" "$S/libexec/"
install -m 0755 "$P/lib/ossl-modules/fips.so" "$S/lib/ossl-modules/"
for e in "$S"/libexec/* "$S/lib/ossl-modules/fips.so"; do
  if readelf -d "$e" | grep -E 'NEEDED.*lib(crypto|ssl)\.so'; then die "$e links libcrypto/libssl dynamically"; fi
done

launcher() { # $1 = binary name
  cat > "$S/$1" <<LEOF
#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# $1 launcher: runs libexec/$1 against the OpenSSL 3.5.9 FIPS provider bundled in this directory.
# Run ./opensidian-sync-fipsinstall once on this machine first. OPENSIDIAN_SYNC_OPENSSL_CONF overrides
# the bundled openssl.cnf (it must still activate the 3.5.9 fips provider, or $1 refuses to start).
set -eu
D=\$(dirname -- "\$(readlink -f -- "\$0")")
C=\${OPENSIDIAN_SYNC_OPENSSL_CONF:-\$D/ssl/openssl.cnf}
if [ ! -f "\$C" ]; then
  echo "$1: \$C missing: run \$D/opensidian-sync-fipsinstall once on this machine (FIPS module self-test + config)" >&2
  exit 2
fi
OPENSSL_CONF=\$C OPENSSL_MODULES=\$D/lib/ossl-modules
export OPENSSL_CONF OPENSSL_MODULES
exec "\$D/libexec/$1" "\$@"
LEOF
  chmod 0755 "$S/$1"
}
launcher opensidian-sync-server
launcher opensidian-syncd

cat > "$S/opensidian-sync-fipsinstall" <<'FEOF'
#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# opensidian-sync-fipsinstall: run ONCE per machine (and again after moving this directory or
# replacing fips.so). Runs the OpenSSL FIPS provider's self-tests on THIS machine
# (openssl fipsinstall -pedantic) and writes ssl/fipsmodule.cnf + ssl/openssl.cnf with absolute
# paths to this directory. fipsmodule.cnf is per machine: never copy it from another host.
set -eu
D=$(dirname -- "$(readlink -f -- "$0")")
case "$D" in *[[:space:]]*) echo "opensidian-sync-fipsinstall: the path $D contains whitespace; extract somewhere without it" >&2; exit 2;; esac
O=$D/libexec/openssl; M=$D/lib/ossl-modules/fips.so
umask 022
OPENSSL_CONF=/dev/null "$O" fipsinstall -pedantic -module "$M" -out "$D/ssl/fipsmodule.cnf.tmp"
mv -f "$D/ssl/fipsmodule.cnf.tmp" "$D/ssl/fipsmodule.cnf"
cat > "$D/ssl/openssl.cnf.tmp" <<CEOF
# written by opensidian-sync-fipsinstall: OpenSSL 3.5.9 FIPS provider (CMVP #5586), approved-only defaults
config_diagnostics = 1
openssl_conf = openssl_init
.include $D/ssl/fipsmodule.cnf
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
mv -f "$D/ssl/openssl.cnf.tmp" "$D/ssl/openssl.cnf"
OPENSSL_CONF=$D/ssl/openssl.cnf OPENSSL_MODULES=$D/lib/ossl-modules "$O" fipsinstall -pedantic -in "$D/ssl/fipsmodule.cnf" -module "$M" -verify
OPENSSL_CONF=$D/ssl/openssl.cnf OPENSSL_MODULES=$D/lib/ossl-modules "$O" list -providers
echo "opensidian-sync-fipsinstall: OK ($D/ssl/openssl.cnf). The launchers $D/opensidian-syncd and $D/opensidian-sync-server use it."
FEOF
chmod 0755 "$S/opensidian-sync-fipsinstall"

cp "$ROOT/LICENSE" "$S/licenses/LICENSE-GPL-3.0"
cp "$P/LICENSE.txt" "$S/licenses/LICENSE-OpenSSL-Apache-2.0.txt"
cat > "$S/licenses/NOTICE-EFF-wordlist.txt" <<'EEOF'
The 12 backup words of Opensidian Sync are drawn from the EFF large wordlist,
embedded unmodified in opensidian-syncd (crates/sync-crypto/wordlists/eff_large_wordlist.txt,
sha256 addd35536511597a02fa0a9ff1e5284677b8883b83e986e43f15a3db996b903e).

EFF large wordlist by the Electronic Frontier Foundation, licensed under
Creative Commons Attribution 3.0 United States (CC BY 3.0 US),
https://creativecommons.org/licenses/by/3.0/us/ — (c) Electronic Frontier Foundation.
Source: https://www.eff.org/files/2016/07/18/eff_large_wordlist.txt (https://www.eff.org/dice)
EEOF
{ echo "Rust crates statically linked into opensidian-sync-server and opensidian-syncd"
  echo "(cargo tree -e normal, name version licence). Licence texts: each crate's source on crates.io;"
  echo "the vendored rustls-webpki carries its LICENSE in the Opensidian source tree (src-tauri/vendor)."
  echo
  (cd "$ROOT/src-tauri" && cargo tree --locked --offline -e normal -p syncd -p sync-server --prefix none --format '{p} {l}') \
    | sed 's/ (\*)$//; s/ (proc-macro)//' | sort -u
} > "$S/licenses/CRATES.txt"

glibc_max() { objdump -T "$1" | grep -o 'GLIBC_[0-9][0-9.]*' | sort -uV | tail -1; }
{ for e in "$S"/libexec/* "$S/lib/ossl-modules/fips.so"; do printf '%s %s\n' "${e#"$S"/}" "$(glibc_max "$e")"; done
  printf 'max %s\n' "$(for e in "$S"/libexec/* "$S/lib/ossl-modules/fips.so"; do glibc_max "$e"; done | sort -V | tail -1)"
} > "$OUT/glibc-$A.txt"
GL=$(sed -n 's/^max //p' "$OUT/glibc-$A.txt")

{ echo "name        $N"
  echo "version     $V"
  echo "arch        $A"
  echo "commit      $(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
  echo "rustc       $(rustc -V)"
  echo "openssl     $(OPENSSL_CONF=/dev/null "$P/bin/openssl" version) (libcrypto static, no-shared, enable-fips)"
  echo "glibc       >= ${GL#GLIBC_} (max GLIBC_ symbol over every ELF, objdump -T)"
  echo "built on    $(. /etc/os-release && echo "$PRETTY_NAME") $(uname -m)"
  echo
  echo "sha256:"
  (cd "$S" && sha256sum libexec/* lib/ossl-modules/fips.so)
} > "$S/BUILDINFO.txt"

sed -e "s/@VERSION@/$V/g" -e "s/@ARCH@/$A/g" -e "s/@GLIBC@/${GL#GLIBC_}/g" "$HERE/cli-README.md.in" > "$S/README.md"

# reproducible-ish tarball: fixed owner, sorted, mtime = commit time
MT=$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || date +%s)
tar --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$MT" -C "$OUT/stage" -czf "$OUT/$N.tar.gz" "$N"
rm -rf "$OUT/stage"
echo "[cli-package] $OUT/$N.tar.gz"; tar tzvf "$OUT/$N.tar.gz"; cat "$OUT/glibc-$A.txt"
