#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Print the CONTENT digest of a .flatpak bundle: the ostree "content checksum"
# of the commit inside it, i.e. sha256(root dirtree checksum || root dirmeta
# checksum) as raw 32-byte values. It covers every file's bytes, mode, uid/gid,
# xattrs and the directory layout (ostree stores no mtimes), and nothing in the
# commit object itself (timestamp, parent, subject/body, xa.* metadata) or in
# the bundle header (origin, runtime-repo, appdata, icons, compression).
# Two bundles with the same digest install the same tree.
# See docs/flatpak-reproducibility.md (Q4).
#
#   packaging/flatpak/content-digest.sh opensidian-0.9.2-x86_64.flatpak
#   -> e78208b343e3019a637ec21cd4cfd651cfc2041015b3f908ce025532a94ca265
#
# Needs only `ostree` (OSTREE=/path/to/ostree to override) and sha256sum;
# no root, no flatpak, no network. -v also prints commit, ref, date to stderr.
set -eu
verbose=0
[ "${1:-}" = "-v" ] && { verbose=1; shift; }
[ $# -eq 1 ] && [ -f "$1" ] || { echo "usage: $0 [-v] FILE.flatpak" >&2; exit 2; }
OSTREE=${OSTREE:-ostree}
command -v "$OSTREE" >/dev/null 2>&1 || { echo "$0: ostree not found (set OSTREE=)" >&2; exit 3; }

tmp=$(mktemp -d "${TMPDIR:-/tmp}/content-digest.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
trap 'exit 130' INT TERM
r=$tmp/repo
"$OSTREE" init --repo="$r" --mode=bare-user-only
# A .flatpak IS an ostree static-delta superblock (from-scratch, all parts
# inline) plus extra header keys; ostree applies it natively and checks every
# object against its checksum while writing it.
"$OSTREE" static-delta apply-offline --repo="$r" "$1" >/dev/null

set -- $(find "$r/objects" -name '*.commit')
[ $# -eq 1 ] || { echo "$0: expected exactly 1 commit in bundle, found $#" >&2; exit 4; }
c=$(basename "$(dirname "$1")")$(basename "$1" .commit)

# ostree's own value (ostree_commit_get_content_checksum)
want=$("$OSTREE" show --repo="$r" "$c" | sed -n 's/^ContentChecksum: *//p')

# Independent recomputation from the root dirtree + dirmeta checksums, so the
# printed digest does not rest on one ostree code path alone.
set -- $("$OSTREE" ls --repo="$r" -C -d "$c" /)
tree=$5 meta=$6
hex2bin() { h=$1; while [ -n "$h" ]; do b=${h%"${h#??}"}; h=${h#??}; printf "\\$(printf %03o "0x$b")"; done; }
got=$( { hex2bin "$tree"; hex2bin "$meta"; } | sha256sum | cut -d' ' -f1)
[ -n "$want" ] && [ "$want" = "$got" ] || { echo "$0: content checksum mismatch: ostree=$want recomputed=$got" >&2; exit 5; }

if [ "$verbose" = 1 ]; then
  {
    echo "commit:   $c"
    echo "dirtree:  $tree"
    echo "dirmeta:  $meta"
    "$OSTREE" show --repo="$r" "$c" | sed -n 's/^Date: */date:     /p'
    "$OSTREE" show --repo="$r" --print-metadata-key=xa.ref "$c" | sed 's/^/ref:      /'
  } >&2
fi
echo "$got"
