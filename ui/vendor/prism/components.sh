#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Rebuild prism-components.js (#186) from the pinned prismjs 1.30.0 tarball, byte-identical:
#   sh components.sh /path/to/extracted/package      (the tarball's package/ dir; facts + file order in ../vendor.lock.json)
# Every language component of components.json except the four already in prism.js (markup css clike javascript),
# unmodified upstream components/prism-<id>.min.js, in dependency order (require + optional + modify first, GNU tsort),
# each followed by one newline. Check: sha256sum -c SHA256SUMS. Needs jq + coreutils tsort; no network, no npm.
set -eu
P=${1:?usage: components.sh <extracted package dir>}
jq -r '.languages | to_entries[] | select(.key!="meta") | .key as $k
  | ([.value.require, .value.optional, .value.modify] | map(if .==null then [] elif type=="string" then [.] else . end) | add) as $d
  | if ($d|length)==0 then "\($k) \($k)" else ($d[] | "\(.) \($k)") end' "$P/components.json" | LC_ALL=C tsort |
while read -r l; do
  case $l in markup|css|clike|javascript) continue ;; esac
  cat "$P/components/prism-$l.min.js"; printf '\n'
done
