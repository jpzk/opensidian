#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# sbom-local.sh — build the opensidian SBOM: opensidian-<ver>[-<arch>].cdx.json + .sha256.
# RELEASE SBOM (operator 2026-11-19 12:47Z, "compute it in the build environment on a github
# workflow"): release.yml job appimage runs `sbom-local.sh --release` INSIDE the pinned
# packaging/ci/appimage-build.sh container right after the build (SBOM_TAG), for x86_64 and
# aarch64, and attaches the result to the draft release (docs/RELEASING.md "SBOM").
# Locally the same script still builds a (non-release) SBOM on any box: rust/vendor/flatpak
# directly, the WebKit part via --webkit podman (same image + pins) or --skip webkit (recorded).
# bash + jq + coreutils (+ cargo for licenses/scope, + podman for the WebKit part off Ubuntu).
#
# Parts (the same four the former CI `sbom` job merged), each checked to be CycloneDX 1.6:
#   rust     rust-sbom.sh src-tauri/Cargo.lock                    (offline)
#   vendor   vendor-sbom.sh (runs vendor-verify.sh first)        (network: registry.npmjs.org)
#   flatpak  flatpak-sbom.sh packaging/flatpak/dev.koto.opensidian.yml (offline; the manifest must exist)
#   webkit   webkit-bundle-sbom.sh [--bin ELF]: the WebKitGTK closure of the PORTABLE AppImage, read
#            from an rpm database that holds EXACTLY this arch's rows of packaging/ci/fedora-pkgset.lock.tsv
#            (fedport 2026-11-28: the build env is Fedora 44, packaging/ci/fedora-env.sh):
#              --webkit native  this machine = the appimage-build.sh container (release.yml); refused
#                               unless rpm -qa == the lock exactly
#              --webkit podman  fedora-env.sh: the digest-pinned Fedora 44 OCI image + the lock's rpms
#                               (sha256 + rpm -K verified, rpm -U, no repo) (any Linux host with podman)
#              --webkit auto    (default) native if this is that env, else podman
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
# RELEASE MODE (--release; release.yml runs it INSIDE packaging/ci/appimage-build.sh's build
# container right after the build, so the SBOM describes the environment the AppImage came from):
#   refuses any --skip, needs -t <tag> (root version + purl pkg:github/jpzk/opensidian@<tag>),
#   --bin (the built ELF), --pkgset (appimage-build.sh pkgset tsv: .deb SHA-256), --arch, cargo
#   (rust part from `cargo metadata`: licenses G2 + target-resolve scope G11) and a clean tree;
#   then sbom-validate.sh --release (structure; skipped must be "") and license-policy.sh (G7)
#   must pass, else nothing is written.
#   --arch <a>     output opensidian-<ver>-<a>.cdx.json (+ .sha256); rust scope for <a>-unknown-linux-gnu
#   -t <tag>       passed to sbom-merge.sh (must agree with -v)
#   --pkgset <tsv> passed to webkit-bundle-sbom.sh
#   SBOM_COMMIT + SBOM_DIRTY (+ SOURCE_DATE_EPOCH): commit/dirty without git (the build container has none)
#   CARGO (default cargo); without cargo (local only) the rust part has no licenses/scope.
#   Also writes <out>/parts/<part>.cdx.json (the scan inputs) and logs/validate.log, license-policy.log.
# Exit 0 = SBOM written; 1 = a part / the merge failed; 2 = usage.
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
say() { echo "[$(date -u +%H:%M:%S)] [sbom-local] $*" >&2; }
die() { say "FAIL: $*"; exit 1; }
usage() { echo "usage: sbom-local.sh [--release] [-o dir] [-v ver] [-t tag] [--arch x86_64|aarch64] [--pkgset tsv] [--skip rust|vendor|flatpak|webkit]... [--bin elf] [--webkit auto|native|podman]" >&2; exit 2; }

