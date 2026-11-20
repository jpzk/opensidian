#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# sbom-scan.sh — release SBOMs (*.cdx.json) -> GitHub code scanning (SARIF) + dependency graph
# (operator 2026-11-20: private Security tab + Dependency submission API, NO public issues).
# Used by .github/workflows/sbom-scan.yml; every subcommand fails closed (rc != 0).
#
#   fetch <outdir> <spec>...   download SBOMs + their .sha256 assets, verify, write <outdir>/manifest.tsv
#        spec: latest            the latest PUBLISHED release
#              drafts            every draft release that has *.cdx.json assets; when the token sees
#                                no draft at all (GITHUB_TOKEN without push access does not list
#                                drafts), falls back to the sbom-* artifacts of the latest successful
#                                release.yml run (actions API, artifact zip checked against its digest)
#              release:<id|tag>  one release by numeric id or tag name (drafts included)
#              artifacts         the release.yml artifact fallback explicitly
#        A release without *.cdx.json assets is logged and skipped; a .cdx.json without its
#        .sha256 asset, a sha256 mismatch, or zero SBOMs in total -> rc 1.
#        manifest.tsv: source  release  path  platform  arch  sha256
#   verify <file> <file.sha256>        sha256 of <file> == the digest in <file.sha256> (else rc 1)
#   platform <file-name>               "<platform> <arch>" (opensidian-<v>-[<platform>-]<arch>.cdx.json)
#   scan <osv-scanner> <manifest.tsv> <outdir>
#        osv-scanner (pinned binary) on every SBOM with FRESH offline advisory DBs downloaded into an
#        empty cache by this run (--offline-vulnerabilities --download-offline-databases), once as
#        json and once as sarif against the same DB; then enrich + merge per category into
#        <outdir>/sbom-<platform>-<arch>.sarif and write <outdir>/summary.md; logs the DB revision.
#   dbrev <cachedir> <eco>...          log size/sha256/newest-record time of each fetched DB; rc 1 if
#                                      one is missing/empty or older than OSV_DB_MAX_AGE_DAYS (3)
#   enrich <osv.sarif> <osv.json> <uri> [label]
#        stdout: SARIF whose rules all carry tags [security, vulnerability] and security-severity
#        when ANY real severity is known (osv max_severity = CVSS score; else the Ubuntu priority
#        -> lower bound of GitHub's bucket, marked severity-source=ubuntu-priority), whose results
#        carry properties {package, version, severity, fix-available, fixed-versions} + the fixed
#        version (or "no fixed version yet") in the message, level error/warning/note by severity,
#        and whose locations point at <uri> (a repo-relative pseudo path for the release asset).
#   merge <category> <enriched.sarif>... stdout: one SARIF run (rules union, results concat)
#   summary <label> <enriched.sarif>     stdout: one markdown list line with severity / fix counts
#   snapshot <sha> <ref> <correlator> <scanned-iso> <manifest-name>=<sbom>...
#        stdout: dependency submission snapshot; packages by purl, ONLY pkg:npm + pkg:cargo (the
#        ecosystems Dependabot supports here), everything else counted on stderr as skipped.
#   submit <snapshot.json>             POST /repos/{repo}/dependency-graph/snapshots; rc 1 unless 201
#
# env: GH_TOKEN, GITHUB_REPOSITORY (owner/repo), GITHUB_API_URL (default https://api.github.com),
#      OSV_DB_MAX_AGE_DAYS (default 3), NOW (epoch, tests).
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
api=${GITHUB_API_URL:-https://api.github.com}
log() { printf '%s\n' "$*" >&2; }
die() { log "::error::sbom-scan: $*"; exit 1; }

# --- GitHub API (JSON on stdout, never -o: downloads below are verified) -------------------
gh_json() { # gh_json <path>
  [ -n "${GH_TOKEN:-}" ] || die "GH_TOKEN not set"
  curl -fsS --proto '=https' --tlsv1.2 --retry 3 \
    -H "Authorization: Bearer $GH_TOKEN" -H 'Accept: application/vnd.github+json' \
    -H 'X-GitHub-Api-Version: 2022-11-28' "$api$1"
}
repo() { [ -n "${GITHUB_REPOSITORY:-}" ] || die "GITHUB_REPOSITORY not set"; printf '%s' "$GITHUB_REPOSITORY"; }

cmd_verify() {
  local f=${1:?verify <file> <file.sha256>} s=${2:?verify <file> <file.sha256>} want got
  [ -s "$f" ] || die "verify: $f missing or empty"
  [ -s "$s" ] || die "verify: checksum file $s missing or empty (refusing an unverified SBOM)"
  want=$(awk 'NR==1{print $1}' "$s")
  grep -Eqx '[0-9a-f]{64}' <<<"$want" || die "verify: $s does not start with a sha256 digest"
  got=$(sha256sum "$f" | awk '{print $1}')
  [ "$got" = "$want" ] || die "verify: sha256 MISMATCH for $f: got $got, $s says $want"
  log "verify: $(basename "$f") sha256 $got OK"
  printf '%s\n' "$got"
}

cmd_platform() {
  local n=${1:?platform <file-name>} b arch plat
  b=$(basename "$n" .cdx.json)
  arch=${b##*-}
  case "$arch" in x86_64|amd64) arch=x86_64 ;; aarch64|arm64) arch=aarch64 ;; universal) ;;
    *) die "platform: cannot read the architecture from '$n'" ;; esac
  b=${b%-*}
  case "${b##*-}" in macos|darwin) plat=macos ;; windows|win) plat=windows ;; linux) plat=linux ;; *) plat=linux ;; esac
  printf '%s %s\n' "$plat" "$arch"
}

