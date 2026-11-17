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
#            sources count only when the file also runs `sha256sum -c`)
#   apt      apt-get/apt install of a package without =version
#   dnf      dnf/yum install of a package without -<version>
#   cargo    cargo install without --locked AND --version (or name@version)
#   flatpak  flatpak install without --commit= (here or a `flatpak update --commit=` within 6 lines)
#   fetch    curl/wget download (-o/-O/--output, wget) without sha256sum/sha512sum within 6 lines;
#            curl|wget piped into a shell is always a finding
#   pkgmgr   npm/npx/yarn/pnpm/bun/pipx/go install|get (forbidden), pip without --require-hashes
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
RULES=" uses image apt dnf cargo flatpak fetch pkgmgr "
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
img_pinned() {  # file ref-text -> 0 if pinned
  local f=$1 s=$2 vars x
  printf '%s' "$s" | grep -Eq '@sha256:[0-9a-f]{64}' && return 0
  if printf '%s' "$s" | grep -Eq '(^|[^a-z])(oci|oci-archive|docker-archive):'; then grep -q 'sha256sum -c' "$f" && return 0; return 1; fi
  vars=$(printf '%s' "$s" | grep -oE '\$\{?[A-Za-z_][A-Za-z0-9_]*' | tr -d '${' | sort -u)
  [ -n "$vars" ] || return 1
  for x in $vars; do var_pinned "$f" "$x" && return 0; done
  return 1
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
      printf '%s\n%s' "$j" "$win" | grep -Eq -- '--commit=[0-9a-f]{64}' || hit flatpak "$rel" $ln "$s"
    fi
    # fetch
    if printf '%s' "$s" | grep -Eq '(^|[^a-z-])(curl|wget)[[:space:]]'; then
      if printf '%s' "$j" | grep -Eq '(curl|wget)[^|]*\|[[:space:]]*(sudo[[:space:]]+)?(ba|z|da)?sh([[:space:]]|$)'; then hit fetch "$rel" $ln "$s"
      elif printf '%s' "$j" | grep -Eq '(^|[^a-z-])wget[[:space:]]|[[:space:]](-[a-zA-Z]*[oO]|--output|--remote-name)([[:space:]=]|$)'; then
        printf '%s\n%s' "$j" "$win" | grep -Eq 'sha(256|512)sum' || hit fetch "$rel" $ln "$s"
      fi
    fi
    # pkgmgr
    if printf '%s' "$s" | grep -Eq '(^|[[:space:];&|(`])(npm|npx|yarn|pnpm|bun|bunx|pipx)[[:space:]]|(^|[[:space:];&|(`])go[[:space:]]+(install|get)[[:space:]]'; then hit pkgmgr "$rel" $ln "$s"
    elif printf '%s' "$s" | grep -Eq '(^|[[:space:];&|(`])(pip3?|python3?[[:space:]]+-m[[:space:]]+pip)[[:space:]]+install' && ! printf '%s' "$j" | grep -q -- '--require-hashes'; then hit pkgmgr "$rel" $ln "$s"; fi
  done
done < "$T/files"

# ---- stale allowlist entries ----------------------------------------------------------------------
while IFS=$'\t' read -r an ar ap and; do
  grep -qx "$an" "$T/used" || { echo "PIN allow ${AL#$ROOT/}:$an: stale (matches nothing): $ar $ap '$and'"; NF=$((NF+1)); }
done < "$T/allow"
echo "pin-check: $(wc -l < "$T/files") files, $(wc -l < "$T/allow") allowlist entries, $NF finding(s)"
[ $NF = 0 ]
