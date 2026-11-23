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
# grype-install.sh: pin-file validation happens before any download
gi() { GRYPE_PINS=$t/p "$here/grype-install.sh" "$t/gi" >/dev/null 2>&1; }
printf 'version v0.120.1\nsha256 amd64 abc\n' >"$t/p"; ! gi && ok "malformed sha256 refused" || bad "malformed sha256 accepted"
printf 'version v1\nversion v2\n' >"$t/p"; ! gi && ok "two versions refused" || bad "two versions accepted"
printf 'version v0.120.1\n' >"$t/p"; ! gi && ok "missing arch digest refused" || bad "missing digest accepted"
[ ! -e "$t/gi/grype" ] && ok "no binary left behind" || bad "binary left behind"
grep -Eq '^version v0\.120\.1$' "$here/grype.pins" && [ "$(grep -Ec '^sha256 (amd64|arm64) [0-9a-f]{64}$' "$here/grype.pins")" = 2 ] \
  && ok "grype.pins: v0.120.1 + 2 digests" || bad "grype.pins shape"
exit $fails
