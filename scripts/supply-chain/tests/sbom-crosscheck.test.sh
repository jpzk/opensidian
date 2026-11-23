#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# sbom-crosscheck.sh (#299) on fixture scanner JSON (no network, no scanner binary): alias / upstream /
# DEBIAN-CVE matching across osv-scanner and grype, version-sensitive keys, one-sided findings, and
# fail-closed on malformed input. Also grype-install.sh pin-file validation (no download reached).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
X=$here/sbom-crosscheck.sh

cat >"$t/osv.json" <<'J'
{"results":[{"packages":[
 {"package":{"name":"cookie","version":"0.18.2","ecosystem":"crates.io"},
  "vulnerabilities":[{"id":"RUSTSEC-2026-0594","aliases":["GHSA-8wcq-xq25-7fhm"],
   "affected":[{"ranges":[{"events":[{"introduced":"0"},{"fixed":"0.18.3"}]}]}]}]},
 {"package":{"name":"libxml2","version":"2.14.5","ecosystem":"Ubuntu:26.04"},
  "vulnerabilities":[{"id":"UBUNTU-CVE-2026-1111","upstream":["CVE-2026-1111"]}]},
 {"package":{"name":"url","version":"2.5.8","ecosystem":"crates.io"},
  "vulnerabilities":[{"id":"RUSTSEC-2026-0592"}]},
 {"package":{"name":"Brotli","version":"1.1.0","ecosystem":"GIT"},
  "vulnerabilities":[{"id":"CVE-2025-6176"}]}
]}]}
J
cat >"$t/grype.json" <<'J'
{"matches":[
 {"vulnerability":{"id":"GHSA-8wcq-xq25-7fhm","severity":"Medium","fix":{"versions":["0.18.3"]}},
  "relatedVulnerabilities":[],"artifact":{"name":"cookie","version":"0.18.2","type":"rust-crate"}},
 {"vulnerability":{"id":"CVE-2026-1111","severity":"High","fix":{"versions":[]}},
  "artifact":{"name":"libxml2","version":"2.14.5","type":"deb"}},
 {"vulnerability":{"id":"CVE-2026-2222","severity":"Low","fix":{"versions":[]}},
  "artifact":{"name":"harfbuzz","version":"11.0.0","type":"deb"}},
 {"vulnerability":{"id":"CVE-2025-6176","severity":"High","fix":{"versions":["1.2.0"]}},
  "artifact":{"name":"brotli","version":"1.0.9","type":"unknown"}}
]}
J
"$X" norm-osv "$t/osv.json" L >"$t/o.tsv" && ok "norm-osv" || bad "norm-osv rc"
"$X" norm-grype "$t/grype.json" L >"$t/g.tsv" && ok "norm-grype" || bad "norm-grype rc"
[ "$(wc -l <"$t/o.tsv")" = 4 ] && ok "osv rows = 4" || bad "osv rows $(wc -l <"$t/o.tsv")"
grep -q $'\tcve-2026-1111\|CVE-2026-1111,UBUNTU-CVE-2026-1111\t' "$t/o.tsv" && ok "UBUNTU-CVE expands to CVE" || bad "UBUNTU-CVE not expanded: $(cat "$t/o.tsv")"
grep -q $'\t0.18.3$' "$t/o.tsv" && ok "osv fixed version kept" || bad "osv fixed version lost"
"$X" diff "$t/o.tsv" "$t/g.tsv" >"$t/d.tsv" || bad "diff rc"
chk() { grep -q "^$1"$'\t'"L"$'\t'"[^"$'\t'"]*"$'\t'"$2"$'\t' "$t/d.tsv" && ok "$1 $2" || { cat "$t/d.tsv"; bad "want $1 $2"; }; }
chk both cookie              # RUSTSEC (osv) == GHSA alias (grype)
chk both libxml2             # UBUNTU-CVE upstream == CVE
chk osv-only url             # only osv knows the RUSTSEC id
chk grype-only harfbuzz      # only grype reports it
chk osv-only brotli          # same CVE, different version -> NOT a match
chk grype-only brotli
[ "$(grep -c . "$t/d.tsv")" = 6 ] && ok "6 diff rows (matched grype rows not repeated)" || bad "diff rows $(grep -c . "$t/d.tsv")"
: >"$t/empty.tsv"
"$X" diff "$t/empty.tsv" "$t/g.tsv" >"$t/d2.tsv" && [ "$(grep -c '^grype-only' "$t/d2.tsv")" = 4 ] \
  && ok "empty osv side: every grype row is grype-only" || bad "empty side mis-assigned: $(cat "$t/d2.tsv")"
