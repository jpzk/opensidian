#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# sbom-crosscheck.sh — #299: scan the SAME CycloneDX SBOMs with two independent scanners
# (pinned osv-scanner + pinned grype) and diff their findings, so a miss by one shows up.
#
#   fetch <tag> <dir>                download every <name>.cdx.json of release <tag> listed in
#                                    $SBOMS (space-separated basenames) + its .sha256; verify each
#                                    (missing / mismatch -> rc 1)
#   fetch-android <dir>              android SBOMs from the latest successful SBOM scan run (artifact
#                                    digest + .sha256 verified); prints the run id
#   fetch-run <run> <sha> <dir>      SBOM artifacts of one release.yml run built from <sha> (digest +
#                                    .sha256 verified, both linux arches required); prints the run id
#   norm-osv <osv.json> <sbom-label>   osv-scanner JSON  -> normalized TSV on stdout
#   norm-grype <grype.json> <sbom-label> grype JSON      -> normalized TSV on stdout
#   diff <osv.tsv> <grype.tsv>       one line per finding: both|osv-only|grype-only <TAB> row
#   summary <diff.tsv>               markdown counts per sbom
#
# Normalized TSV: sbom  ecosystem  name  version  id  ids  severity  fixed
#   ids = sorted, comma-joined union of the finding id, its aliases / related ids, its upstream
#   ids, and the CVE inside DEBIAN-CVE-* / UBUNTU-CVE-* ids. Two findings MATCH when they are on the
#   same sbom + lowercased package name + version and share at least one id. Ecosystem labels differ
#   per scanner (crates.io vs rust-crate) and are display-only.
# Fail closed: unreadable / wrong-shaped scanner JSON -> rc 1, never "0 findings".
set -euo pipefail
log() { printf '%s\n' "$*" >&2; }
die() { log "::error::$*"; exit 1; }

cmd_fetch() {
  local tag=${1:?fetch <tag> <dir>} dir=${2:?} repo=${GITHUB_REPOSITORY:-jpzk/opensidian} f
  case "$tag" in v[0-9]*.[0-9]*.[0-9]*) ;; *) die "fetch: bad tag '$tag'" ;; esac
  [ -n "${SBOMS:-}" ] || die "fetch: SBOMS is empty"
  mkdir -p "$dir"
  for f in $SBOMS; do
    case "$f" in *[!A-Za-z0-9._-]*|'') die "fetch: bad asset name '$f'" ;; *.cdx.json) ;; *) die "fetch: '$f' is not *.cdx.json" ;; esac
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$dir/$f.sha256" "https://github.com/$repo/releases/download/$tag/$f.sha256" || die "fetch: $f.sha256 missing"
    curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$dir/$f" "https://github.com/$repo/releases/download/$tag/$f" || die "fetch: $f missing"
    [ "$(awk '{print $2}' "$dir/$f.sha256")" = "$f" ] || die "fetch: $f.sha256 names another file"
    (cd "$dir" && sha256sum -c "$f.sha256" >&2) || die "fetch: sha256 mismatch for $f"
    jq -e '.bomFormat == "CycloneDX" and (.components | type == "array")' "$dir/$f" >/dev/null || die "fetch: $f is not a CycloneDX SBOM"
  done
}