ALL="rust vendor flatpak webkit"
out=$root/dist/sbom ver= bin= wk=auto skip=" " tag= pkgset= arch= release=false
while [ $# -gt 0 ]; do
  case $1 in
    -o) [ $# -ge 2 ] || usage; out=$2; shift 2 ;;
    -v) [ $# -ge 2 ] || usage; ver=$2; shift 2 ;;
    -t) [ $# -ge 2 ] || usage; tag=$2; shift 2 ;;
    --bin) [ $# -ge 2 ] || usage; bin=$(readlink -f "$2"); shift 2 ;;
    --pkgset) [ $# -ge 2 ] || usage; pkgset=$(readlink -f "$2"); shift 2 ;;
    --arch) [ $# -ge 2 ] || usage; arch=$2; shift 2 ;;
    --release) release=true; shift ;;
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
[ -z "$pkgset" ] || [ -f "$pkgset" ] || die "--pkgset $pkgset is not a file"
case $arch in ""|x86_64|aarch64) ;; *) die "--arch $arch: x86_64 or aarch64" ;; esac
CARGO=${CARGO:-cargo}
if [ "$release" = true ]; then
  [ "$skip" = " " ] || die "--release: no part may be skipped (got --skip${skip% })"
  [ -n "$tag" ] || die "--release needs -t <tag>"
  [ -n "$bin" ] || die "--release needs --bin <the release opensidian ELF>"
  [ -n "$pkgset" ] || die "--release needs --pkgset <appimage-build.sh pkgset-<ver>-<arch>.tsv>"
  [ -n "$arch" ] || die "--release needs --arch"
  command -v "$CARGO" >/dev/null || die "--release needs cargo (licenses + target resolve from cargo metadata)"
fi
for c in jq sha256sum awk; do command -v "$c" >/dev/null || die "missing $c"; done
cd "$root"
[ -n "$ver" ] || { ver=$(jq -r .version src-tauri/tauri.conf.json); ver=${ver%.0}; }
# SBOM_COMMIT / SBOM_DIRTY: the build container has no git; the workflow passes what the
# checkout is (github.sha, and the tree is the clean checkout: dirty=false). Otherwise from git.
if [ -n "${SBOM_COMMIT:-}" ]; then
  sha=$SBOM_COMMIT; dirty=${SBOM_DIRTY:?SBOM_DIRTY must be set with SBOM_COMMIT (true|false)}
  case $dirty in true|false) ;; *) die "SBOM_DIRTY=$dirty: true|false" ;; esac
  printf '%s' "$sha" | grep -Eq '^[0-9a-f]{40}$' || die "SBOM_COMMIT=$sha is not a 40-hex commit"
  export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:?SOURCE_DATE_EPOCH must be set with SBOM_COMMIT}
else
  command -v git >/dev/null || die "missing git (or set SBOM_COMMIT + SBOM_DIRTY + SOURCE_DATE_EPOCH)"
  export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
  sha=$(git rev-parse HEAD)
  dirty=false; [ -z "$(git status --porcelain --untracked-files=no)" ] || dirty=true
fi
[ "$release" = false ] || [ "$dirty" = false ] || die "--release: the tree is dirty"
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

