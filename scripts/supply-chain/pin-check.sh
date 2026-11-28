#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# pin-check.sh — #189: every dependency is pinned to an exact version AND a content hash.
#   bash scripts/supply-chain/pin-check.sh [ROOT] [ALLOWLIST]
#     ROOT       tree to scan (default: the repo this script lives in)
#     ALLOWLIST  default ROOT/supply-chain/pin-allow.tsv (absent = empty)
# Scans *.sh *.bash *.yml *.yaml Dockerfile* Containerfile* under ROOT (not vendored trees, not
# scripts/supply-chain/tests/fixtures, not .opengrep rule files). Comment lines and echo/printf lines
# are not commands and are skipped; a trailing " # comment" is cut before matching.
# Rules (one finding line "PIN <rule> <file>:<line>: <text>" each):
#   uses     GitHub `uses:` not pinned to a 40-hex commit sha (local ./ actions excepted)
#   image    `image:`/`container:`/FROM/podman|docker pull|run without @sha256:<64 hex>
#            ($VAR images resolve through VAR= assignments in the same file; oci:/oci-archive:
#            sources count only when the file also runs `sha256sum -c` AND, after the load, checks
#            `podman|docker image exists|inspect <ref>` with a digest-pinned ref whose repository
#            is the target of a `podman|docker tag` of the loaded image)
#   apt      apt-get/apt install of a package without =version
#   dnf      dnf/yum install of a package without -<version>
#   cargo    cargo install without --locked AND --version (or name@version)
#   flatpak  flatpak install without --commit= (here or a `flatpak update --commit=` within 6 lines);
#            --commit="$VAR" counts only when every VAR= assignment in the same file is 64 hex
#   fetch    curl/wget download (-o/-O/--output, wget) without sha256sum/sha512sum within 6 lines,
#            unless (operator 2026-11-19) a gpgv|sqv line within 6 lines verifies the same -o target,
#            fails closed (|| die / || exit N, never || true or piped) and uses a --keyring that is
#            a committed repo file listed with its sha256 in supply-chain/keyrings.tsv (no host or
#            downloaded keyring); curl|wget piped into a shell is always a finding
#   pkgmgr   npm/npx/yarn/pnpm/bun/pipx/go install|get (forbidden), pip without --require-hashes
#   rpmlock  packaging/ci/fedora-pkgset.lock.tsv (Fedora 44 build env): malformed row, duplicate, a missing
#            arch, lock sha256 != fedora-env.sh LOCK_SHA256, an appimage-build.sh pin not in the lock
# ALLOWLIST: TSV  rule  path  needle  expires(YYYY-MM-DD)  scanner  reason
#   A finding is allowed when rule and path match and the line contains needle. expires >= today,
#   reason >= 10 chars. scanner is "-" or one of the NAMED scanner exception (operator 2026-11-17:
#   semgrep opengrep cargo-audit cargo-deny cargo-vet osv-scanner); a scanner entry's needle must
#   contain the scanner name, so the exception cannot cover anything else. An entry that matches
#   nothing is stale and is a finding too.
# rc 0 = clean, 1 = findings / bad allowlist, 2 = usage. PIN_CHECK_TODAY=YYYY-MM-DD overrides today.
set -u
HERE=$(cd "$(dirname -- "$0")" && pwd)
ROOT=${1:-$(cd "$HERE/../.." && pwd)}; ROOT=${ROOT%/}
AL=${2:-$ROOT/supply-chain/pin-allow.tsv}
[ -d "$ROOT" ] || { echo "pin-check: no dir $ROOT" >&2; exit 2; }
TODAY=${PIN_CHECK_TODAY:-$(date -u +%F)}
SCANNERS=" semgrep opengrep cargo-audit cargo-deny cargo-vet osv-scanner "
RULES=" uses image apt dnf cargo flatpak fetch pkgmgr rpmlock "
NF=0; T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
: > "$T/find"; : > "$T/used"