# fetch-android <dir>: the android SBOMs are not release assets (no APK released yet); sbom-scan.yml
# builds them from the android branch and keeps them as artifact sbom-scan-android. Take that artifact
# from the latest SUCCESSFUL SBOM scan run, verify the zip against the artifact's sha256 digest and
# every SBOM against its .sha256 file. Prints the run id on stdout.
cmd_fetch_android() {
  local dir=${1:?fetch-android <dir>} repo=${GITHUB_REPOSITORY:-jpzk/opensidian} run id dg f n=0
  run=$(gh run list --repo "$repo" --workflow sbom-scan.yml --status success -L 1 --json databaseId --jq '.[0].databaseId')
  [[ "$run" =~ ^[0-9]+$ ]] || die "fetch-android: no successful SBOM scan run"
  id=$(gh api "repos/$repo/actions/runs/$run/artifacts" --jq '[.artifacts[] | select(.name == "sbom-scan-android" and (.expired | not))][0].id')
  dg=$(gh api "repos/$repo/actions/runs/$run/artifacts" --jq '[.artifacts[] | select(.name == "sbom-scan-android" and (.expired | not))][0].digest')
  [[ "$id" =~ ^[0-9]+$ ]] || die "fetch-android: run $run has no sbom-scan-android artifact"
  [[ "$dg" =~ ^sha256:[0-9a-f]{64}$ ]] || die "fetch-android: artifact $id has no sha256 digest"
  mkdir -p "$dir"
  gh api "repos/$repo/actions/artifacts/$id/zip" >"$dir/a.zip" || die "fetch-android: download of artifact $id failed"
  echo "${dg#sha256:}  $dir/a.zip" | sha256sum -c - >&2 || die "fetch-android: artifact zip != digest $dg"
  unzip -q -o -j "$dir/a.zip" '*.cdx.json' '*.cdx.json.sha256' -d "$dir" || die "fetch-android: unzip failed"
  rm -f "$dir/a.zip"
  for f in "$dir"/*.cdx.json; do
    [ -f "$f.sha256" ] || die "fetch-android: $(basename "$f") has no .sha256"
    (cd "$dir" && sha256sum -c "$(basename "$f").sha256" >&2) || die "fetch-android: sha256 mismatch for $f"
    n=$((n + 1))
  done
  [ "$n" -gt 0 ] || die "fetch-android: artifact has no SBOM"
  printf '%s\n' "$run"
}

# fetch-run <run-id> <sha> <dir>: the CI-built SBOMs of ONE Release run (#299: scan the PR head, not only
# the last published release). The run must be a release.yml run whose head_sha == <sha> (the commit
# under test); takes every non-expired artifact named sbom-<ver>-<arch> / sbom-macos-<arch>, verifies
# each zip against the artifact's sha256 digest and every SBOM against its .sha256 file. Both linux
# arches (x86_64, aarch64: the SBOMs with the nested WebKit components) are required. Prints the run id.
cmd_fetch_run() {
  local run=${1:?fetch-run <run-id> <sha> <dir>} sha=${2:?} dir=${3:?} repo=${GITHUB_REPOSITORY:-jpzk/opensidian}
  local meta wf hs a id dg name n=0 f
  [[ "$run" =~ ^[0-9]+$ ]] || die "fetch-run: bad run id '$run'"
  [[ "$sha" =~ ^[0-9a-f]{40}$ ]] || die "fetch-run: bad sha '$sha'"
  meta=$(gh api "repos/$repo/actions/runs/$run" --jq '[.path, .head_sha] | join(" ")') || die "fetch-run: run $run not found"
  read -r wf hs <<<"$meta"
  [ "$wf" = ".github/workflows/release.yml" ] || die "fetch-run: run $run is '$wf', not release.yml"
  [ "$hs" = "$sha" ] || die "fetch-run: run $run built $hs, not $sha"
  mkdir -p "$dir"
  while IFS=$'\t' read -r id dg name || [ -n "$id" ]; do
    [[ "$id" =~ ^[0-9]+$ ]] || die "fetch-run: bad artifact id '$id'"
    [[ "$dg" =~ ^sha256:[0-9a-f]{64}$ ]] || die "fetch-run: artifact $name has no sha256 digest"
    gh api "repos/$repo/actions/artifacts/$id/zip" >"$dir/a.zip" || die "fetch-run: download of $name failed"
    echo "${dg#sha256:}  $dir/a.zip" | sha256sum -c - >&2 || die "fetch-run: $name zip != digest $dg"
    unzip -q -o -j "$dir/a.zip" '*.cdx.json' '*.cdx.json.sha256' -d "$dir" || die "fetch-run: unzip of $name failed"
    rm -f "$dir/a.zip"
  done < <(gh api "repos/$repo/actions/runs/$run/artifacts?per_page=100" --jq '.artifacts[]
             | select((.expired | not) and (.name | test("^sbom-([0-9][0-9.]*-(x86_64|aarch64)|macos-[a-z0-9_]+)$")))
             | [.id, .digest, .name] | @tsv' || die "fetch-run: cannot list artifacts of run $run")
  for f in "$dir"/*.cdx.json; do
    [ -f "$f" ] || continue
    [ -f "$f.sha256" ] || die "fetch-run: $(basename "$f") has no .sha256"
    (cd "$dir" && sha256sum -c "$(basename "$f").sha256" >&2) || die "fetch-run: sha256 mismatch for $f"
    jq -e '.bomFormat == "CycloneDX" and (.components | type == "array")' "$f" >/dev/null || die "fetch-run: $f is not a CycloneDX SBOM"
    n=$((n + 1))
  done
  for a in x86_64 aarch64; do
    ls "$dir"/*[0-9]-"$a".cdx.json >/dev/null 2>&1 || die "fetch-run: run $run has no linux $a SBOM"
  done
  log "fetch-run: $n SBOM(s) from release run $run @ $sha"
  printf '%s\n' "$run"
}
# shared jq: id set normalization
JQ_IDS='def ids($xs): [$xs[] | select(type == "string" and . != "")
          | ., (capture("^(?:DEBIAN|UBUNTU)-(?<c>CVE-[0-9]+-[0-9]+)$")?.c // empty)] | unique | join(",");
        def orn: if . == null or . == "" then "-" else . end;'

cmd_norm_osv() {
  local j=${1:?norm-osv <osv.json> <label>} l=${2:?}
  jq -e '.results | type == "array"' "$j" >/dev/null 2>&1 || die "norm-osv: $j is not osv-scanner JSON"
  jq -r --arg l "$l" "$JQ_IDS"'
    .results[].packages[]? | .package as $p | (.vulnerabilities // [])[]
    | [$l, ($p.ecosystem | orn),
       # Maven: osv-scanner says group:artifact, grype says artifactId -> key on artifactId
       ($p.name | if $p.ecosystem == "Maven" then (split(":") | last) else . end | ascii_downcase),
       ($p.version | orn), .id,
       ids([.id] + (.aliases // []) + (.upstream // [])),
       ((.database_specific.severity // .severity[0]?.score // null) | if . == null then null else tostring end | orn),
       ([.affected[]?.ranges[]?.events[]? | .fixed // empty] | unique | join(",") | orn)]
    | @tsv' "$j" | sort -u
}

cmd_norm_grype() {
  local j=${1:?norm-grype <grype.json> <label>} l=${2:?}
  jq -e '.matches | type == "array"' "$j" >/dev/null 2>&1 || die "norm-grype: $j is not grype JSON"
  jq -r --arg l "$l" "$JQ_IDS"'
    .matches[] | [$l, (.artifact.type | orn), (.artifact.name | ascii_downcase), (.artifact.version | orn),
       .vulnerability.id, ids([.vulnerability.id] + [.relatedVulnerabilities[]?.id]),
       (.vulnerability.severity | orn), ((.vulnerability.fix.versions // []) | join(",") | orn)]
    | @tsv' "$j" | sort -u
}

cmd_diff() {
  local a=${1:?diff <osv.tsv> <grype.tsv>} b=${2:?}
  [ -r "$a" ] && [ -r "$b" ] || die "diff: unreadable input"
  awk -F'\t' -v OFS='\t' '
    NF != 8 { bad = 1; exit }
    { key = $1 SUBSEP $3 SUBSEP $4; n = split($6, ids, ",")
      row[side, FNR] = $0; cnt[side] = FNR
      for (i = 1; i <= n; i++) idx[side, key, ids[i]] = 1
      k[side, FNR] = key; il[side, FNR] = $6 }
    END {
      if (bad) { print "diff: malformed row (need 8 tab-separated fields)" > "/dev/stderr"; exit 1 }
      for (s = 1; s <= 2; s++) { o = 3 - s
        for (r = 1; r <= cnt[s]; r++) { m = 0; n = split(il[s, r], ids, ",")
          for (i = 1; i <= n; i++) if ((o, k[s, r], ids[i]) in idx) { m = 1; break }
          if (m && s == 1) print "both", row[s, r]
          else if (!m) print (s == 1 ? "osv-only" : "grype-only"), row[s, r] } } }' side=1 "$a" side=2 "$b"
}

cmd_summary() {
  local d=${1:?summary <diff.tsv>}
  printf '| sbom | both | osv-only | grype-only |\n|---|---|---|---|\n'
  awk -F'\t' '{ s[$2]; c[$2, $1]++ } END { for (x in s) printf "| %s | %d | %d | %d |\n", x, c[x,"both"], c[x,"osv-only"], c[x,"grype-only"] }' "$d" | sort
}

c=${1:-}; shift || true
case "$c" in
  fetch) cmd_fetch "$@" ;;
  fetch-android) cmd_fetch_android "$@" ;;
  fetch-run) cmd_fetch_run "$@" ;;
  norm-osv) cmd_norm_osv "$@" ;;
  norm-grype) cmd_norm_grype "$@" ;;
  diff) cmd_diff "$@" ;;
  summary) cmd_summary "$@" ;;
  *) die "usage: $0 fetch|fetch-android|fetch-run|norm-osv|norm-grype|diff|summary ..." ;;
esac