"$X" summary "$t/d.tsv" | grep -q '^| L | 2 | 2 | 2 |$' && ok summary || bad "summary: $("$X" summary "$t/d.tsv")"
# Maven: osv-scanner names a package group:artifact, grype names it by artifactId only (CI run
# 45169073492: kotlin-gradle-plugin GHSA-r937-wjx7-w2jp showed up as osv-only AND grype-only).
cat >"$t/mo.json" <<'J'
{"results":[{"packages":[
 {"package":{"name":"org.jetbrains.kotlin:kotlin-gradle-plugin","version":"1.9.25","ecosystem":"Maven"},
  "vulnerabilities":[{"id":"GHSA-r937-wjx7-w2jp","aliases":["CVE-2026-53914"]}]},
 {"package":{"name":"org.example:only-osv","version":"1.0","ecosystem":"Maven"},
  "vulnerabilities":[{"id":"GHSA-xxxx-xxxx-xxxx"}]}
]}]}
J
cat >"$t/mg.json" <<'J'
{"matches":[
 {"vulnerability":{"id":"GHSA-r937-wjx7-w2jp","severity":"Medium","fix":{"versions":[]}},
  "artifact":{"name":"kotlin-gradle-plugin","version":"1.9.25","type":"java-archive",
   "purl":"pkg:maven/org.jetbrains.kotlin/kotlin-gradle-plugin@1.9.25"}}
]}
J
"$X" norm-osv "$t/mo.json" M >"$t/mo.tsv" && "$X" norm-grype "$t/mg.json" M >"$t/mg.tsv" \
  && "$X" diff "$t/mo.tsv" "$t/mg.tsv" >"$t/md.tsv" || bad "maven fixture rc"
grep -q $'^both\tM\tMaven\tkotlin-gradle-plugin\t1.9.25\t' "$t/md.tsv" && ok "maven group:artifact == grype artifactId" \
  || bad "maven not matched: $(cat "$t/md.tsv")"
! grep -q '^grype-only' "$t/md.tsv" && ok "maven: no spurious grype-only" || bad "maven spurious grype-only"
grep -q $'^osv-only\tM\tMaven\tonly-osv\t' "$t/md.tsv" && ok "maven one-sided still reported" || bad "maven one-sided lost"
# fail closed
echo '{"matches":"x"}' >"$t/bad.json"
! "$X" norm-grype "$t/bad.json" L >/dev/null 2>&1 && ok "malformed grype JSON fails" || bad "malformed grype JSON accepted"
echo 'not json' >"$t/bad2.json"
! "$X" norm-osv "$t/bad2.json" L >/dev/null 2>&1 && ok "non-JSON osv fails" || bad "non-JSON osv accepted"
printf 'a\tb\n' >"$t/short.tsv"
! "$X" diff "$t/short.tsv" "$t/g.tsv" >/dev/null 2>&1 && ok "short TSV row fails" || bad "short TSV accepted"
! SBOMS=x.cdx.json "$X" fetch latest "$t/f" >/dev/null 2>&1 && ok "fetch rejects non-semver tag" || bad "fetch accepted bad tag"
! SBOMS='../x.cdx.json' "$X" fetch v0.9.6 "$t/f" >/dev/null 2>&1 && ok "fetch rejects path in asset" || bad "fetch accepted ../"
! SBOMS='' "$X" fetch v0.9.6 "$t/f" >/dev/null 2>&1 && ok "fetch rejects empty SBOMS" || bad "fetch accepted empty"
# fetch-run (head SBOMs of one Release run) against a stub gh: run metadata + artifact list + zip
mkdir -p "$t/fr/bin" "$t/fr/src"
for a in x86_64 aarch64 macos-x86_64; do
  n=opensidian-0.9.7-$a.cdx.json
  echo '{"bomFormat":"CycloneDX","components":[{"name":"c","components":[{"name":"nested"}]}]}' >"$t/fr/src/$n"
  (cd "$t/fr/src" && sha256sum "$n" >"$n.sha256" && zip -q "../$a.zip" "$n" "$n.sha256")
