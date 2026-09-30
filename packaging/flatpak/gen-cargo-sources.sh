#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Generate a flatpak-builder `sources` array that vendors every crates.io
# dependency in src-tauri/Cargo.lock, so the build needs NO network.
#
# WHY NOT flatpak-builder-tools/cargo/flatpak-cargo-generator.py:
#   that generator exists to handle git dependencies and patched sources, which
#   it does by CLONING them at generate time. This tree has none (see
#   docs/recon-flatpak/README.md, B1): every package in Cargo.lock is either the
#   local root or `source = "registry+https://github.com/rust-lang/crates.io-index"`
#   with a sha256 in the lock. For that case the sources file is a mechanical
#   transform of Cargo.lock and needs no network, no python, no aiohttp/toml
#   dependency chain, and nothing to pin beyond this file. If a git dependency is
#   ever added, this script REFUSES (rc 3) rather than emitting a silently
#   incomplete manifest — at that point either drop the git dep or take the
#   upstream generator, deliberately.
#
# usage: packaging/flatpak/gen-cargo-sources.sh [BUILDDIR] > cargo-sources.json
#   BUILDDIR: the module's build directory inside the sandbox; CARGO_HOME is
#   BUILDDIR/cargo and the vendor dir BUILDDIR/cargo/vendor. Default
#   /run/build/opensidian (flatpak-builder's dir for a module named "opensidian").
set -e
cd "$(dirname "$0")/../.."
LOCK=src-tauri/Cargo.lock
BUILDDIR=${1:-/run/build/opensidian}
CRATES=https://static.crates.io/crates

[ -f "$LOCK" ] || { echo "no $LOCK" >&2; exit 2; }

# Refuse anything that is not crates.io: a git/path/patched source would need a
# clone we are not doing, and a half-populated vendor dir fails deep inside the
# sandbox with a confusing error instead of here with a clear one.
bad=$(grep '^source = ' "$LOCK" | grep -v '^source = "registry+https://github.com/rust-lang/crates.io-index"$' || true)
if [ -n "$bad" ]; then
  echo "REFUSED: non-crates.io source in $LOCK:" >&2
  echo "$bad" >&2
  echo "see the header of $0 — add git support deliberately, do not paper over it" >&2
  exit 3
fi

awk -v crates="$CRATES" -v builddir="$BUILDDIR" '
function flush() {
  if (name != "" && cksum != "") {
    printf "%s", sep; sep = ",\n"
    printf "  {\n"
    printf "    \"type\": \"archive\",\n"
    printf "    \"archive-type\": \"tar-gzip\",\n"
    printf "    \"url\": \"%s/%s/%s-%s.crate\",\n", crates, name, name, ver
    printf "    \"sha256\": \"%s\",\n", cksum
    printf "    \"dest\": \"cargo/vendor/%s-%s\"\n", name, ver
    printf "  },\n"
    # cargo refuses a vendored crate without its checksum file; "files": {} is
    # the documented way to say "do not re-verify the unpacked tree", the
    # package hash is what cargo checks.
    printf "  {\n"
    printf "    \"type\": \"inline\",\n"
    printf "    \"contents\": \"{\\\"package\\\": \\\"%s\\\", \\\"files\\\": {}}\",\n", cksum
    printf "    \"dest\": \"cargo/vendor/%s-%s\",\n", name, ver
    printf "    \"dest-filename\": \".cargo-checksum.json\"\n"
    printf "  }"
    n++
  }
  name = ""; ver = ""; cksum = ""
}
BEGIN { print "["; sep = "" }
/^\[\[package\]\]/ { flush(); next }
/^name = / { gsub(/^name = "|"$/, ""); name = $0; next }
/^version = / { gsub(/^version = "|"$/, ""); ver = $0; next }
/^checksum = / { gsub(/^checksum = "|"$/, ""); cksum = $0; next }
END {
  flush()
  printf "%s", sep
  printf "  {\n"
  printf "    \"type\": \"inline\",\n"
  printf "    \"contents\": \"[source.crates-io]\\nreplace-with = \\\"vendored-sources\\\"\\n\\n[source.vendored-sources]\\ndirectory = \\\"%s/cargo/vendor\\\"\\n\",\n", builddir
  printf "    \"dest\": \"cargo\",\n"
  printf "    \"dest-filename\": \"config.toml\"\n"
  printf "  }\n"
  print "]"
  printf "gen-cargo-sources: %d crates vendored from %s\n", n, crates > "/dev/stderr"
}
' "$LOCK"
