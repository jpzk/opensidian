#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vendor-sbom.sh: passes on the repo; fails on an unlisted file and on a flipped byte in each
# vendored file. Scratch copies only (mktemp), the tree is never modified.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
s=$here/vendor-sbom.sh; t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }

out=$("$s") && jq -e '.bomFormat=="CycloneDX" and .specVersion=="1.6"
   and ([.components[].purl] | (index("pkg:npm/mathjax@3.2.2") != null) and (index("pkg:npm/pdfjs-dist@6.3.289") != null)
        and (index("pkg:npm/prismjs@1.30.0") != null) and (index("pkg:npm/moment@2.30.1") != null))
   and ([.components[] | select(.name | startswith("@codemirror/"))] | length) >= 11' <<<"$out" >/dev/null \
  && ok "repo: CycloneDX 1.6 with mathjax/pdfjs-dist/prismjs/moment/codemirror ($(jq '.components|length' <<<"$out") components)" \
  || bad "repo run"

fresh() { rm -rf "$t/v"; cp -a "$root/ui/vendor" "$t/v"; }
flip() { # flip the low bit of the byte at offset $2 of file $1
  local f=$1 off=$2 b
  b=$(od -An -tu1 -j "$off" -N1 "$f" | tr -d ' ')
  printf "$(printf '\\%03o' $(( b ^ 1 )))" | dd of="$f" bs=1 seek="$off" conv=notrunc status=none
}

fresh; echo x >"$t/v/pdfjs/extra.js"
VENDOR_DIR=$t/v "$s" >/dev/null 2>&1 && bad "unlisted file accepted" || ok "unlisted ui/vendor/pdfjs/extra.js -> non-zero"
fresh; echo x >"$t/v/stray.js"
VENDOR_DIR=$t/v "$s" >/dev/null 2>&1 && bad "unlisted top-level file accepted" || ok "unlisted ui/vendor/stray.js -> non-zero"

while IFS= read -r f; do
  case $f in */VENDOR.md|*/SHA256SUMS) continue;; esac
  fresh; sz=$(stat -c %s "$t/v/$f"); flip "$t/v/$f" $(( sz / 2 ))
  VENDOR_DIR=$t/v "$s" >/dev/null 2>&1 && bad "flipped byte in $f accepted" || ok "flipped byte in $f -> non-zero"
done <<EOF_LS
$(cd "$root/ui/vendor" && find . -type f | sed 's|^\./||' | sort)
EOF_LS
exit $fails
