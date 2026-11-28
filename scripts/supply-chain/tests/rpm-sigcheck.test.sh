#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# rpm-sigcheck.sh (PR #369 review: signer pinned to ONE key), offline: rpm/rpmkeys are STUBS that print
# rpm 6.0.x output verbatim (captured from rpm 6.0.2 on a Fedora 44 rpm), so this runs on any runner.
# Red fixtures: package signed by a DIFFERENT (also trusted) key, unsigned, NOKEY, BAD digest,
# keyring with a second key, rpm 4 (short key IDs) -> all must fail closed.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); S=$here/rpm-sigcheck.sh
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
F=36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6 O=2596a99eaab33821893c0a79458ca832957f5868
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
mkdir -p "$t/bin"
cat > "$t/bin/rpm" <<'STUB'
#!/bin/bash
[ "$1" = --version ] && { echo "RPM version ${STUB_RPMV:-6.0.2}"; exit 0; }; exit 9
STUB
cat > "$t/bin/rpmkeys" <<'STUB'
#!/bin/bash
for a; do case "$a" in --list) [ -n "${STUB_KEYS:-}" ] && printf '%s\n' ${STUB_KEYS:-} | sed 's/$/ Some (44) <k@x> public key/'; exit 0 ;;
  --import) exit 0 ;; -Kv) kv=1 ;; esac; f=$a; done
[ -n "${kv:-}" ] || exit 9
echo "$f:"
case "${STUB_SIG:-good}" in
  good)  echo "    Header OpenPGP V4 RSA/SHA256 signature, key fingerprint: 36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6: OK" ;;
  keyid) echo "    Header OpenPGP V4 RSA/SHA256 signature, key ID dbfcf71c6d9f90a6: OK" ;;
  other) echo "    Header OpenPGP V4 RSA/SHA256 signature, key fingerprint: 2596a99eaab33821893c0a79458ca832957f5868: OK" ;;
  mixed) echo "    Header OpenPGP V4 RSA/SHA256 signature, key fingerprint: 36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6: OK"
         echo "    Header OpenPGP V4 RSA/SHA256 signature, key fingerprint: 2596a99eaab33821893c0a79458ca832957f5868: OK" ;;
  nokey) echo "    Header OpenPGP V4 RSA/SHA256 signature, key ID dbfcf71c6d9f90a6: NOKEY"; s=1 ;;
  none)  ;;
  short) echo "    Header V4 RSA/SHA256 Signature, key ID 6d9f90a6: OK" ;;
esac
echo "    Header SHA256 digest: ${STUB_DIG:-OK}"
echo "    Payload SHA256 digest: OK"
exit ${s:-0}
STUB
chmod +x "$t/bin/rpm" "$t/bin/rpmkeys"; export PATH=$t/bin:$PATH
touch "$t/a.rpm" "$t/k.asc"
run() { local want=$1 name=$2; shift 2; if env "$@" >"$t/out" 2>&1; then rc=0; else rc=$?; fi
  if [ "$want" = ok ] && [ $rc = 0 ]; then ok "$name"; elif [ "$want" = red ] && [ $rc != 0 ]; then ok "$name (red: $(tail -1 "$t/out"))"; else bad "$name rc=$rc"; cat "$t/out"; fi; }
run ok  "signed by pinned key (fingerprint)"           STUB_KEYS=$F bash "$S" check "$t/db" "$F" "$t/a.rpm"
run ok  "signed by pinned key (long key ID)"           STUB_KEYS=$F STUB_SIG=keyid bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "signed by a DIFFERENT trusted key"            STUB_KEYS="$F $O" STUB_SIG=other bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "signed by a different key, keyring only ours" STUB_KEYS=$F STUB_SIG=other bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "second signature by another key"             STUB_KEYS=$F STUB_SIG=mixed bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "NOKEY"                                        STUB_KEYS=$F STUB_SIG=nokey bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "unsigned (digests only)"                      STUB_KEYS=$F STUB_SIG=none bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "BAD header digest"                            STUB_KEYS=$F STUB_DIG=BAD bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "rpm 4 short key ID output"                    STUB_KEYS=$F STUB_SIG=short bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "rpm major != 6"                               STUB_KEYS=$F STUB_RPMV=4.19.1 bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "keyring trusts a second key"                  STUB_KEYS="$F $O" bash "$S" check "$t/db" "$F" "$t/a.rpm"
run red "keyring empty"                                STUB_KEYS= bash "$S" keys "$F"
run ok  "keyring import -> exactly the pinned key"     STUB_KEYS=$F bash "$S" keyring "$t/k.asc" "$F" "$t/kdb"
mkdir -p "$t/kdb2"; touch "$t/kdb2/rpmdb.sqlite"
run red "keyring dir not empty"                        STUB_KEYS=$F bash "$S" keyring "$t/k.asc" "$F" "$t/kdb2"
[ $fails = 0 ] && echo "rpm-sigcheck.test: all ok" || { echo "rpm-sigcheck.test: FAILED"; exit 1; }
