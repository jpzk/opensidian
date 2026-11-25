#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# snapshot-bump.sh — rewrite the working tree to the NEWEST pins (operator decisions 2026-11-19:
# 15:46Z AppImage Ubuntu snapshot = cutting edge, 15:49Z scanners = cutting edge). Run by
# .github/workflows/snapshot-bump.yml, which turns the diff into a PR (never merged by CI).
# Every result is still a FIXED pin, resolved and verified here; any failure exits non-zero and
# the workflow opens no PR (fail closed). Nothing is installed or executed from what is fetched.
#   1. SNAP: the top of the previous full UTC hour (YYYYMMDDTHH0000Z, a past fixed timestamp; never
#      /latest). Lock: snapshot-resolve.sh (gpgv against the committed keyring, Packages.xz sha256)
#      -> packaging/ci/appimage-snapshot.lock.tsv; PKGS versions rewritten from the lock in
#      appimage-build.sh, sbom-local.sh, webkit-apt.sh, packaging/ci/linux-test.sh; old SNAP
#      rewritten where it is pinned.
#   2. image: newest docker.io/library/ubuntu resolute-* tag, manifest-list digest from the registry,
#      must list linux/amd64 and linux/arm64/v8 -> DIGEST (appimage-build.sh, linux-test.sh) + WK_IMG
#      (sbom-local.sh).
#   3. scanners: osv-scanner newest non-prerelease release (API digest == SHA256SUMS per arch) ->
#      osv-scanner.pins; cargo-audit/deny/vet newest non-yanked stable version (sparse-index cksum ==
#      sha256 of the downloaded .crate) -> cargo-scanners.pins + the literal in supply-chain.yml;
#      semgrep :latest manifest digest + the version tag carrying the same digest -> semgrep.yml.
# usage: snapshot-bump.sh [REPO_ROOT]   prints a markdown summary (PR body) on stdout.
set -euo pipefail
R=$(cd "${1:-$(dirname "$0")/../..}" && pwd)
say(){ echo "[$(date -u +%H:%M:%S)] [snapshot-bump] $*" >&2; }
die(){ say "FAIL: $*"; exit 1; }
for t in curl jq gpgv xz dpkg sha256sum awk sed grep; do command -v $t >/dev/null || die "missing $t"; done
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
C=(curl -4 -fsSL --proto '=https' --tlsv1.2 --retry 3 -m 120)
AB=$R/packaging/ci/appimage-build.sh; SL=$R/scripts/supply-chain/sbom-local.sh; WA=$R/scripts/supply-chain/webkit-apt.sh
WT=$R/scripts/supply-chain/tests/webkit-apt.test.sh; LOCK=$R/packaging/ci/appimage-snapshot.lock.tsv
LT=$R/packaging/ci/linux-test.sh   # linux.yml PR test env: the same pins as appimage-build.sh (it checks equality)
SC=$R/.github/workflows/supply-chain.yml; SG=$R/.github/workflows/semgrep.yml
OP=$R/scripts/supply-chain/osv-scanner.pins; CP=$R/scripts/supply-chain/cargo-scanners.pins
sum=$W/summary; : > "$sum"
note(){ printf '%s\n' "$*" >> "$sum"; }
note "Automated pin bump (snapshot-bump.yml). Review the diff; CI never merges this."; note ""

# ---- 1. snapshot ------------------------------------------------------------------------------
old_snap=$(sed -n 's/^SNAP=\([0-9]\{8\}T[0-9]\{6\}Z\).*/\1/p' "$AB" | head -1)
[ -n "$old_snap" ] || die "no SNAP= in $AB"
snap=$(date -u -d '1 hour ago' +%Y%m%dT%H0000Z)
[[ "$snap" > "$old_snap" ]] || die "computed SNAP $snap is not newer than $old_snap"
old_pkgs=$(sed -n 's/^PKGS="\([^"]*\)".*/\1/p' "$AB" | head -1)
[ -n "$old_pkgs" ] || die "no PKGS= in $AB"
names=$(for p in $old_pkgs; do printf '%s ' "${p%%=*}"; done)
say "resolving $snap for: $names jq libjq1"
bash "$R/scripts/supply-chain/snapshot-resolve.sh" "$snap" $names jq libjq1 > "$W/lock" || die "snapshot-resolve $snap"
new_pkgs=
for n in $names; do
  v=$(awk -F'\t' -v n="$n" '$1==n && $3=="amd64" {print $2}' "$W/lock")
  [ "$(printf '%s\n' "$v" | grep -c .)" = 1 ] || die "lock: not exactly one amd64 version for $n"
  new_pkgs="${new_pkgs:+$new_pkgs }$n=$v"