# ---- WebKit part: the pinned Fedora 44 set of packaging/ci/fedora-env.sh (fedport) ----
# The rpm database must hold EXACTLY this arch's rows of packaging/ci/fedora-pkgset.lock.tsv
# (what appimage-build.sh's container installs + verifies), so the SBOM cannot drift from what
# the portable AppImage bundles.
LOCK=$root/packaging/ci/fedora-pkgset.lock.tsv
lock_eq_rpmdb() {  # rc 0 iff rpm -qa (minus gpg-pubkey) == the lock rows of this arch
  local a; a=$(uname -m)
  [ -s "$LOCK" ] || { echo "sbom-local: $LOCK missing or empty" >&2; return 1; }
  diff <(awk -F'\t' -v a="$a" '!/^#/ && $1==a {print $2"\t"$3"\t"$4}' "$LOCK" | LC_ALL=C sort) \
       <(rpm -qa --qf '%{NAME}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}\t%{ARCH}\n' | grep -v '^gpg-pubkey	' | LC_ALL=C sort) >&2
}
webkit_native() {
  [ -f /etc/fedora-release ] && command -v rpm >/dev/null || die "native: not the Fedora 44 build env (use --webkit podman)"
  [ -s "$LOCK" ] || die "no $LOCK"
  lock_eq_rpmdb || die "native: installed rpm set != $LOCK ($(uname -m)) - the SBOM must describe what the AppImage bundles"
  bash "$here/webkit-bundle-sbom.sh" ${bin:+--bin "$bin"} ${pkgset:+--pkgset "$pkgset"}
}
webkit_podman() {
  command -v podman >/dev/null || die "podman: not installed (or run inside the Fedora 44 build env, or --skip webkit)"
  local img rpms
  img=$(bash "$root/packaging/ci/fedora-env.sh" image) || die "fedora-env.sh image"
  rpms=${FEDORA_RPM_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/opensidian-appimage/rpms-$(uname -m)}
  bash "$root/packaging/ci/fedora-env.sh" fetch "$rpms" >&2 || die "fedora-env.sh fetch"
  say "webkit via podman: $img + the pinned lock ($(sha256sum "$LOCK" | cut -c1-16))"
  podman run --rm --network=none -e HASBIN="${bin:+1}" -v "$rpms":/rpms:ro -v "$root":/src:ro \
    ${bin:+-v "$bin":/opensidian-bin:ro} "$img" bash -euo pipefail -c '
    bash /src/packaging/ci/fedora-env.sh install /rpms /tmp/pkgset.tsv >&2
    cd /src && bash scripts/supply-chain/webkit-bundle-sbom.sh ${HASBIN:+--bin /opensidian-bin} --pkgset /tmp/pkgset.tsv'
}
webkit_part() {
  local m=$wk
  if [ "$m" = auto ]; then
    if [ -f /etc/fedora-release ] && rpm -q webkit2gtk4.1 >/dev/null 2>&1 && lock_eq_rpmdb 2>/dev/null; then m=native; else m=podman; fi
  fi
  say "webkit mode: $m"
  "webkit_$m"
}

# ---- rust part: Cargo.lock + licenses (G2) + target resolve scope (G11) from cargo metadata ----
rust_part() {
  local tgt=${arch:-$(uname -m)}-unknown-linux-gnu
  if command -v "$CARGO" >/dev/null; then
    say "rust: cargo metadata (--all-features: licenses; --filter-platform $tgt: shipped scope)"
    "$CARGO" metadata --locked --all-features --format-version 1 --manifest-path src-tauri/Cargo.toml >"$t/cargo-meta.json" \
      || { echo "cargo metadata --all-features failed" >&2; return 1; }
    "$CARGO" metadata --locked --filter-platform "$tgt" --format-version 1 --manifest-path src-tauri/Cargo.toml >"$t/cargo-resolve.json" \
      || { echo "cargo metadata --filter-platform $tgt failed" >&2; return 1; }
    bash "$here/rust-sbom.sh" --metadata "$t/cargo-meta.json" --resolve "$t/cargo-resolve.json" --target "$tgt" src-tauri/Cargo.lock
  else
    say "WARNING: no cargo: rust part WITHOUT licenses and scope (never for a release; --release refuses)"
    bash "$here/rust-sbom.sh" src-tauri/Cargo.lock
  fi
}

parts=()
for p in $ALL; do
  case $skip in *" $p "*) say "part $p: SKIPPED (--skip $p, recorded in the SBOM)"; continue ;; esac
  case $p in
    rust)    run_part rust rust_part ;;
    vendor)  run_part vendor bash "$here/vendor-sbom.sh" ;;
    flatpak) m=packaging/flatpak/dev.koto.opensidian.yml
             [ -f "$m" ] || die "part flatpak: input $m missing (pass --skip flatpak to leave it out, recorded)"
             run_part flatpak bash "$here/flatpak-sbom.sh" "$m" ;;
    webkit)  run_part webkit webkit_part ;;
  esac
  parts+=("$t/$p.cdx.json")