done
dg() { echo "sha256:$(sha256sum "$t/fr/$1.zip" | cut -c1-64)"; }
cat >"$t/fr/bin/gh" <<'G'
#!/usr/bin/env bash
# stub: $FR_PATH / $FR_SHA = run metadata, $FR_ARTS = artifact TSV (id digest name), $FR_DIR = zips by id
case "$2" in
  */artifacts/*/zip) id=${2%/zip}; id=${id##*/}; cat "$FR_DIR/$id.zip" ;;
  */artifacts\?*) printf '%s\n' "$FR_ARTS" ;;
  repos/*/actions/runs/*) echo "$FR_PATH $FR_SHA" ;;
  *) exit 1 ;;
esac
G
chmod +x "$t/fr/bin/gh"
S=0123456789abcdef0123456789abcdef01234567
cp "$t/fr/x86_64.zip" "$t/fr/1.zip"; cp "$t/fr/aarch64.zip" "$t/fr/2.zip"; cp "$t/fr/macos-x86_64.zip" "$t/fr/3.zip"
arts=$(printf '%s\t%s\t%s\n' 1 "$(dg 1)" sbom-0.9.7-x86_64 2 "$(dg 2)" sbom-0.9.7-aarch64 3 "$(dg 3)" sbom-macos-x86_64)
fr() { PATH=$t/fr/bin:$PATH FR_DIR=$t/fr FR_PATH=${P:-.github/workflows/release.yml} FR_SHA=${H:-$S} FR_ARTS=${A:-$arts} \
       "$X" fetch-run 42 "$S" "$1"; }
r=$(fr "$t/fr/o1" 2>/dev/null) && [ "$r" = 42 ] && [ "$(ls "$t/fr/o1"/*.cdx.json | wc -l)" = 3 ] \
  && ok "fetch-run: release run @ sha -> 3 SBOMs (2 linux + macOS), digests + .sha256 verified" || bad "fetch-run good case"
! H=fedcba9876543210fedcba9876543210fedcba98 fr "$t/fr/o2" >/dev/null 2>&1 && ok "fetch-run: run built another sha -> fail" || bad "fetch-run accepted other sha"
! P=.github/workflows/ci.yml fr "$t/fr/o3" >/dev/null 2>&1 && ok "fetch-run: not a release.yml run -> fail" || bad "fetch-run accepted non-release run"
cp "$t/fr/aarch64.zip" "$t/fr/1.zip"   # artifact 1 now differs from its listed digest
! fr "$t/fr/o4" >/dev/null 2>&1 && ok "fetch-run: zip != artifact digest -> fail" || bad "fetch-run accepted digest mismatch"
cp "$t/fr/x86_64.zip" "$t/fr/1.zip"
! A=$(printf '%s\n' "$arts" | grep -v aarch64) fr "$t/fr/o5" >/dev/null 2>&1 && ok "fetch-run: linux aarch64 SBOM missing -> fail" || bad "fetch-run accepted missing arch"
! A=$(printf '%s\n' "$arts" | grep -v sbom-0.9.7-x86_64) fr "$t/fr/o6" >/dev/null 2>&1 && ok "fetch-run: only macOS x86_64 (no linux x86_64) -> fail" || bad "fetch-run took macOS for linux"
! "$X" fetch-run 4x2 "$S" "$t/fr/o7" >/dev/null 2>&1 && ok "fetch-run: bad run id refused" || bad "fetch-run bad run id"
! "$X" fetch-run 42 HEAD "$t/fr/o8" >/dev/null 2>&1 && ok "fetch-run: non-sha refused" || bad "fetch-run bad sha"
# grype-install.sh: pin-file validation happens before any download
gi() { GRYPE_PINS=$t/p "$here/grype-install.sh" "$t/gi" >/dev/null 2>&1; }
printf 'version v0.120.1\nsha256 amd64 abc\n' >"$t/p"; ! gi && ok "malformed sha256 refused" || bad "malformed sha256 accepted"
printf 'version v1\nversion v2\n' >"$t/p"; ! gi && ok "two versions refused" || bad "two versions accepted"
printf 'version v0.120.1\n' >"$t/p"; ! gi && ok "missing arch digest refused" || bad "missing digest accepted"
[ ! -e "$t/gi/grype" ] && ok "no binary left behind" || bad "binary left behind"
grep -Eq '^version v0\.120\.1$' "$here/grype.pins" && [ "$(grep -Ec '^sha256 (amd64|arm64) [0-9a-f]{64}$' "$here/grype.pins")" = 2 ] \
  && ok "grype.pins: v0.120.1 + 2 digests" || bad "grype.pins shape"
exit $fails
