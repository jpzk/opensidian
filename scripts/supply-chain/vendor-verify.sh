#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# vendor-verify.sh — prove ui/vendor/vendor.lock.json against the vendored BYTES and the npm
# registry (issue #188). bash + jq + curl + coreutils + tar only; no npm, no node.
#
# Source of truth: ui/vendor/vendor.lock.json. VENDOR.md files are prose and are NEVER read here.
# For every library in the lock:
#   a) GET <registry>/<name>/<version>: .version, .dist.integrity and .dist.tarball must equal the lock;
#   b) download the lock's tarball URL, its sha512 must equal the lock integrity; extract with tar;
#      every vendored file's sha256 must equal the lock sha256 AND the file at tarball_path
#      (local_header_lines=N: the vendored file is N lines of our own header + the upstream bytes;
#      the whole file must also equal vendored_sha256);
#   c) every embedded_version rule (ERE in the file's bytes) must match and yield exactly the lock
#      version (pdfjsVersion, moment header + .version, MathJax VERSION; prism has none);
#   d) every file under ui/vendor must be a lock file, a bundle, or listed in allowed_unlocked
#      (exact paths); symlinks are refused; any VENDOR.md (retired prose notes) is an error;
#   e) lock published == registry time[version], and published is >= 42 days (six weeks) before
#      added (the date the bytes landed in git).
# For every bundle (CodeMirror cm6.js, an esbuild bundle no upstream file matches): the bundle
#   sha256 must equal the lock, its committed package-lock.json must exist, its non-dev packages
#   must be exactly the lock's bundles[].packages (name, version, integrity), every package must
#   have version/integrity/resolved equal to the registry's version/dist.integrity/dist.tarball,
#   and every shipped package's publish date must match the registry and obey the six-week rule.
#
# Exit 0 = all proven; 1 = any mismatch (all problems are listed); 2 = usage.
# Usage: scripts/supply-chain/vendor-verify.sh
#   VENDOR_DIR=<dir>   vendored tree (default ui/vendor); LOCK=<file> (default $VENDOR_DIR/vendor.lock.json)
#   NPM_REGISTRY=<url> (default https://registry.npmjs.org)
set -euo pipefail
export LC_ALL=C

root=$(cd "$(dirname "$0")/../.." && pwd)
vd=${VENDOR_DIR:-$root/ui/vendor}
lock=${LOCK:-$vd/vendor.lock.json}
reg=${NPM_REGISTRY:-https://registry.npmjs.org}
for t in jq curl tar sha256sum sha512sum base64 od comm date; do
  command -v "$t" >/dev/null || { echo "vendor-verify: $t required" >&2; exit 2; }
done
[ -d "$vd" ] || { echo "vendor-verify: no such dir: $vd" >&2; exit 2; }
[ -f "$lock" ] || { echo "vendor-verify: no lock: $lock" >&2; exit 1; }

nerr=0
err() { echo "vendor-verify: FAIL $*" >&2; nerr=$((nerr + 1)); }
ok() { echo "vendor-verify: ok   $*"; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
covered=$work/covered; : >"$covered"

sha256() { sha256sum "$1" | cut -d' ' -f1; }
# "sha512-<base64>" -> lowercase hex ("" if malformed)
sri_hex() {
  case $1 in sha512-*) ;; *) return 0 ;; esac
  printf '%s' "${1#sha512-}" | base64 -d 2>/dev/null | od -An -v -tx1 | tr -d ' \n'
}
purl_of() { local n=$1; printf 'pkg:npm/%s@%s' "${n/#@/%40}" "$2"; }
# registry packument -> $work/meta.json = versions[v] + {time: time[v]} (fails if v is absent)
meta() {
  curl -4 -fsS --retry 3 -m 120 "$reg/$1" -o "$work/pack.json" &&
    jq -e --arg v "$2" '.versions[$v] as $d | select($d != null) | $d + {time: .time[$v]}' \
      "$work/pack.json" >"$work/meta.json"
}
# whole days from the date of ISO timestamp $1 to the date of $2
days() { echo $((($(date -ud "${2:0:10}" +%s) - $(date -ud "${1:0:10}" +%s)) / 86400)); }
MINDAYS=42 # six-week rule

jq -e '.lockfileVersion == 1 and (.libraries|type=="array") and (.bundles|type=="array")
       and (.allowed_unlocked|type=="array")' "$lock" >/dev/null \
  || { echo "vendor-verify: FAIL $lock: not a lockfileVersion 1 vendor lock" >&2; exit 1; }

