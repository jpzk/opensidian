#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# sbom-local.sh — build the release SBOM LOCALLY: opensidian-<ver>.cdx.json + .sha256.
# OPERATOR DECISION 2026-11-19 ("lets deactivate now lets only run the sbom locally"): CI
# (supply-chain.yml) no longer produces or attests an SBOM; the release SBOM is built with this
# script on the release build machine and uploaded to the draft release by hand
# (docs/RELEASING.md "SBOM (local)"). bash + jq + coreutils (+ podman for the WebKit part off Ubuntu).
#
# Parts (the same four the former CI `sbom` job merged), each checked to be CycloneDX 1.6:
#   rust     rust-sbom.sh src-tauri/Cargo.lock                    (offline)
#   vendor   vendor-sbom.sh (runs vendor-verify.sh first)        (network: registry.npmjs.org)
#   flatpak  flatpak-sbom.sh packaging/flatpak/dev.koto.opensidian.yml (offline; the manifest must exist)
#   webkit   webkit-bundle-sbom.sh [--bin ELF]: the WebKitGTK closure of the PORTABLE AppImage, read
#            from a dpkg database that holds EXACTLY appimage-build.sh's pinned PKGS at its SNAP:
#              --webkit native  this machine (Ubuntu 26.04 after scripts/supply-chain/webkit-apt.sh);
#                               refused unless every PKGS pin is installed at its pinned version
#              --webkit podman  appimage-build.sh's digest-pinned ubuntu image, apt from the same
#                               snapshot.ubuntu.com SNAP, PKGS + jq pinned (any Linux host with podman)
#              --webkit auto    (default) native if libwebkit2gtk-4.1-0 is dpkg-installed, else podman
#   -> sbom-merge.sh -o <out> -v <ver> <parts>  (dedup + dangling-ref checks, content-derived serial)
#
# FAILS CLOSED: a part that fails or is missing aborts the run (nothing written to <out>). The only
# way to leave a part out is an explicit --skip <part>, which is RECORDED in the merged SBOM
# (metadata.properties opensidian:sbom-local:skipped) and printed; never publish a release SBOM
# with a skipped part without saying so in the release notes.
#
# Usage: scripts/supply-chain/sbom-local.sh [-o <dir>] [-v <ver>] [--skip <part>]... [--bin <elf>]
#                                           [--webkit auto|native|podman]
#   -o   output dir (default dist/sbom); writes opensidian-<ver>.cdx.json, its .sha256 (sha256sum
#        format) and logs/<part>.log; prints the SBOM path on stdout as the last line
#   -v   version (default src-tauri/tauri.conf.json .version minus a trailing ".0", as release.yml)
#   --bin the release opensidian ELF: its own ldd closure joins the WebKit part's roots (podman:
#        mounted read-only)
#   SOURCE_DATE_EPOCH (default: HEAD commit time) fixes every timestamp.
# Exit 0 = SBOM written; 1 = a part / the merge failed; 2 = usage.
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
say() { echo "[$(date -u +%H:%M:%S)] [sbom-local] $*" >&2; }
die() { say "FAIL: $*"; exit 1; }
usage() { echo "usage: sbom-local.sh [-o dir] [-v ver] [--skip rust|vendor|flatpak|webkit]... [--bin elf] [--webkit auto|native|podman]" >&2; exit 2; }

ALL="rust vendor flatpak webkit"
out=$root/dist/sbom ver= bin= wk=auto skip=" "
while [ $# -gt 0 ]; do
  case $1 in
    -o) [ $# -ge 2 ] || usage; out=$2; shift 2 ;;
    -v) [ $# -ge 2 ] || usage; ver=$2; shift 2 ;;
    --bin) [ $# -ge 2 ] || usage; bin=$(readlink -f "$2"); shift 2 ;;
    --webkit) [ $# -ge 2 ] || usage; wk=$2; shift 2 ;;
    --skip) [ $# -ge 2 ] || usage
            case " $ALL " in *" $2 "*) ;; *) echo "unknown part '$2' (parts: $ALL)" >&2; usage ;; esac
            skip="$skip$2 "; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown argument '$1'" >&2; usage ;;
  esac