# ---- allowlist ------------------------------------------------------------------------------------
n=0; : > "$T/allow"
if [ -f "$AL" ]; then
  while IFS= read -r l || [ -n "$l" ]; do n=$((n+1))
    case "$l" in ''|'#'*) continue ;; esac
    IFS=$'\t' read -r r p nd ex sc rs <<< "$l"
    bad=""
    case "$RULES" in *" $r "*) ;; *) bad="unknown rule '$r'" ;; esac
    [ -n "$p" ] && [ -n "$nd" ] || bad="${bad:+$bad; }empty path/needle"
    printf '%s' "$ex" | grep -Eqx '[0-9]{4}-[0-9]{2}-[0-9]{2}' || bad="${bad:+$bad; }expiry '$ex' is not YYYY-MM-DD"
    [ -z "${ex##[0-9]*}" ] && [[ ! "$ex" < "$TODAY" ]] || bad="${bad:+$bad; }expired $ex (today $TODAY)"
    [ "${#rs}" -ge 10 ] || bad="${bad:+$bad; }no reason"
    if [ "$sc" != "-" ]; then
      case "$SCANNERS" in *" $sc "*) case "$nd" in *"$sc"*) ;; *) bad="${bad:+$bad; }scanner '$sc' entry needle '$nd' does not name it" ;; esac ;;
        *) bad="${bad:+$bad; }'$sc' is not a named scanner (${SCANNERS# })" ;; esac
    fi
    if [ -n "$bad" ]; then echo "PIN allow ${AL#$ROOT/}:$n: $bad"; NF=$((NF+1)); continue; fi
    printf '%s\t%s\t%s\t%s\n' "$n" "$r" "$p" "$nd" >> "$T/allow"
  done < "$AL"
fi

allowed() {  # rule file text -> 0 if an allowlist entry covers it (records the use)
  local an ar ap and
  while IFS=$'\t' read -r an ar ap and; do
    [ "$ar" = "$1" ] && [ "$ap" = "$2" ] || continue
    case "$3" in *"$and"*) echo "$an" >> "$T/used"; return 0 ;; esac
  done < "$T/allow"; return 1
}
hit() {  # rule file line text
  allowed "$1" "$2" "$4" && return 0
  printf 'PIN %s %s:%s: %s\n' "$1" "$2" "$3" "$(printf '%s' "$4" | sed 's/^[[:space:]]*//' | cut -c1-160)"
  NF=$((NF+1))
}

