#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# grype-install.sh — install the PINNED grype (grype.pins) for the runner's architecture (#299).
# Same contract as osv-install.sh: version + per-arch sha256 from the committed pin file, no API
# call, no "latest"; unknown arch / malformed pins / missing digest / mismatch -> exit 1 and no
# binary left behind. Prints the installed binary path on stdout; logs go to stderr.
# usage: grype-install.sh [DEST_DIR]   (default: $RUNNER_TEMP/grype or ./.grype)
set -euo pipefail

repo=anchore/grype
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
log() { printf '%s\n' "$*" >&2; }
die() { log "::error::$*"; exit 1; }

case "${1:-}" in -*) die "unknown option '$1'" ;; esac
dest=${1:-${RUNNER_TEMP:+$RUNNER_TEMP/grype}}; dest=${dest:-$PWD/.grype}
pins=${GRYPE_PINS:-$here/grype.pins}

m=$(uname -m)
case "$m" in
  x86_64|amd64)  goarch=amd64 ;;
  aarch64|arm64) goarch=arm64 ;;
  *) die "unsupported machine architecture '$m' (no grype asset mapped)" ;;
esac

[ -r "$pins" ] || die "pin file $pins missing"
bad=$(grep -Ev '^(#.*|version v[0-9][0-9A-Za-z.+-]*|sha256 [a-z0-9]+ [0-9a-f]{64}|)$' "$pins" || true)
[ -z "$bad" ] || die "malformed line(s) in $pins: $bad"
[ "$(grep -c '^version ' "$pins")" = 1 ] || die "$pins must hold exactly one version line"
tag=$(awk '$1 == "version" {print $2}' "$pins")
case "$tag" in v[0-9]*) ;; *) die "unexpected tag '$tag'" ;; esac
n=$(awk -v a="$goarch" '$1 == "sha256" && $2 == a' "$pins" | wc -l)
[ "$n" = 1 ] || die "no (or more than one) pinned sha256 for $goarch in $pins ($n) — refusing to install an unverified grype $tag"
sha=$(awk -v a="$goarch" '$1 == "sha256" && $2 == a {print $3}' "$pins")
ver=${tag#v}
asset=grype_${ver}_linux_${goarch}.tar.gz

mkdir -p "$dest"
tgz="$dest/$asset"; bin="$dest/grype"
rm -f "$bin" "$tgz"
curl -fsSL --proto '=https' --tlsv1.2 --retry 3 -o "$tgz" \
  "https://github.com/$repo/releases/download/${tag}/${asset}" \
  || { rm -f "$tgz"; die "download of $asset failed"; }
echo "${sha}  $tgz" | sha256sum -c - >&2 || { rm -f "$tgz"; die "sha256 mismatch for $asset (expected $sha)"; }
tar -C "$dest" -xzf "$tgz" grype || { rm -f "$tgz" "$bin"; die "extract of $asset failed"; }
rm -f "$tgz"
log "grype release: $tag (pinned)"
log "grype asset:   $asset (uname -m: $m)"
log "grype sha256:  $sha"
"$bin" version >&2 || { rm -f "$bin"; die "grype $tag does not run on this machine ($m)"; }
printf '%s\n' "$bin"