nlib=$(jq '.libraries|length' "$lock")
for ((i = 0; i < nlib; i++)); do
  L=$(jq -c ".libraries[$i]" "$lock")
  name=$(jq -r .name <<<"$L"); ver=$(jq -r .version <<<"$L")
  integ=$(jq -r .integrity <<<"$L"); url=$(jq -r .tarball <<<"$L")
  id="$name@$ver"; bad=$nerr
  [ "$(jq -r .purl <<<"$L")" = "$(purl_of "$name" "$ver")" ] || err "$id: purl $(jq -r .purl <<<"$L") != $(purl_of "$name" "$ver")"
  want=$(sri_hex "$integ"); [ ${#want} -eq 128 ] || err "$id: lock integrity is not a sha512 SRI"

  # a) registry metadata
  if meta "$name" "$ver"; then
    rv=$(jq -r .version "$work/meta.json"); ri=$(jq -r .dist.integrity "$work/meta.json"); rt=$(jq -r .dist.tarball "$work/meta.json")
    [ "$rv" = "$ver" ] || err "$id: registry version is $rv"
    [ "$ri" = "$integ" ] || err "$id: registry dist.integrity $ri != lock $integ"
    [ "$rt" = "$url" ] || err "$id: registry dist.tarball $rt != lock $url"
    # e) publish date == registry time[version], and >= 6 weeks before the bytes were added
    rtime=$(jq -r '.time // ""' "$work/meta.json"); pub=$(jq -r '.published // ""' <<<"$L"); add=$(jq -r '.added // ""' <<<"$L")
    [ -n "$pub" ] && [ "$pub" = "$rtime" ] || err "$id: lock published '$pub' != registry time[$ver] '$rtime'"
    if [ -n "$pub" ] && [ -n "$add" ] && d=$(days "$pub" "$add" 2>/dev/null); then
      [ "$d" -ge "$MINDAYS" ] || err "$id: published $pub only $d day(s) before added $add (six-week rule: >= $MINDAYS)"
    else err "$id: lock needs published + added dates"; fi
  else
    err "$id: no version $ver in registry packument $reg/$name"
  fi

  # b) tarball bytes
  tgz=$work/lib$i.tgz; x=$work/lib$i
  if ! curl -4 -fsS --retry 3 -m 300 "$url" -o "$tgz"; then err "$id: download $url failed"; continue; fi
  got=$(sha512sum "$tgz" | cut -d' ' -f1)
  if [ "$got" != "$want" ]; then err "$id: tarball sha512 $got != lock integrity"; continue; fi
  mkdir -p "$x"; tar -xzf "$tgz" -C "$x" --no-same-owner --no-same-permissions || { err "$id: tar failed"; continue; }
  nf=$(jq '.files|length' <<<"$L")
  for ((j = 0; j < nf; j++)); do
    F=$(jq -c ".files[$j]" <<<"$L")
    p=$(jq -r .path <<<"$F"); tp=$(jq -r .tarball_path <<<"$F"); s=$(jq -r .sha256 <<<"$F")
    hl=$(jq -r '.local_header_lines // 0' <<<"$F")
    printf '%s\n' "$p" >>"$covered"
    case $tp in /* | *..*) err "$id: unsafe tarball_path $tp"; continue ;; esac
    [ -f "$vd/$p" ] && [ ! -L "$vd/$p" ] || { err "$id: vendored file $p missing"; continue; }
    [ -f "$x/$tp" ] || { err "$id: $tp not in tarball"; continue; }
    up=$(sha256 "$x/$tp")
    [ "$up" = "$s" ] || err "$id: lock sha256 of $tp ($s) != tarball ($up)"
    if [ "$hl" -gt 0 ]; then
      body=$(tail -n +"$((hl + 1))" "$vd/$p" | sha256sum | cut -d' ' -f1)
      [ "$body" = "$up" ] || err "$id: $p (after $hl header lines) differs from tarball $tp"
      [ "$(sha256 "$vd/$p")" = "$(jq -r .vendored_sha256 <<<"$F")" ] || err "$id: $p != lock vendored_sha256"
    else
      [ "$(sha256 "$vd/$p")" = "$up" ] || err "$id: $p differs from tarball $tp"
    fi
  done

  # c) embedded version strings
  nr=$(jq '.embedded_version|length' <<<"$L")
  for ((j = 0; j < nr; j++)); do
    p=$(jq -r ".embedded_version[$j].path" <<<"$L"); re=$(jq -r ".embedded_version[$j].regex" <<<"$L")
    [ -f "$vd/$p" ] || { err "$id: embedded_version file $p missing"; continue; }
    vs=$(grep -oE -- "$re" "$vd/$p" | grep -oE '[0-9]+(\.[0-9]+)+' | sort -u | tr '\n' ' ' || true)
    [ "$vs" = "$ver " ] || err "$id: $p embeds version '${vs% }' (rule /$re/), lock says $ver"
  done
  [ "$bad" = "$nerr" ] && ok "$id: registry, tarball sha512, $nf file(s), $nr version string(s)"
done

nb=$(jq '.bundles|length' "$lock")
for ((i = 0; i < nb; i++)); do
  B=$(jq -c ".bundles[$i]" "$lock")
  bn=$(jq -r .name <<<"$B"); p=$(jq -r .path <<<"$B"); pl=$(jq -r .package_lock <<<"$B")
  printf '%s\n' "$p" >>"$covered"
  if [ -f "$vd/$p" ] && [ ! -L "$vd/$p" ]; then
    [ "$(sha256 "$vd/$p")" = "$(jq -r .sha256 <<<"$B")" ] || err "bundle $bn: $p sha256 != lock"
  else err "bundle $bn: $p missing"; fi
  [ -f "$vd/$pl" ] || { err "bundle $bn: package-lock $pl missing"; continue; }
  jq -e '.lockfileVersion >= 2 and (.packages|type=="object")' "$vd/$pl" >/dev/null || { err "bundle $bn: $pl not a v2/v3 package-lock"; continue; }
  badd=$(jq -r '.added // ""' <<<"$B")
  bad=$nerr
  # the lock's bundled-package list must be exactly the package-lock's non-dev packages
  plset=$(jq -r '.packages|to_entries[]|select(.key!="" and (.value.dev|not))|"\(.key|sub("^.*node_modules/";"")) \(.value.version) \(.value.integrity)"' "$vd/$pl" | sort)
  lkset=$(jq -r '.packages[]?|"\(.name) \(.version) \(.integrity)"' <<<"$B" | sort)
  if [ "$plset" != "$lkset" ]; then # temp files, not <(...): some sandboxes have no /dev/fd
    printf '%s\n' "$plset" >"$work/pl.set"; printf '%s\n' "$lkset" >"$work/lk.set"
    err "bundle $bn: lock packages != $pl non-dev packages: only-lock=[$(comm -23 "$work/lk.set" "$work/pl.set" | tr '\n' ';')] only-package-lock=[$(comm -13 "$work/lk.set" "$work/pl.set" | tr '\n' ';')]"
  fi
  n=0
  while read -r key pv pi pr pd; do
    pn=${key##*node_modules/}; n=$((n + 1))
    if ! meta "$pn" "$pv"; then err "bundle $bn: $pn@$pv not in registry"; continue; fi
    [ "$(jq -r .dist.integrity "$work/meta.json")" = "$pi" ] || err "bundle $bn: $pn@$pv integrity != registry"
    [ "$(jq -r .dist.tarball "$work/meta.json")" = "$pr" ] || err "bundle $bn: $pn@$pv resolved != registry dist.tarball"
    rtime=$(jq -r '.time // ""' "$work/meta.json")
    if [ "$pd" = true ]; then # build tool (esbuild): integrity only, never shipped
      continue
    fi
    lpub=$(jq -r --arg n "$pn" '.packages[]?|select(.name==$n)|.published' <<<"$B")
    [ "$lpub" = "${rtime:0:10}" ] || err "bundle $bn: $pn@$pv lock published '$lpub' != registry ${rtime:0:10}"
    d=$(days "$rtime" "$badd") && [ "$d" -ge "$MINDAYS" ] || err "bundle $bn: $pn@$pv published $rtime, < $MINDAYS days before added $badd"
  done <<EOF_PL
$(jq -r '.packages | to_entries[] | select(.key != "") | "\(.key) \(.value.version) \(.value.integrity) \(.value.resolved) \(.value.dev // false)"' "$vd/$pl")
EOF_PL
  [ "$bad" = "$nerr" ] && ok "bundle $bn: $p sha256, $n package-lock entries match the registry"
done

# d) the prose notes were retired for vendor.lock.json (#188): one coming back is an error by itself
while IFS= read -r f; do
  [ -n "$f" ] && err "${f#"$vd"/}: VENDOR.md was retired, record facts in vendor.lock.json"
done <<EOF_MD
$(find "$vd" -iname 'VENDOR.md')
EOF_MD

# d) coverage: nothing under ui/vendor escapes the lock
jq -r '.allowed_unlocked[]' "$lock" >>"$covered"
unl=0
while IFS= read -r f; do
  [ -n "$f" ] || continue
  rel=${f#"$vd"/}
  if [ -L "$f" ]; then err "symlink in vendor tree: $rel"; unl=1; continue; fi
  grep -qxF -- "$rel" "$covered" || { err "$rel is not covered by vendor.lock.json"; unl=1; }
done <<EOF_FILES
$(find "$vd" \( -type f -o -type l \) | sort)
EOF_FILES
[ "$unl" = 0 ] && ok "coverage: every file under $(basename "$vd") is locked or explicitly allowed"

if [ "$nerr" != 0 ]; then echo "vendor-verify: FAILED ($nerr problem(s))" >&2; exit 1; fi
echo "vendor-verify: PASS ($nlib libraries, $nb bundle(s))"