# one release object (JSON on stdin) -> download + verify every *.cdx.json, append manifest rows
fetch_release() { # fetch_release <outdir> <source>  (release JSON on stdin)
  local out=$1 src=$2 rel tag id n name aid sid dir sha plat arch
  rel=$(cat)
  tag=$(jq -r '.tag_name' <<<"$rel"); id=$(jq -r '.id' <<<"$rel")
  n=$(jq '[.assets[] | select(.name | endswith(".cdx.json"))] | length' <<<"$rel")
  if [ "$n" -eq 0 ]; then log "fetch: release $tag (id $id, $src) has no *.cdx.json asset: nothing to scan"; return 0; fi
  dir="$out/$src-$tag"; mkdir -p "$dir"
  while IFS=$'\t' read -r name aid; do
    sid=$(jq -r --arg s "$name.sha256" '[.assets[] | select(.name == $s) | .id][0] // empty' <<<"$rel")
    [ -n "$sid" ] || die "fetch: $tag asset $name has no $name.sha256 asset (fail closed)"
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -H "Authorization: Bearer $GH_TOKEN" \
      -H 'Accept: application/octet-stream' -o "$dir/$name.sha256" "$api/repos/$(repo)/releases/assets/$sid"
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -H "Authorization: Bearer $GH_TOKEN" \
      -H 'Accept: application/octet-stream' -o "$dir/$name" "$api/repos/$(repo)/releases/assets/$aid"
    sha256sum "$dir/$name" >/dev/null
    sha=$(cmd_verify "$dir/$name" "$dir/$name.sha256")
    read -r plat arch < <(cmd_platform "$name")
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$src" "$tag" "$dir/$name" "$plat" "$arch" "$sha" >>"$out/manifest.tsv"
    log "fetch: $src $tag $name -> sbom-$plat-$arch"
  done < <(jq -r '.assets[] | select(.name | endswith(".cdx.json")) | [.name, (.id|tostring)] | @tsv' <<<"$rel")
}