done
[ ${#parts[@]} -gt 0 ] || die "every part skipped: nothing to merge"

bash "$here/sbom-merge.sh" -o "$t/merged" -v "$ver" ${tag:+-t "$tag"} "${parts[@]}" >"$t/merged.path" 2>"$out/logs/merge.log" \
  || { cat "$out/logs/merge.log" >&2; die "sbom-merge.sh failed"; }
m=$(cat "$t/merged.path"); name=$(basename "$m")
[ -z "$arch" ] || name=${name%.cdx.json}-$arch.cdx.json   # per-arch release asset: opensidian-<ver>-<arch>.cdx.json
inc=$(for p in $ALL; do case $skip in *" $p "*) ;; *) printf '%s ' "$p" ;; esac; done)
sk=$(for p in $ALL; do case $skip in *" $p "*) printf "%s " "$p" ;; esac; done); sk=${sk% }   # canonical part order
# skipped is "" when every part is in (release builds require that), else the skipped parts
jq --arg sha "$sha" --arg dirty "$dirty" --arg inc "${inc% }" --arg sk "$sk" --arg arch "$arch" --arg rel "$release" '
  .metadata.tools.components += [{type: "application", name: "scripts/supply-chain/sbom-local.sh"}]
  | .metadata.properties = ((.metadata.properties // []) + [
      {name: "opensidian:sbom-local:commit", value: $sha},
      {name: "opensidian:sbom-local:dirty", value: $dirty},
      {name: "opensidian:sbom-local:parts", value: $inc},
      {name: "opensidian:sbom-local:skipped", value: $sk},
      {name: "opensidian:sbom-local:release", value: $rel} ]
      + (if $arch == "" then [] else [{name: "opensidian:sbom-local:arch", value: $arch}] end))' "$m" >"$t/final"
bash "$here/sbom-validate.sh" $([ "$release" = true ] && echo --release) "$t/final" 2>"$out/logs/validate.log" \
  || { cat "$out/logs/validate.log" >&2; die "structural validation failed (log $out/logs/validate.log)"; }
cat "$out/logs/validate.log" >&2
if [ "$release" = true ]; then
  # G7: the license policy gate is part of producing a release SBOM: a denied license = no SBOM.
  bash "$here/license-policy.sh" "$t/final" >"$out/logs/license-policy.log" 2>&1 \
    || { cat "$out/logs/license-policy.log" >&2; die "license policy gate FAILED (log $out/logs/license-policy.log)"; }
  tail -3 "$out/logs/license-policy.log" >&2
fi
mv "$t/final" "$out/$name"
# the parts as generated (scan inputs: release.yml gates the WebKit part with osv-fixed-gate.sh)
mkdir -p "$out/parts"; for p in $ALL; do if [ -f "$t/$p.cdx.json" ]; then cp "$t/$p.cdx.json" "$out/parts/$p.cdx.json"; fi; done
(cd "$out" && sha256sum "$name" > "$name.sha256" && sha256sum -c --quiet "$name.sha256")
say "components: $(jq '.components | length' "$out/$name") (parts: ${inc% }; skipped: ${sk:-none}); without licenses: $(jq '[.components[] | select((.licenses // []) | length == 0)] | length' "$out/$name")"
for p in $ALL; do if [ -f "$t/$p.cdx.json" ]; then say "  $p: $(jq '.components | length' "$t/$p.cdx.json")"; fi; done
[ -z "$sk" ] || say "WARNING: SKIPPED part(s) $sk — this SBOM is INCOMPLETE; say so in the release notes"
say "wrote $out/$name"
say "sha256 $(cut -d' ' -f1 "$out/$name.sha256")  ($out/$name.sha256)"
echo "$out/$name"