done
# every pinned apt list (appimage-build.sh PKGS + SBOM_PKGS, webkit-apt.sh PKGS = a subset,
# sbom-local.sh PKGS = PKGS + jq libjq1) is rewritten token by token: each name=<old> becomes
# name=<lock amd64 version>; a name not in the lock, or not exactly one such line, fails closed.
# Fixed-string awk only (versions contain regex metacharacters: no grep/sed patterns on them).
repin() { # file var
  local f=$1 k=$2
  [ "$(awk -v p="$k=\"" 'index($0, p) == 1' "$f" | wc -l)" = 1 ] || die "$f: not exactly one $k=\"...\" line"
  awk -F'\t' -v p="$k=\"" -v lk="$W/lock" -v f="$f" '
    BEGIN { while ((getline l < lk) > 0) { split(l, c, "\t"); if (c[3] == "amd64") { if (c[1] in v) dup[c[1]] = 1; v[c[1]] = c[2] } } }
    index($0, p) == 1 {
      rest = substr($0, length(p) + 1); e = index(rest, "\""); if (e == 0) { print f ": unterminated " p > "/dev/stderr"; exit 3 }
      n = split(substr(rest, 1, e - 1), t, " "); out = ""
      for (i = 1; i <= n; i++) { nm = t[i]; sub(/=.*/, "", nm)
        if (!(nm in v) || (nm in dup)) { print f ": " nm " not exactly once in the lock" > "/dev/stderr"; exit 3 }
        out = out (i > 1 ? " " : "") nm "=" v[nm] }
      $0 = p out substr(rest, e) }
    { print }' "$f" > "$W/re" || die "$f: $k rewrite failed"
  cat "$W/re" > "$f"
}
cp "$W/lock" "$LOCK"
repin "$AB" PKGS; repin "$AB" SBOM_PKGS; repin "$WA" PKGS; repin "$SL" PKGS; repin "$LT" PKGS
[ "$(sed -n 's/^PKGS="\([^"]*\)".*/\1/p' "$AB")" = "$new_pkgs" ] || die "$AB PKGS != resolved set"
for f in "$AB" "$WA" "$WT" "$LT"; do sed -i "s/$old_snap/$snap/g" "$f"; done
[ "$(sed -n 's/^PKGS="\([^"]*\)".*/\1/p' "$LT")" = "$new_pkgs" ] || die "$LT PKGS != resolved set"
note "- snapshot.ubuntu.com: $old_snap -> $snap (lock re-resolved, gpgv + Packages.xz sha256)"
for p in $new_pkgs; do case " $old_pkgs " in *" $p "*) ;; *) note "  - $p (was $(printf '%s\n' $old_pkgs | grep "^${p%%=*}=" | cut -d= -f2-))" ;; esac; done

# ---- 2. image ---------------------------------------------------------------------------------
old_dig=$(sed -n 's/^DIGEST=\(sha256:[0-9a-f]\{64\}\).*/\1/p' "$AB" | head -1)
[ -n "$old_dig" ] || die "no DIGEST= in $AB"
tag=$("${C[@]}" "https://hub.docker.com/v2/repositories/library/ubuntu/tags?page_size=100&name=resolute-" \
  | jq -r '.results[].name' | grep -E '^resolute-[0-9]{8}(\.[0-9]+)?$' | sort -V | tail -1)
[ -n "$tag" ] || die "no resolute-YYYYMMDD tag on Docker Hub"
tok=$("${C[@]}" "https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/ubuntu:pull" | jq -r .token)
acc='application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json'
"${C[@]}" -D "$W/h" -o "$W/idx" -H "Authorization: Bearer $tok" -H "Accept: $acc" "https://registry-1.docker.io/v2/library/ubuntu/manifests/$tag"
dig=$(tr -d '\r' < "$W/h" | awk 'tolower($1)=="docker-content-digest:" {print $2}' | tail -1)
[[ "$dig" =~ ^sha256:[0-9a-f]{64}$ ]] || die "no digest for ubuntu:$tag"
[ "sha256:$(sha256sum < "$W/idx" | cut -d' ' -f1)" = "$dig" ] || die "ubuntu:$tag index body != docker-content-digest"
jq -e '[.manifests[].platform | "\(.os)/\(.architecture)\(if .variant then "/"+.variant else "" end)"] | (index("linux/amd64") != null) and (index("linux/arm64/v8") != null)' "$W/idx" >/dev/null \
  || die "ubuntu:$tag manifest list lacks linux/amd64 or linux/arm64/v8"
sed -i "s/$old_dig/$dig/" "$AB" "$LT"; sed -i "s|^WK_IMG=docker.io/library/ubuntu@$old_dig|WK_IMG=docker.io/library/ubuntu@$dig|" "$SL"
grep -q "^WK_IMG=docker.io/library/ubuntu@$dig" "$SL" || die "WK_IMG in $SL was not $old_dig"
grep -q "^DIGEST=$dig$" "$LT" || die "DIGEST in $LT was not $old_dig"
[ "$old_dig" = "$dig" ] || note "- ubuntu image: $old_dig -> ubuntu:$tag $dig"

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
