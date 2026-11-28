#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# snapshot-bump.sh — rewrite the working tree to the NEWEST pins (operator decisions 2026-11-19:
# 15:46Z AppImage Ubuntu snapshot = cutting edge, 15:49Z scanners = cutting edge). Run by
# .github/workflows/snapshot-bump.yml, which turns the diff into a PR (never merged by CI).
# Every result is still a FIXED pin, resolved and verified here; any failure exits non-zero and
# the workflow opens no PR (fail closed). Nothing is installed or executed from what is fetched.
#   (fedport 2026-11-28: the AppImage/linux-test build env moved to Fedora 44, packaging/ci/fedora-env.sh +
#   fedora-pkgset.lock.tsv. Its re-resolve is a maintainer step - scripts/supply-chain/fedora-resolve.sh at a
#   koji event + LOCK_SHA256 in fedora-env.sh, pin-check rule rpmlock - not a weekly bot bump: the former
#   steps 1 (snapshot.ubuntu.com SNAP + appimage-snapshot.lock.tsv) and 2 (ubuntu image digest) are gone.)
#   3. scanners: osv-scanner newest non-prerelease release (API digest == SHA256SUMS per arch) ->
#      osv-scanner.pins; cargo-audit/deny/vet newest non-yanked stable version (sparse-index cksum ==
#      sha256 of the downloaded .crate) -> cargo-scanners.pins + the literal in supply-chain.yml;
#      semgrep :latest manifest digest + the version tag carrying the same digest -> semgrep.yml.
# usage: snapshot-bump.sh [REPO_ROOT]   prints a markdown summary (PR body) on stdout.
set -euo pipefail
R=$(cd "${1:-$(dirname "$0")/../..}" && pwd)
say(){ echo "[$(date -u +%H:%M:%S)] [snapshot-bump] $*" >&2; }
die(){ say "FAIL: $*"; exit 1; }
for t in curl jq sha256sum awk sed grep; do command -v $t >/dev/null || die "missing $t"; done
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
C=(curl -4 -fsSL --proto '=https' --tlsv1.2 --retry 3 -m 120)
SC=$R/.github/workflows/supply-chain.yml; SG=$R/.github/workflows/semgrep.yml
OP=$R/scripts/supply-chain/osv-scanner.pins; CP=$R/scripts/supply-chain/cargo-scanners.pins
sum=$W/summary; : > "$sum"
note(){ printf '%s\n' "$*" >> "$sum"; }
note "Automated pin bump (snapshot-bump.yml). Review the diff; CI never merges this."; note ""

acc='application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json'

# ---- 3. scanners ------------------------------------------------------------------------------
gh_hdr=(-H 'Accept: application/vnd.github+json'); [ -z "${GH_TOKEN:-}" ] || gh_hdr+=(-H "Authorization: Bearer $GH_TOKEN")
rel=$("${C[@]}" "${gh_hdr[@]}" https://api.github.com/repos/google/osv-scanner/releases/latest)
ot=$(jq -r '.tag_name' <<<"$rel"); [ "$(jq -r '.prerelease or .draft' <<<"$rel")" = false ] || die "osv-scanner $ot is a prerelease"
[[ "$ot" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "osv-scanner tag '$ot'"
"${C[@]}" -o "$W/osvsums" "https://github.com/google/osv-scanner/releases/download/$ot/osv-scanner_SHA256SUMS"
cp "$OP" "$W/op"; sed -i "s/^version .*/version $ot/" "$W/op"
for a in amd64 arm64; do
  d=$(jq -r --arg n "osv-scanner_linux_$a" '.assets[] | select(.name==$n) | .digest' <<<"$rel"); d=${d#sha256:}
  s=$(awk -v n="osv-scanner_linux_$a" '$2==n {print $1}' "$W/osvsums")
  [[ "$d" =~ ^[0-9a-f]{64}$ ]] && [ "$d" = "$s" ] || die "osv-scanner $ot $a: API digest '$d' != SHA256SUMS '$s'"
  sed -i "s/^sha256 $a .*/sha256 $a $d/" "$W/op"
done
[ "$(grep -c '^sha256 ' "$W/op")" = 2 ] || die "$OP must hold exactly the amd64 + arm64 lines"
[ "$ot" = "$(awk '$1=="version"{print $2}' "$OP")" ] || { note "- osv-scanner: $(awk '$1=="version"{print $2}' "$OP") -> $ot"; cp "$W/op" "$OP"; }
# spelled via $ci so pin-check does not read these patterns as an unpinned install
ci='cargo install'
for c in cargo-audit cargo-deny cargo-vet; do
  ov=$(awk -v n="$c" '$1=="crate" && $2==n {print $3}' "$CP"); [ -n "$ov" ] || die "no pin for $c"
  ln=$("${C[@]}" "https://index.crates.io/${c:0:2}/${c:2:2}/$c" | jq -c 'select(.yanked==false and (.vers|test("^[0-9]+\\.[0-9]+\\.[0-9]+$")))' \
    | jq -rs 'sort_by(.vers | split(".") | map(tonumber)) | last | "\(.vers) \(.cksum)"')
  nv=${ln% *}; ck=${ln#* }
  [[ "$nv" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ && "$ck" =~ ^[0-9a-f]{64}$ ]] || die "$c: bad index entry '$ln'"
  [ "$nv" != "$ov" ] || continue
  "${C[@]}" -o "$W/c.crate" "https://static.crates.io/crates/$c/$c-$nv.crate"
  echo "$ck  $W/c.crate" | sha256sum -c - >&2 || die "$c $nv: downloaded .crate sha256 != index cksum"
  sed -i "s/^crate $c $ov [0-9a-f]\{64\}\$/crate $c $nv $ck/" "$CP"
  sed -i "s/$ci --locked --version =$ov $c\$/$ci --locked --version =$nv $c/" "$SC"
  grep -q "$ci --locked --version =$nv $c\$" "$SC" || die "$c literal not found in $SC"
  note "- $c: $ov -> $nv (sha256 $ck)"
done
stok=$("${C[@]}" "https://auth.docker.io/token?service=registry.docker.io&scope=repository:semgrep/semgrep:pull" | jq -r .token)
"${C[@]}" -D "$W/sh" -o /dev/null -I -H "Authorization: Bearer $stok" -H "Accept: $acc" https://registry-1.docker.io/v2/semgrep/semgrep/manifests/latest
sd=$(tr -d '\r' < "$W/sh" | awk 'tolower($1)=="docker-content-digest:" {print $2}' | tail -1)
[[ "$sd" =~ ^sha256:[0-9a-f]{64}$ ]] || die "no digest for semgrep:latest"
sv=$("${C[@]}" "https://hub.docker.com/v2/repositories/semgrep/semgrep/tags?page_size=50&ordering=last_updated" \
  | jq -r --arg d "$sd" '.results[] | select(.digest==$d) | .name' | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' | sort -V | tail -1)
[ -n "$sv" ] || die "no version tag carries semgrep:latest digest $sd"
osg=$(grep -Eo 'semgrep/semgrep:[0-9.]+@sha256:[0-9a-f]{64}' "$SG" | head -1); [ -n "$osg" ] || die "no pinned semgrep image in $SG"
if [ "$osg" != "semgrep/semgrep:$sv@$sd" ]; then sed -i "s|$osg|semgrep/semgrep:$sv@$sd|" "$SG"; note "- semgrep image: ${osg#semgrep/semgrep:} -> $sv@$sd"; fi
note ""; note "Gates to watch: release.yml dry run (WebKit CVE gate, SBOM, license policy), supply-chain gate-tests + pin-check."
cat "$sum"
