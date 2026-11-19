#!/bin/bash
# positive: InRelease-style fetch, verified with gpgv against the committed, sha256-pinned keyring, fail closed
set -euo pipefail
die(){ echo "FAIL: $*" >&2; exit 1; }
HERE=$(cd "$(dirname -- "$0")" && pwd)
KR=$HERE/../supply-chain/keys/archive.gpg
W=$(mktemp -d)
curl -4 -fsS -o "$W/InRelease" "https://example.org/dists/x/InRelease" || die "fetch"
gpgv --keyring "$KR" --output "$W/Release" "$W/InRelease" 2>"$W/gpgv" || { cat "$W/gpgv" >&2; die "signature"; }