# var_pinned FILE VAR -> 0 if every VAR= assignment in FILE is an image ref with @sha256:<64>
var_pinned() {
  local v a ok=1
  v=$(grep -oE "(^|[[:space:];(])$2=[^[:space:];]+" "$1" | sed "s/^.*$2=//; s/[\"']//g")
  [ -n "$v" ] || return 1
  while IFS= read -r a; do
    if printf '%s' "$a" | grep -Eq '@sha256:[0-9a-f]{64}$|^sha256:[0-9a-f]{64}$'; then continue; fi
    if [[ "$a" =~ @\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$ ]]; then var_pinned "$1" "${BASH_REMATCH[1]}" && continue; fi
    ok=0
  done <<< "$v"
  [ $ok = 1 ]
}
oci_pinned() {  # file -> 0 if its oci:/archive image load is content-checked
  # The archive sha256 alone does not cover what is loaded: the extracted dir (or another path) can
  # differ from the checked file. So ALL of: a `sha256sum -c` in the file; from the first
  # oci:/archive load on, a `podman|docker tag <id> <repo>[:tag]` that names the loaded image; and
  # after the load a `podman|docker image exists|inspect <ref>` whose ref is digest-pinned (literal
  # @sha256:<64 hex> or through VAR= assignments) AND whose repository is one of those tag targets,
  # so the digest check is about the image that was loaded, not some unrelated pinned image.
  local f=$1 n r x refs repos
  grep -q 'sha256sum -c' "$f" || return 1
  n=$(grep -nE '(^|[^a-z])(oci|oci-archive|docker-archive):' "$f" | grep -E '(podman|docker)' | head -1 | cut -d: -f1)
  [ -n "$n" ] || return 1
  repos=$(tail -n +"$n" "$f" | grep -oE '(podman|docker)[[:space:]]+tag[[:space:]].*' | awk '{print $NF}' \
    | sed -E "s/[\"');&|]//g; s#:[^/]*\$##" | sort -u)
  [ -n "$repos" ] || return 1
  refs=$(tail -n +"$((n+1))" "$f" | grep -oE '(podman|docker)[[:space:]]+image[[:space:]]+(exists|inspect)[[:space:]]+[^[:space:];|&)]+' | awk '{print $4}')
  while IFS= read -r r; do
    r=${r//\"/}; r=${r//\'/}
    [ -n "$r" ] || continue
    printf '%s' "$r" | grep -Eq '(oci|oci-archive|docker-archive):' && continue
    img_pinned "$f" "$r" || continue
    for x in $(ref_repos "$f" "$r"); do grep -qxF -- "$x" <<< "$repos" && return 0; done
  done <<< "$refs"
  return 1
}
ref_repos() {  # file ref-text -> the repository part(s) of the ref (VAR refs through every VAR= assignment)
  local f=$1 r=$2 x
  if [[ "$r" =~ ^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$ ]]; then
    x=${BASH_REMATCH[1]}
    grep -oE "(^|[[:space:];(])$x=[^[:space:];]+" "$f" | sed -E "s/^.*$x=//; s/[\"']//g" | sed -E 's/@.*$//; s#:[^/]*$##'
  else
    printf '%s\n' "$r" | sed -E 's/@.*$//; s#:[^/]*$##'
  fi
}
img_pinned() {  # file ref-text -> 0 if pinned
  local f=$1 s=$2 vars x
  printf '%s' "$s" | grep -Eq '@sha256:[0-9a-f]{64}' && return 0
  if printf '%s' "$s" | grep -Eq '(^|[^a-z])(oci|oci-archive|docker-archive):'; then oci_pinned "$f"; return; fi
  vars=$(printf '%s' "$s" | grep -oE '\$\{?[A-Za-z_][A-Za-z0-9_]*' | tr -d '${' | sort -u)
  [ -n "$vars" ] || return 1
  for x in $vars; do var_pinned "$f" "$x" && return 0; done
  return 1
}
# commit_pinned FILE TEXT -> 0 if TEXT carries --commit=<64 hex>, or --commit="$VAR" where FILE
# assigns VAR at least once and every VAR= assignment in FILE is exactly 64 hex
commit_pinned() {
  local v a
  printf '%s' "$2" | grep -Eq -- '--commit=[0-9a-f]{64}' && return 0
  [[ "$2" =~ --commit=[\"\']?\$\{?([A-Za-z_][A-Za-z0-9_]*) ]] || return 1
  v=$(grep -oE "(^|[[:space:];(])${BASH_REMATCH[1]}=[^[:space:];)]*" "$1" | sed -E "s/^.*${BASH_REMATCH[1]}=//; s/[\"']//g")
  [ -n "$v" ] || return 1
  while IFS= read -r a; do [[ "$a" =~ ^[0-9a-f]{64}$ ]] || return 1; done <<< "$v"
}
# packages of an install command: words after "install" up to a shell separator, flags dropped,
# "$VAR" expanded from VAR="..." in the same file
pkgs() {  # file cmdtext
  local w v rest
  rest=$(printf '%s' "$2" | sed -E 's/^.*[[:space:]]install([[:space:]]|$)//; s/(&&|\|\||[;|>}]|\)).*$//; s/[\x27"]//g')
  for w in $rest; do
    case "$w" in -*) continue ;; esac
    if [[ "$w" =~ ^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$ ]]; then
      v=$(grep -oE "(^|[[:space:]])${BASH_REMATCH[1]}=(\"[^\"]*\"|[^[:space:]\"';]+)" "$1" | head -1 | sed -E 's/^[^=]*=//; s/"//g')
      [ -n "$v" ] && { printf '%s\n' $v; continue; }
    fi
    printf '%s\n' "$w"
  done
}

# sig_pinned FILE JOINED WINDOW -> 0 if the download's output file is signature-verified within the
# 6-line window (operator 2026-11-19 16:22Z): a gpgv|sqv line names the same -o/--output target,
# fails closed (`|| die`, `|| { ... die`, `|| exit N>0`; no `|| true`/`|| :`, not piped), and its
# --keyring resolves (literal, or EVERY VAR= assignment in FILE) to a repo-relative path
# ($VAR/ prefix and ../ allowed, no absolute/host path, no other $VAR) that is listed in
# ROOT/supply-chain/keyrings.tsv with the sha256 of the committed file, and is not itself downloaded.
sig_pinned() {
  local f=$1 out v line seg kr vals x rel want got kt=$ROOT/supply-chain/keyrings.tsv
  [[ "$2" =~ (^|[[:space:]])(-o|--output)[[:space:]=]+(\"[^\"]*\"|\'[^\']*\'|[^[:space:]]+) ]] || return 1
  out=${BASH_REMATCH[3]}; out=${out//\"/}; out=${out//\'/}
  [ -n "$out" ] && [ -f "$kt" ] || return 1
  line=$(printf '%s\n' "$3" | sed -E "s/[\"']//g" | grep -E '(^|[^a-z-])(gpgv|sqv)[[:space:]]' | grep -F -- " $out" | head -1)
  [ -n "$line" ] || return 1
  seg=${line%%||*}; [ "$seg" != "$line" ] || return 1          # no || at all: not fail closed
  seg=${seg#*gpgv}; seg=${seg#*sqv}
  [[ "$seg" == *"|"* ]] && return 1                            # piped: exit status lost
  printf '%s' "$line" | grep -Eq '\|\|[[:space:]]*(true|:)([[:space:];)}]|$)' && return 1
  printf '%s' "$line" | grep -Eq '\|\|[[:space:]]*(\{[^}]*[[:space:];{])?(die|exit[[:space:]]+[1-9])' || return 1
  [[ "$line" =~ --keyring[[:space:]=]+([^[:space:]]+) ]] || return 1
  kr=${BASH_REMATCH[1]}
  if [[ "$kr" =~ ^\$\{?([A-Za-z_][A-Za-z0-9_]*)\}?$ ]]; then
    vals=$(grep -oE "(^|[[:space:];(])${BASH_REMATCH[1]}=[^[:space:];]+" "$f" | sed -E "s/^.*${BASH_REMATCH[1]}=//; s/[\"']//g")
  else vals=$kr; fi
  [ -n "$vals" ] || return 1
  # the keyring (var or value) must not be a download target anywhere in the file
  grep -E '(^|[^a-z-])(curl|wget)[[:space:]]' "$f" | sed -E "s/[\"']//g" | grep -qF -- "$kr" && return 1
  while IFS= read -r v; do
    case "$v" in /*|'') return 1 ;; esac
    rel=$(printf '%s' "$v" | sed -E 's#^\$\{?[A-Za-z_][A-Za-z0-9_]*\}?/##; s#^(\./|\.\./)+##')
    case "$rel" in *'$'*|*..*|'') return 1 ;; esac
    grep -E '(^|[^a-z-])(curl|wget)[[:space:]]' "$f" | grep -qF -- "$rel" && return 1
    [ -f "$ROOT/$rel" ] || return 1
    want=$(awk -F'\t' -v p="$rel" '$1==p{print $2}' "$kt" | head -1)
    [[ "$want" =~ ^[0-9a-f]{64}$ ]] || return 1
    got=$(sha256sum < "$ROOT/$rel" | cut -d' ' -f1)
    [ "$got" = "$want" ] || return 1
  done <<< "$vals"
  return 0
}

# ---- scan -----------------------------------------------------------------------------------------
nocom() { printf '%s' "$1" | sed -E 's/[[:space:]]#[^"\x27]*$//'; }  # cut a trailing " # comment"
find "$ROOT" \( -name .git -o -path "$ROOT/ui/vendor" -o -path "$ROOT/src-tauri/vendor" -o -path "$ROOT/src-tauri/target" \
     -o -path "$ROOT/scripts/supply-chain/tests/fixtures" -o -path "$ROOT/.opengrep" -o -name node_modules -o -path "$ROOT/dist" \) -prune -o \
     -type f \( -name '*.sh' -o -name '*.bash' -o -name '*.yml' -o -name '*.yaml' -o -name 'Dockerfile*' -o -name 'Containerfile*' \) -print \
  | LC_ALL=C sort > "$T/files"
[ "$(basename -- "$0")" = pin-check.sh ] && SELF=$(cd "$HERE" && pwd)/pin-check.sh || SELF=""
while IFS= read -r F; do
  [ "$F" = "$SELF" ] && continue
  rel=${F#$ROOT/}
  mapfile -t L < "$F"
  for ((i=0; i<${#L[@]}; i++)); do
    raw=${L[$i]}; ln=$((i+1))
    s=$(nocom "$raw")
    [[ "$s" =~ ^[[:space:]]*(#|$) ]] && continue
    [[ "$s" =~ ^[[:space:]]*(-[[:space:]]+)?(echo|printf|say|die|name:)[[:space:]] ]] && continue
    # joined command (backslash continuations) and the 6-line look-ahead window
    j=$s; k=$i; while [[ "$j" =~ \\[[:space:]]*$ ]] && [ $((k+1)) -lt ${#L[@]} ]; do k=$((k+1)); j="${j%\\*} $(nocom "${L[$k]}")"; done
    win=$(printf '%s\n' "${L[@]:$((i+1)):6}")
    # uses
    if [[ "$s" =~ ^[[:space:]]*(-[[:space:]]+)?uses:[[:space:]]*[\"\']?([^[:space:]\"\']+) ]]; then
      u=${BASH_REMATCH[2]}
      case "$u" in ./*) ;; docker://*) printf '%s' "$u" | grep -Eq '@sha256:[0-9a-f]{64}$' || hit uses "$rel" $ln "$s" ;;
        *) printf '%s' "$u" | grep -Eq '@[0-9a-f]{40}$' || hit uses "$rel" $ln "$s" ;; esac
    fi
    # image
    if [[ "$s" =~ ^[[:space:]]*(-[[:space:]]+)?(image|container):[[:space:]]*([^[:space:]]+) ]] && [ "${BASH_REMATCH[3]}" != "|" ]; then
      img_pinned "$F" "${BASH_REMATCH[3]}" || hit image "$rel" $ln "$s"
    fi
    if [[ "$rel" =~ (Dockerfile|Containerfile) ]] && [[ "$s" =~ ^[[:space:]]*FROM[[:space:]]+(--[^[:space:]]+[[:space:]]+)*([^[:space:]]+) ]]; then
      [ "${BASH_REMATCH[2]}" = scratch ] || img_pinned "$F" "${BASH_REMATCH[2]}" || hit image "$rel" $ln "$s"
    fi
    if printf '%s' "$s" | grep -Eq '(^|[^a-z-])(podman|docker)[[:space:]]+(image[[:space:]]+)?(pull|run|create)[[:space:]]'; then
      img_pinned "$F" "$j" || hit image "$rel" $ln "$s"
    fi
    # apt / dnf
    if printf '%s' "$j" | grep -Eq '(^|[^a-z-])apt(-get)?[[:space:]]+([^;&|]*[[:space:]])?install([[:space:]]|$)'; then
      for p in $(pkgs "$F" "$j"); do case "$p" in *=?*) ;; *) hit apt "$rel" $ln "$s [$p]" ;; esac; done
    fi
    if printf '%s' "$j" | grep -Eq '(^|[^a-z-])(dnf|yum|microdnf)[[:space:]]+([^;&|]*[[:space:]])?install([[:space:]]|$)'; then
      for p in $(pkgs "$F" "$j"); do printf '%s' "$p" | grep -Eq -- '-[0-9][^[:space:]]*-[^[:space:]]+$' || hit dnf "$rel" $ln "$s [$p]"; done
    fi
    # cargo
    if printf '%s' "$j" | grep -Eq '(^|[^a-z-])cargo[[:space:]]+(\+[^[:space:]]+[[:space:]]+)?install[[:space:]]'; then
      printf '%s' "$j" | grep -Eq -- '--locked' && printf '%s' "$j" | grep -Eq -- '--version[= ]+=?[0-9]|[a-z0-9_-]@[0-9]+\.[0-9]+\.[0-9]+' || hit cargo "$rel" $ln "$s"
    fi
    # flatpak
    if printf '%s' "$j" | grep -Eq '(^|[^a-z-])flatpak[[:space:]]+([^;&|]*[[:space:]])?install([[:space:]]|$)'; then
      commit_pinned "$F" "$(printf '%s\n%s' "$j" "$win" | grep -E -- '--commit=' | head -1)" || hit flatpak "$rel" $ln "$s"
    fi
    # fetch
    if printf '%s' "$s" | grep -Eq '(^|[^a-z-])(curl|wget)[[:space:]]'; then
      if printf '%s' "$j" | grep -Eq '(curl|wget)[^|]*\|[[:space:]]*(sudo[[:space:]]+)?(ba|z|da)?sh([[:space:]]|$)'; then hit fetch "$rel" $ln "$s"
      elif printf '%s' "$j" | grep -Eq '(^|[^a-z-])wget[[:space:]]|[[:space:]](-[a-zA-Z]*[oO]|--output|--remote-name)([[:space:]=]|$)'; then
        printf '%s\n%s' "$j" "$win" | grep -Eq 'sha(256|512)sum' || sig_pinned "$F" "$j" "$win" || hit fetch "$rel" $ln "$s"
      fi
    fi
    # pkgmgr
    if printf '%s' "$s" | grep -Eq '(^|[[:space:];&|(`])(npm|npx|yarn|pnpm|bun|bunx|pipx)[[:space:]]|(^|[[:space:];&|(`])go[[:space:]]+(install|get)[[:space:]]'; then hit pkgmgr "$rel" $ln "$s"
    elif printf '%s' "$s" | grep -Eq '(^|[[:space:];&|(`])(pip3?|python3?[[:space:]]+-m[[:space:]]+pip)[[:space:]]+install' && ! printf '%s' "$j" | grep -q -- '--require-hashes'; then hit pkgmgr "$rel" $ln "$s"; fi
  done
done < "$T/files"

# ---- rpmlock (fedport 2026-11-28): the Fedora 44 build env's exact package set --------------------
# packaging/ci/fedora-pkgset.lock.tsv = every rpm of the appimage-build.sh container per arch. Each row:
# target-arch(x86_64|aarch64) name epoch:version-release arch(=target|noarch) sha256(64 hex)
# srcname/version/release; no duplicate (target,name,arch); both arches present. The lock's own
# sha256 must equal LOCK_SHA256= in packaging/ci/fedora-env.sh (which refuses any other lock at
# build time), and every appimage-build.sh PKGS / SBOM_PKGS n-v-r must be a row of BOTH arches.
# So a changed version, hash, added or dropped package = a finding until the lock is re-resolved
# (scripts/supply-chain/fedora-resolve.sh) and LOCK_SHA256 updated with it.
FL=$ROOT/packaging/ci/fedora-pkgset.lock.tsv FE=$ROOT/packaging/ci/fedora-env.sh FA=$ROOT/packaging/ci/appimage-build.sh
if [ -f "$FL" ] || [ -f "$FE" ]; then
  if [ ! -f "$FL" ]; then hit rpmlock packaging/ci/fedora-pkgset.lock.tsv 0 "missing (packaging/ci/fedora-env.sh needs it)"
  else
    awk -F'\t' '!/^#/ && NF {
        n++; k=$1"\t"$2"\t"$4; bad=""
        if (NF != 6) bad="6 fields expected, got " NF
        else if ($1 != "x86_64" && $1 != "aarch64") bad="target-arch " $1
        else if ($2 !~ /^[A-Za-z0-9][A-Za-z0-9._+-]*$/) bad="name " $2
        else if ($3 !~ /^[0-9]+:[^-[:space:]]+-[^-[:space:]]+$/) bad="evr not epoch:version-release: " $3
        else if ($4 != $1 && $4 != "noarch") bad="arch " $4 " for target " $1
        else if ($5 !~ /^[0-9a-f]{64}$/) bad="sha256 not 64 hex: " $5
        else if ($6 !~ /^[^\/[:space:]]+\/[^\/[:space:]]+\/[^\/[:space:]]+$/) bad="koji build path not src/version/release: " $6
        else if (k in seen) bad="duplicate " k
        seen[k]=1; t[$1]++
        if (bad != "") printf "%d\t%s\n", NR, bad
      }
      END { if (!t["x86_64"]) print "0\tno x86_64 rows"; if (!t["aarch64"]) print "0\tno aarch64 rows" }' "$FL" > "$T/rl"
    while IFS=$'\t' read -r ln why; do hit rpmlock packaging/ci/fedora-pkgset.lock.tsv "$ln" "$why"; done < "$T/rl"
    want=$(sed -n 's/^LOCK_SHA256=\([0-9a-f]\{64\}\)\([[:space:]].*\)\{0,1\}$/\1/p' "$FE" 2>/dev/null | head -1)
    got=$(sha256sum "$FL" | cut -d' ' -f1)
    [ "$want" = "$got" ] || hit rpmlock packaging/ci/fedora-pkgset.lock.tsv 0 "sha256 $got != LOCK_SHA256 '${want:-none}' in packaging/ci/fedora-env.sh (lock drifted: re-resolve + update both)"
    if [ -f "$FA" ]; then
      for p in $(sed -n 's/^\(SBOM_\)\{0,1\}PKGS="\([^"]*\)".*/\2/p' "$FA"); do for la in x86_64 aarch64; do
        awk -F'\t' -v p="$p" -v a="$la" '!/^#/ && $1==a { e=$3; sub(/^[0-9]+:/,"",e); if ($2"-"e == p) f=1 } END{exit !f}' "$FL" \
          || hit rpmlock packaging/ci/appimage-build.sh 0 "pin $p is not a $la row of fedora-pkgset.lock.tsv"
      done; done
    fi
  fi
fi

# ---- stale allowlist entries ----------------------------------------------------------------------
while IFS=$'\t' read -r an ar ap and; do
  grep -qx "$an" "$T/used" || { echo "PIN allow ${AL#$ROOT/}:$an: stale (matches nothing): $ar $ap '$and'"; NF=$((NF+1)); }
done < "$T/allow"
echo "pin-check: $(wc -l < "$T/files") files, $(wc -l < "$T/allow") allowlist entries, $NF finding(s)"
[ $NF = 0 ]