fetch_artifacts() { # fetch_artifacts <outdir>
  local out=$1 run arts name aid dig zip dir f sha plat arch n=0
  run=$(gh_json "/repos/$(repo)/actions/workflows/release.yml/runs?status=success&per_page=1" | jq -r '.workflow_runs[0].id // empty')
  [ -n "$run" ] || die "fetch artifacts: no successful release.yml run"
  arts=$(gh_json "/repos/$(repo)/actions/runs/$run/artifacts?per_page=100")
  while IFS=$'\t' read -r name aid dig; do
    [ -n "$name" ] || continue
    dir="$out/artifact-$run-$name"; mkdir -p "$dir"; zip="$dir.zip"
    grep -Eqx 'sha256:[0-9a-f]{64}' <<<"$dig" || die "fetch artifacts: $name (run $run) has no sha256 digest"
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -H "Authorization: Bearer $GH_TOKEN" \
      -o "$zip" "$api/repos/$(repo)/actions/artifacts/$aid/zip"
    echo "${dig#sha256:}  $zip" | sha256sum -c - >&2 || die "fetch artifacts: $name zip does not match its digest $dig"
    unzip -q -o "$zip" -d "$dir"; rm -f "$zip"
    for f in "$dir"/*.cdx.json; do
      [ -e "$f" ] || die "fetch artifacts: $name holds no *.cdx.json"
      sha=$(cmd_verify "$f" "$f.sha256")
      read -r plat arch < <(cmd_platform "$f")
      printf '%s\t%s\t%s\t%s\t%s\t%s\n' "artifact" "run-$run" "$f" "$plat" "$arch" "$sha" >>"$out/manifest.tsv"
      n=$((n + 1)); log "fetch: artifact $name (release.yml run $run) $(basename "$f") -> sbom-$plat-$arch"
    done
  done < <(jq -r '.artifacts[] | select((.name | startswith("sbom-")) and (.name | startswith("sbom-logs-") | not) and (.expired | not)) | [.name, (.id|tostring), (.digest // "")] | @tsv' <<<"$arts")
  [ "$n" -gt 0 ] || die "fetch artifacts: release.yml run $run has no unexpired sbom-* artifact"
}

cmd_fetch() {
  local out=${1:?fetch <outdir> <spec>...}; shift
  [ $# -gt 0 ] || die "fetch: no spec"
  mkdir -p "$out"; : >"$out/manifest.tsv"
  local spec all ndraft sel
  for spec in "$@"; do
    case "$spec" in
      latest)
        gh_json "/repos/$(repo)/releases/latest" | fetch_release "$out" published ;;
      drafts)
        all=$(gh_json "/repos/$(repo)/releases?per_page=100")
        ndraft=$(jq '[.[] | select(.draft)] | length' <<<"$all")
        if [ "$ndraft" -eq 0 ]; then
          log "fetch: this token lists NO draft release (no push access, or no draft): using the release.yml artifact fallback"
          fetch_artifacts "$out"
        else
          while IFS= read -r sel; do fetch_release "$out" draft <<<"$sel"; done < <(jq -c '.[] | select(.draft)' <<<"$all")
        fi ;;
      release:*)
        sel=${spec#release:}
        all=$(gh_json "/repos/$(repo)/releases?per_page=100")
        all=$(jq -c --arg s "$sel" '[.[] | select((.id|tostring) == $s or .tag_name == $s)][0] // empty' <<<"$all")
        [ -n "$all" ] || die "fetch: release '$sel' not found (or a draft this token cannot see)"
        fetch_release "$out" "$(jq -r 'if .draft then "draft" else "published" end' <<<"$all")" <<<"$all" ;;
      artifacts) fetch_artifacts "$out" ;;
      *) die "fetch: unknown spec '$spec'" ;;
    esac
  done
  # same bytes reachable twice (published == the draft it came from): keep one row per sha256
  awk -F'\t' '!seen[$6]++' "$out/manifest.tsv" >"$out/manifest.tmp" && mv "$out/manifest.tmp" "$out/manifest.tsv"
  [ -s "$out/manifest.tsv" ] || die "fetch: zero SBOMs found for: $* (fail closed)"
  log "fetch: $(wc -l <"$out/manifest.tsv") SBOM(s) verified"
}

cmd_dbrev() {
  local c=${1:?dbrev <cachedir> <eco>...}; shift
  local max=${OSV_DB_MAX_AGE_DAYS:-3} now=${NOW:-$(date -u +%s)} eco z newest ts
  for eco in "$@"; do
    z="$c/osv-scalibr/$eco/all.zip"
    [ -s "$z" ] || die "advisory DB $eco not fetched ($z missing/empty): failing, no stale-cache pass"
    newest=$(unzip -Z -T "$z" | awk '$NF ~ /\.json$/ {print $(NF-1)}' | sort | tail -1)
    [ -n "$newest" ] || die "advisory DB $eco ($z) holds no records"
    if [ "${newest:0:4}" -ge 2000 ] 2>/dev/null; then
      ts=$(date -u -d "${newest:0:8} ${newest:9:2}:${newest:11:2}:${newest:13:2}" +%s) || die "advisory DB $eco: bad time '$newest'"
    else
      # entries without zip times (e.g. crates.io: DOS epoch 1980-00-00): newest record "modified" instead
      newest=$(unzip -p "$z" '*.json' | jq -rn '[inputs | .modified // empty] | max // empty')
      [ -n "$newest" ] || die "advisory DB $eco ($z): no zip times and no record 'modified'"
      ts=$(date -u -d "$newest" +%s) || die "advisory DB $eco: bad modified time '$newest'"
    fi
    echo "osv advisory DB: $eco size $(stat -c %s "$z") sha256 $(sha256sum "$z" | awk '{print $1}') newest record $(date -u -d "@$ts" +%Y-%m-%dT%H:%M:%SZ) (fetched by this run, checked $(date -u -d "@$now" +%Y-%m-%dT%H:%M:%SZ))"
    [ $(( (now - ts) / 86400 )) -le "$max" ] || die "advisory DB $eco newest record is older than $max days: stale"
  done
}

# purl types in an SBOM -> osv offline DB directory names
sbom_ecos() { jq -r '[.components[]?.purl // empty | capture("^pkg:(?<t>[a-z]+)/(?<ns>[^/@]*)") |
  if .t == "npm" then "npm" elif .t == "cargo" then "crates.io" elif .t == "deb" and .ns == "ubuntu" then "Ubuntu"
  elif .t == "deb" and .ns == "debian" then "Debian" else empty end] | unique[]' "$1"; }

cmd_enrich() {
  local s=${1:?enrich <osv.sarif> <osv.json> <uri> [label]} j=${2:?} uri=${3:?} label=${4:-}
  jq -e '.runs | length == 1' "$s" >/dev/null || die "enrich: $s is not a one-run SARIF"
  jq -e '.results | type == "array"' "$j" >/dev/null || die "enrich: $j is not osv-scanner json"
  jq --slurpfile J "$j" --arg uri "$uri" --arg label "$label" '
    def bucket: if . == null then "unknown" elif . >= 9 then "critical" elif . >= 7 then "high"
                elif . >= 4 then "medium" elif . > 0 then "low" else "unknown" end;
    def prio: {"critical": 9.0, "high": 7.0, "medium": 4.0, "low": 0.1}[. // ""];
    def num: if type == "string" and test("^[0-9]+(\\.[0-9]+)?$") then tonumber else null end;
    ($J[0]) as $o
    # id/alias -> {score, src}
    | ([ $o.results[]?.packages[]? as $p | $p.groups[]? as $g
         | ($g.ids + ($g.aliases // []))[] as $id
         | ([ $p.vulnerabilities[]? | select(.id as $v | ($g.ids + ($g.aliases // [])) | index($v))
              | .severity[]? | select(.type == "Ubuntu") | .score | ascii_downcase | prio ] | map(select(. != null)) | max) as $up
         | { key: $id, value: (if ($g.max_severity | num) != null then {score: ($g.max_severity | num), src: "osv-max-cvss"}
                                elif $up != null then {score: $up, src: "ubuntu-priority"} else null end) } ]
       | map(select(.value != null)) | from_entries) as $sev
    # "name@version|id" -> fixed versions (same package + ecosystem, as osv-fixed-gate.sh)
    | ([ $o.results[]?.packages[]? as $p | $p.vulnerabilities[]? as $v
         | ([ $v.affected[]? | select(.package.name == $p.package.name and (.package.ecosystem == $p.package.ecosystem
                or (.package.ecosystem | startswith($p.package.ecosystem + ":"))))
              | .ranges[]?.events[]? | .fixed // empty ] | unique) as $fx
         | ([$v.id] + ($v.aliases // []))[] as $id
         | { key: "\($p.package.name)@\($p.package.version)|\($id)", value: $fx } ]
       | group_by(.key) | map({key: .[0].key, value: (map(.value) | add | unique)}) | from_entries) as $fix
    | .runs[0] |= (
        .tool.driver.rules |= map(
          . as $r | ($sev[$r.id] // ([($r.deprecatedIds // [])[] | $sev[.] // empty] | max_by(.score))) as $s
          | .properties = ((.properties // {}) + {tags: (((.properties.tags // []) + ["security", "vulnerability"]) | unique)})
          | if $s != null then .properties += {"security-severity": ($s.score | tostring), "severity-source": $s.src} else . end)
      | (.tool.driver.rules | map({key: .id, value: (.properties["security-severity"] // null)}) | from_entries) as $rs
      | (.tool.driver.rules | map({key: .id, value: ([.id] + (.deprecatedIds // []))}) | from_entries) as $ids
      | .results |= map(
          (.message.text | capture("^Package '\''(?<pkg>.+)@(?<ver>[^@'\'']+)'\'' is vulnerable to") // {pkg: null, ver: null}) as $m
          | ($m.pkg == null) as $unparsed
          | ([ ($ids[.ruleId] // [.ruleId])[] as $id | $fix["\($m.pkg)@\($m.ver)|\($id)"] // [] ] | add // [] | unique) as $fx
          | (($rs[.ruleId] // null) | if . == null then null else tonumber end | bucket) as $b
          | if $unparsed then error("enrich: unexpected osv-scanner message: \(.message.text)") else . end
          | .level = ({"critical": "error", "high": "error", "medium": "warning", "low": "note", "unknown": "warning"}[$b])
          | .message.text = (.message.text
              + (if ($fx | length) > 0 then " Fixed in \($fx | join(", "))." else " No fixed version yet." end)
              + (if $label != "" then " [\($label)]" else "" end))
          | .properties = ((.properties // {}) + {package: $m.pkg, version: $m.ver, severity: $b,
              "fix-available": (($fx | length) > 0), "fixed-versions": $fx})
          | .locations = [{physicalLocation: {artifactLocation: {uri: $uri}, region: {startLine: 1}}}]
          | del(.ruleIndex))
      | del(.artifacts))
  ' "$s"
}

cmd_merge() {
  local cat=${1:?merge <category> <enriched.sarif>...}; shift
  [ $# -gt 0 ] || die "merge: no input"
  jq -s --arg cat "$cat" '. as $all
    | (.[0] | del(.runs)) + {runs: [ .[0].runs[0]
      | .tool.driver.rules = ([ $all[].runs[0].tool.driver.rules[] ] | unique_by(.id))
      | .results = [ $all[].runs[0].results[] ]
      | .automationDetails = {id: ($cat + "/")} ]}
  ' "$@"
}

cmd_summary() {
  local label=${1:?summary <label> <enriched.sarif>} f=${2:?}
  jq -r --arg l "$label" '.runs[0].results as $r
    | ($r | map(.properties.severity)) as $s
    | def c($x): ($s | map(select(. == $x)) | length);
    "- **\($l)**: \($r | length) findings | critical \(c("critical")), high \(c("high")), medium \(c("medium")), low \(c("low")), unknown \(c("unknown")) | fix available \($r | map(select(.properties["fix-available"])) | length), no fix yet \($r | map(select(.properties["fix-available"] | not)) | length)"' "$f"
}

cmd_scan() {
  local osv=${1:?scan <osv-scanner> <manifest.tsv> <outdir>} man=${2:?} out=${3:?}
  [ -x "$osv" ] || die "scan: $osv not executable"
  [ -s "$man" ] || die "scan: empty manifest $man"
  mkdir -p "$out/raw"
  local cache; cache=$(mktemp -d); trap 'rm -rf "$cache"' RETURN
  export OSV_SCANNER_LOCAL_DB_CACHE_DIRECTORY=$cache
  local src rel path plat arch sha i=0 uri cat ecos=() e
  : >"$out/summary.md"
  while IFS=$'\t' read -r src rel path plat arch sha; do
    i=$((i + 1)); cat="sbom-$plat-$arch"; uri="release-assets/$rel/$(basename "$path")"
    log "[$(date -u +%H:%M:%S)] scan $i: $rel $(basename "$path") ($cat)"
    "$here/osv-json.sh" "$out/raw/$i.json" "$osv" scan source --offline-vulnerabilities --download-offline-databases \
      --format json -L "$path" >&2 || die "scan: osv-scanner json failed on $path"
    set +e; "$osv" scan source --offline-vulnerabilities --download-offline-databases --format sarif -L "$path" >"$out/raw/$i.sarif"; e=$?; set -e
    case $e in 0|1) ;; *) die "scan: osv-scanner sarif failed (rc=$e) on $path" ;; esac
    jq -e '.runs[0].tool.driver.name == "osv-scanner"' "$out/raw/$i.sarif" >/dev/null || die "scan: $path: not osv-scanner SARIF"
    [ "$(jq '[.results[].packages[]?.groups[]?] | length' "$out/raw/$i.json")" = "$(jq '.runs[0].tool.driver.rules | length' "$out/raw/$i.sarif")" ] \
      || log "scan: note: json groups != sarif rules for $path (osv-scanner dedups per id)"
    cmd_enrich "$out/raw/$i.sarif" "$out/raw/$i.json" "$uri" "$src $rel" >"$out/raw/$i.enriched.sarif"
    printf '%s\t%s\n' "$cat" "$out/raw/$i.enriched.sarif" >>"$out/raw/cats.tsv"
    cmd_summary "$cat ($src $rel, $(basename "$path"), sha256 ${sha:0:12})" "$out/raw/$i.enriched.sarif" >>"$out/summary.md"
    while IFS= read -r e; do ecos+=("$e"); done < <(sbom_ecos "$path")
  done <"$man"
  mapfile -t ecos < <(printf '%s\n' "${ecos[@]}" | sort -u)
  [ "${#ecos[@]}" -gt 0 ] || die "scan: no scannable ecosystem in any SBOM"
  cmd_dbrev "$cache" "${ecos[@]}" | tee "$out/dbrev.txt"
  for cat in $(cut -f1 "$out/raw/cats.tsv" | sort -u); do
    # shellcheck disable=SC2046
    cmd_merge "$cat" $(awk -F'\t' -v c="$cat" '$1 == c {print $2}' "$out/raw/cats.tsv") >"$out/$cat.sarif"
    log "scan: $out/$cat.sarif: $(jq '.runs[0].results | length' "$out/$cat.sarif") results, $(jq '[.runs[0].tool.driver.rules[] | select(.properties["security-severity"])] | length' "$out/$cat.sarif")/$(jq '.runs[0].tool.driver.rules | length' "$out/$cat.sarif") rules with security-severity"
  done
}

cmd_snapshot() {
  local sha=${1:?snapshot <sha> <ref> <correlator> <scanned-iso> <name>=<sbom>...} ref=${2:?} corr=${3:?} scanned=${4:?}; shift 4
  [ $# -gt 0 ] || die "snapshot: no SBOM"
  grep -Eqx '[0-9a-f]{40}' <<<"$sha" || die "snapshot: sha '$sha' is not a 40-hex commit"
  case "$ref" in refs/heads/*|refs/tags/*) ;; *) die "snapshot: ref '$ref' is not a full ref" ;; esac
  # manifests go through files, not argv: real SBOMs exceed ARG_MAX as --argjson
  local arg name f m tmp i=0
  tmp=$(mktemp -d); trap 'rm -rf "$tmp"' RETURN
  for arg in "$@"; do
    name=${arg%%=*}; f=${arg#*=}
    [ -s "$f" ] || die "snapshot: $f missing"
    jq -e '.bomFormat == "CycloneDX"' "$f" >/dev/null || die "snapshot: $f is not CycloneDX"
    jq -r --arg n "$name" '[.components[]? | (.purl // "") | (capture("^pkg:(?<t>[a-z]+)/").t // "no-purl")
        | select(. != "npm" and . != "cargo")] | group_by(.) | map("snapshot: \($n): skipped \(length) pkg:\(.[0]) component(s) (not an ecosystem Dependabot supports here)")[]' "$f" >&2
    m=$(jq --arg n "$name" '
      ([.components[]? | select(.purl // "" | test("^pkg:(npm|cargo)/"))]) as $keep
      | ($keep | map(.["bom-ref"] // .purl) ) as $refs
      | ($keep | map({key: (.["bom-ref"] // .purl), value: .purl}) | from_entries) as $p
      | ((.dependencies // []) | map({key: .ref, value: (.dependsOn // [])}) | from_entries) as $dep
      | ([.dependencies[]? | select(.ref as $r | $p[$r] == null) | .dependsOn[]?] | unique) as $direct
      | {($n): {name: $n, file: {source_location: $n},
          resolved: ($keep | map({key: .purl, value: {package_url: .purl,
              relationship: (if ((.["bom-ref"] // .purl) as $r | $direct | index($r)) then "direct" else "indirect" end),
              scope: "runtime",
              dependencies: ([$dep[(.["bom-ref"] // .purl)][]? | $p[.] // empty] | unique)}}) | from_entries)}}' "$f")
    [ "$(jq --arg n "$name" '.[$n].resolved | length' <<<"$m")" -gt 0 ] || log "snapshot: $name: no npm/cargo package"
    log "snapshot: $name: kept $(jq --arg n "$name" '[.[$n].resolved[] | .package_url | capture("^pkg:(?<t>[a-z]+)/").t] | group_by(.) | map("\(length) pkg:\(.[0])") | join(", ")' -r <<<"$m")"
    i=$((i + 1)); printf '%s\n' "$m" >"$tmp/$i.json"
  done
  jq -s 'add' "$tmp"/*.json >"$tmp/manifests"
  jq -n --arg sha "$sha" --arg ref "$ref" --arg corr "$corr" --arg scanned "$scanned" --slurpfile ms "$tmp/manifests" '$ms[0] as $man | {
    version: 0, sha: $sha, ref: $ref,
    job: {correlator: $corr, id: $corr},
    detector: {name: "opensidian-sbom-scan", version: "1", url: "https://github.com/jpzk/opensidian/blob/main/scripts/supply-chain/sbom-scan.sh"},
    scanned: $scanned, manifests: $man}'
}

cmd_submit() {
  local f=${1:?submit <snapshot.json>} resp code body
  [ -n "${GH_TOKEN:-}" ] || die "GH_TOKEN not set"
  jq -e '.version == 0 and (.manifests | length) > 0' "$f" >/dev/null || die "submit: $f is not a snapshot"
  # response body on stdout, HTTP status appended as the last line (not a download: nothing is saved)
  resp=$(curl -sS --proto '=https' --tlsv1.2 -w '\n%{http_code}' -X POST \
    -H "Authorization: Bearer $GH_TOKEN" -H 'Accept: application/vnd.github+json' -H 'X-GitHub-Api-Version: 2022-11-28' \
    --data-binary @"$f" "$api/repos/$(repo)/dependency-graph/snapshots") || die "submit: request failed"
  code=${resp##*$'\n'}; body=${resp%$'\n'*}
  [ "$code" = 201 ] || die "submit: API answered HTTP $code: ${body:0:600}"
  echo "submit: HTTP 201 $(jq -c '{id, result, message, created_at}' <<<"$body")"
}

sub=${1:-}; shift || true
case "$sub" in
  fetch|verify|platform|scan|dbrev|enrich|merge|summary|snapshot|submit) "cmd_$sub" "$@" ;;
  *) log "usage: sbom-scan.sh fetch|verify|platform|scan|dbrev|enrich|merge|summary|snapshot|submit ..."; exit 2 ;;
esac