done
case $wk in auto|native|podman) ;; *) usage ;; esac
[ -z "$bin" ] || [ -f "$bin" ] || die "--bin $bin is not a file"
for c in jq sha256sum git awk; do command -v "$c" >/dev/null || die "missing $c"; done
cd "$root"
[ -n "$ver" ] || { ver=$(jq -r .version src-tauri/tauri.conf.json); ver=${ver%.0}; }
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
sha=$(git rev-parse HEAD)
dirty=false; [ -z "$(git status --porcelain --untracked-files=no)" ] || dirty=true
mkdir -p "$out/logs"; out=$(cd "$out" && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
say "opensidian $ver @ $sha (dirty=$dirty) -> $out; skip:${skip% }"
[ "$dirty" = false ] || say "WARNING: tracked files modified; the SBOM describes HEAD + local edits"

part_ok() {  # part_ok <file>: a CycloneDX 1.6 document with a metadata.component and >= 1 component
  jq -e '.bomFormat == "CycloneDX" and .specVersion == "1.6"
         and (.metadata.component."bom-ref" | type == "string") and ((.components // []) | length > 0)' "$1" >/dev/null 2>&1
}
run_part() {  # run_part <part> <cmd...>: stdout -> $t/<part>.cdx.json, stderr -> logs/<part>.log
  local p=$1; shift
  say "part $p: $*"
  if ! "$@" >"$t/$p.cdx.json" 2>"$out/logs/$p.log"; then tail -20 "$out/logs/$p.log" >&2; die "part $p failed (log $out/logs/$p.log)"; fi
  part_ok "$t/$p.cdx.json" || die "part $p is not a non-empty CycloneDX 1.6 SBOM (log $out/logs/$p.log)"
  say "part $p: $(jq '.components | length' "$t/$p.cdx.json") components"
}

# ---- WebKit part: the pinned set of packaging/ci/appimage-build.sh ----
AB=$root/packaging/ci/appimage-build.sh
ab_var() { sed -n "s/^$1=\"\{0,1\}\([^\"]*\)\"\{0,1\}\$/\1/p" "$AB" | head -1; }
webkit_native() {
  bash "$here/webkit-apt.sh" --check >&2 || die "webkit-apt.sh pins != appimage-build.sh"
  local p n want got
  for p in $(ab_var PKGS); do
    n=${p%%=*}; want=${p#*=}
    got=$(dpkg-query -W -f='${Version}' "$n" 2>/dev/null) || die "native: $n not installed (run scripts/supply-chain/webkit-apt.sh, or --webkit podman)"
    [ "$got" = "$want" ] || die "native: $n is $got, appimage-build.sh pins $want (the SBOM must describe what the AppImage bundles)"
  done
  bash "$here/webkit-bundle-sbom.sh" ${bin:+--bin "$bin"}
}
# Literal pins (pin-check #189 reads them from this file); asserted == appimage-build.sh below,
# like webkit-apt.sh --check, so the SBOM cannot drift from what the AppImage bundles.
WK_IMG=docker.io/library/ubuntu@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7
# = appimage-build.sh PKGS + jq/libjq1 for webkit-bundle-sbom.sh (same snapshot, resolute-security;
# not in the WebKit ldd closure, so they add nothing to the SBOM)
PKGS="build-essential=12.12ubuntu2.26.04.2 pkg-config=2.5.1-4 libssl-dev=3.5.5-1ubuntu3.7 libwebkit2gtk-4.1-dev=2.52.6-0ubuntu0.26.04.1 libwebkit2gtk-4.1-0=2.52.6-0ubuntu0.26.04.1 libjavascriptcoregtk-4.1-0=2.52.6-0ubuntu0.26.04.1 libgtk-3-dev=3.24.52-0ubuntu1 libayatana-appindicator3-dev=0.5.94-1build1 librsvg2-dev=2.61.3+dfsg-3 patchelf=0.18.0-1.4build1 file=1:5.46-5build2 ca-certificates=20260601~26.04.1 curl=8.18.0-1ubuntu2.7 xz-utils=5.8.3-1 jq=1.8.1-4ubuntu2 libjq1=1.8.1-4ubuntu2"
webkit_podman() {
  command -v podman >/dev/null || die "podman: not installed (or use --webkit native on Ubuntu 26.04, or --skip webkit)"
  local snap ca
  snap=$(ab_var SNAP)
  [ -n "$snap" ] || die "cannot read SNAP from $AB"
  [ "$WK_IMG" = "docker.io/library/ubuntu@$(ab_var DIGEST)" ] || die "WK_IMG $WK_IMG != appimage-build.sh DIGEST $(ab_var DIGEST)"
  [ "$PKGS" = "$(ab_var PKGS) jq=1.8.1-4ubuntu2 libjq1=1.8.1-4ubuntu2" ] || die "sbom-local.sh PKGS != appimage-build.sh PKGS + jq libjq1"
  ca=${HOST_CA_BUNDLE:-}
  if [ -z "$ca" ]; then for c in /etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt; do [ -s "$c" ] && { ca=$c; break; }; done; fi
  [ -s "$ca" ] || die "no host CA bundle (set HOST_CA_BUNDLE)"
  say "webkit via podman: $WK_IMG, apt snapshot $snap"
  podman image exists "$WK_IMG" || podman pull -q "$WK_IMG" >&2 || die "podman pull $WK_IMG"
  podman run --rm --network=host -e SNAP="$snap" -e PKGS="$PKGS" -e HASBIN="${bin:+1}" \
    -v "$ca":/host-ca.crt:ro -v "$root":/src:ro ${bin:+-v "$bin":/opensidian-bin:ro} "$WK_IMG" bash -euo pipefail -c '
    rm -f /etc/apt/sources.list /etc/apt/sources.list.d/* /etc/apt/apt.conf.d/docker-clean
    printf "Types: deb\nURIs: https://snapshot.ubuntu.com/ubuntu/%s/\nSuites: resolute resolute-updates resolute-security\nComponents: main universe\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\nCheck-Valid-Until: no\n" "$SNAP" > /etc/apt/sources.list.d/snapshot.sources
    echo "Acquire::https::CAInfo \"/host-ca.crt\";" > /etc/apt/apt.conf.d/99host-ca
    export DEBIAN_FRONTEND=noninteractive
    apt-get -q update > /tmp/apt.log 2>&1 || { cat /tmp/apt.log >&2; exit 1; }
    if grep -E "^(Err:|W:|E:|Error:|Warning:)" /tmp/apt.log >&2; then echo "apt update against the snapshot was not clean" >&2; exit 1; fi
    apt-get -y -q --no-install-recommends install $PKGS >> /tmp/apt.log 2>&1 || { tail -30 /tmp/apt.log >&2; exit 1; }
    for p in $PKGS; do n=${p%%=*}; want=${p#*=}; got=$(dpkg-query -W -f="\${Version}" "$n"); [ "$got" = "$want" ] || { echo "$n is $got, pinned $want" >&2; exit 1; }; done
    echo "pinned set installed: $(echo $PKGS | wc -w) packages" >&2
    cd /src && bash scripts/supply-chain/webkit-bundle-sbom.sh ${HASBIN:+--bin /opensidian-bin}'
}
webkit_part() {
  local m=$wk
  if [ "$m" = auto ]; then
    if command -v dpkg-query >/dev/null && dpkg-query -W libwebkit2gtk-4.1-0 >/dev/null 2>&1; then m=native; else m=podman; fi
  fi
  say "webkit mode: $m"
  "webkit_$m"
}

parts=()
for p in $ALL; do
  case $skip in *" $p "*) say "part $p: SKIPPED (--skip $p, recorded in the SBOM)"; continue ;; esac
  case $p in
    rust)    run_part rust bash "$here/rust-sbom.sh" src-tauri/Cargo.lock ;;
    vendor)  run_part vendor bash "$here/vendor-sbom.sh" ;;
    flatpak) m=packaging/flatpak/dev.koto.opensidian.yml
             [ -f "$m" ] || die "part flatpak: input $m missing (pass --skip flatpak to leave it out, recorded)"
             run_part flatpak bash "$here/flatpak-sbom.sh" "$m" ;;
    webkit)  run_part webkit webkit_part ;;
  esac
  parts+=("$t/$p.cdx.json")
done
[ ${#parts[@]} -gt 0 ] || die "every part skipped: nothing to merge"

bash "$here/sbom-merge.sh" -o "$t/merged" -v "$ver" "${parts[@]}" >"$t/merged.path" 2>"$out/logs/merge.log" \
  || { cat "$out/logs/merge.log" >&2; die "sbom-merge.sh failed"; }
m=$(cat "$t/merged.path"); name=$(basename "$m")
inc=$(for p in $ALL; do case $skip in *" $p "*) ;; *) printf '%s ' "$p" ;; esac; done)
sk=$(for p in $ALL; do case $skip in *" $p "*) printf "%s " "$p" ;; esac; done); sk=${sk% }   # canonical part order
jq --arg sha "$sha" --arg dirty "$dirty" --arg inc "${inc% }" --arg sk "$sk" '
  .metadata.tools.components += [{type: "application", name: "scripts/supply-chain/sbom-local.sh"}]
  | .metadata.properties = ((.metadata.properties // []) + [
      {name: "opensidian:sbom-local:commit", value: $sha},
      {name: "opensidian:sbom-local:dirty", value: $dirty},
      {name: "opensidian:sbom-local:parts", value: $inc},
      {name: "opensidian:sbom-local:skipped", value: (if $sk == "" then "none" else $sk end)} ])' "$m" >"$t/final"
jq -e '.bomFormat == "CycloneDX" and .specVersion == "1.6" and (.components | length > 0)' "$t/final" >/dev/null || die "final SBOM failed self-check"
mv "$t/final" "$out/$name"
(cd "$out" && sha256sum "$name" > "$name.sha256" && sha256sum -c --quiet "$name.sha256")
say "components: $(jq '.components | length' "$out/$name") (parts: ${inc% }; skipped: ${sk:-none})"
for p in $ALL; do if [ -f "$t/$p.cdx.json" ]; then say "  $p: $(jq '.components | length' "$t/$p.cdx.json")"; fi; done
[ -z "$sk" ] || say "WARNING: SKIPPED part(s) $sk — this SBOM is INCOMPLETE; say so in the release notes"
say "wrote $out/$name"
say "sha256 $(cut -d' ' -f1 "$out/$name.sha256")  ($out/$name.sha256)"
echo "$out/$name"
