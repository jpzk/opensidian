#!/usr/bin/env bash
# osv-install.sh — install osv-scanner for the RUNNER's architecture.
#
# Two modes, both fail closed and both pick the asset by `uname -m`
# (x86_64|amd64 -> linux_amd64, aarch64|arm64 -> linux_arm64, anything
# else -> error; no silent amd64 fallback, which broke the arm64 runner):
#
#   --pinned   (release.yml appimage gate, both arches): the version and the
#              per-arch sha256 come from osv-scanner.pins next to this script.
#              No API call. The asset of THAT version for THIS arch is
#              downloaded and checked with sha256sum -c against ITS OWN
#              pinned digest; unknown arch, no pinned digest for the arch,
#              malformed pin file or mismatch -> exit 1, no binary left behind.
#
#   (default)  supply-chain.yml: the LATEST non-prerelease release, UNPINNED on
#              purpose (operator 2026-11-17, issue #189 item 3: "dont pin
#              scanners in workflow but everything else"): ONE GitHub API call
#              to releases/latest (no TOCTOU between tag and digest), refuses
#              prerelease/draft, the arch asset is verified against that
#              asset's API "digest" field (missing/malformed -> fail).
#
# Resolved tag, asset and sha256 are logged. Trust anchor (default mode) =
# GitHub release metadata over TLS; (--pinned) = the digests in the repo.
#
# usage: osv-install.sh [--pinned] [DEST_DIR]  (default: $RUNNER_TEMP or ./.osv)
# prints the installed binary path on stdout; logs go to stderr.
# env: GH_TOKEN / GITHUB_TOKEN (optional, default mode only: API rate limit)
set -euo pipefail

repo=google/osv-scanner
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
log() { printf '%s\n' "$*" >&2; }
die() { log "::error::$*"; exit 1; }

pinned=false
if [ "${1:-}" = --pinned ]; then pinned=true; shift; fi
case "${1:-}" in -*) die "unknown option '$1'" ;; esac
dest=${1:-${RUNNER_TEMP:-$PWD/.osv}}

m=$(uname -m)
case "$m" in
  x86_64|amd64)  goarch=amd64 ;;
  aarch64|arm64) goarch=arm64 ;;
  *) die "unsupported machine architecture '$m' (no osv-scanner asset mapped)" ;;
esac
asset=osv-scanner_linux_$goarch

if $pinned; then
  pins=$here/osv-scanner.pins
  [ -r "$pins" ] || die "pin file $pins missing"
  bad=$(grep -Ev '^(#.*|version v[0-9][0-9A-Za-z.+-]*|sha256 [a-z0-9]+ [0-9a-f]{64}|)$' "$pins" || true)
  [ -z "$bad" ] || die "malformed line(s) in $pins: $bad"
  [ "$(grep -c '^version ' "$pins")" = 1 ] || die "$pins must hold exactly one version line"
  tag=$(awk '$1 == "version" {print $2}' "$pins")
  n=$(awk -v a="$goarch" '$1 == "sha256" && $2 == a' "$pins" | wc -l)
  [ "$n" = 1 ] || die "no (or more than one) pinned sha256 for $goarch in $pins ($n) — refusing to install an unverified $asset $tag"
  sha=$(awk -v a="$goarch" '$1 == "sha256" && $2 == a {print $3}' "$pins")
else
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
fi
grep -Eqx '[0-9a-f]{64}' <<<"$sha" || die "malformed sha256 '$sha' for $asset"
case "$tag" in v[0-9]*) ;; *) die "unexpected tag '$tag'" ;; esac

mkdir -p "$dest"
bin="$dest/osv-scanner"
rm -f "$bin" "$bin.tmp"
curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$bin.tmp" \
  "https://github.com/$repo/releases/download/${tag}/${asset}" \
  || { rm -f "$bin.tmp"; die "download of $asset $tag failed"; }
echo "${sha}  $bin.tmp" | sha256sum -c - >&2 || { rm -f "$bin.tmp"; die "sha256 mismatch for $asset $tag (expected $sha)"; }
chmod +x "$bin.tmp"
log "osv-scanner release: $tag ($($pinned && echo pinned || echo latest))"
log "osv-scanner asset:   $asset (uname -m: $m)"
log "osv-scanner sha256:  $sha"
"$bin.tmp" --version >&2 || { rm -f "$bin.tmp"; die "$asset $tag does not run on this machine ($m)"; }
mv "$bin.tmp" "$bin"
printf '%s\n' "$bin"
