#!/usr/bin/env bash
# osv-install.sh — install the LATEST non-prerelease osv-scanner release.
#
# UNPINNED on purpose (operator 2026-11-17, issue #189 item 3: "dont pin
# scanners in workflow but everything else"), same exception as
# cargo-audit / cargo-deny / cargo-vet and the opengrep.yml pattern:
#   - ONE GitHub API call to releases/latest (no TOCTOU between tag and
#     digest); refuses prerelease/draft;
#   - the linux_amd64 asset is verified with sha256sum -c against that
#     asset's API "digest" field — fails closed if it is missing/malformed;
#   - the resolved tag + sha256 are logged.
# Trust anchor = GitHub's release metadata over TLS. Blast radius = the
# calling job's permissions (contents: read, no secrets).
#
# usage: osv-install.sh [DEST_DIR]   (default: $RUNNER_TEMP or ./.osv)
# prints the installed binary path on stdout; logs go to stderr.
# env: GH_TOKEN / GITHUB_TOKEN (optional, raises the API rate limit)
set -euo pipefail

repo=google/osv-scanner
asset=osv-scanner_linux_amd64
dest=${1:-${RUNNER_TEMP:-$PWD/.osv}}
mkdir -p "$dest"
log() { printf '%s\n' "$*" >&2; }
die() { log "::error::$*"; exit 1; }

tok=${GH_TOKEN:-${GITHUB_TOKEN:-}}
hdr=(-H 'Accept: application/vnd.github+json' -H 'X-GitHub-Api-Version: 2022-11-28')
[ -z "$tok" ] || hdr+=(-H "Authorization: Bearer $tok")

meta=$(curl -fsSL --proto '=https' --tlsv1.2 --retry 3 "${hdr[@]}" \
  "https://api.github.com/repos/$repo/releases/latest") \
  || die "GitHub API request for $repo releases/latest failed"

tag=$(jq -r '.tag_name // empty' <<<"$meta")
pre=$(jq -r '(.prerelease or .draft) | tostring' <<<"$meta")
digest=$(jq -r --arg a "$asset" '[.assets[] | select(.name == $a)][0].digest // empty' <<<"$meta")
[ -n "$tag" ] || die "no tag_name from $repo releases/latest"
[ "$pre" = false ] || die "$tag is a prerelease/draft"
case "$digest" in
  sha256:*) sha=${digest#sha256:} ;;
  *) die "asset $asset in $tag has no sha256 digest ('$digest')" ;;
esac
grep -Eqx '[0-9a-f]{64}' <<<"$sha" || die "malformed digest '$digest'"
case "$tag" in v[0-9]*) ;; *) die "unexpected tag '$tag'" ;; esac

bin="$dest/osv-scanner"
curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$bin.tmp" \
  "https://github.com/$repo/releases/download/${tag}/${asset}"
echo "${sha}  $bin.tmp" | sha256sum -c - >&2 || { rm -f "$bin.tmp"; die "sha256 mismatch for $asset $tag"; }
chmod +x "$bin.tmp"; mv "$bin.tmp" "$bin"

log "osv-scanner release: $tag"
log "osv-scanner asset:   $asset"
log "osv-scanner sha256:  $sha"
"$bin" --version >&2
printf '%s\n' "$bin"
