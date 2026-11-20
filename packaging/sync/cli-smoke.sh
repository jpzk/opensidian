#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# cli-smoke.sh — end-to-end smoke of a PACKAGED sync CLI tarball (sync-cli.yml), from a fresh extract:
#   launchers refuse before fipsinstall -> opensidian-sync-fipsinstall -> server on loopback ->
#   vault create -> client A setup --enroll + status + sync-now -> client B joins with --words ->
#   B pulls A's files -> B edits, A pulls the edit.
#   cli-smoke.sh TARBALL [PORT]
# Needs: tar, jq, curl-free (loopback HTTP only). Runs as an unprivileged user (the server refuses root).
set -euo pipefail
die() { echo "[cli-smoke] FAIL: $*" >&2; exit 1; }
say() { echo "[cli-smoke $(date -u +%H:%M:%S)] $*"; }
[ $# -ge 1 ] || die "usage: cli-smoke.sh TARBALL [PORT]"
TB=$(readlink -f -- "$1"); PORT=${2:-18471}
W=$(mktemp -d); SRV=""
trap '[ -n "$SRV" ] && kill "$SRV" 2>/dev/null; rm -rf "$W"' EXIT
tar xzf "$TB" -C "$W"; D=$(echo "$W"/opensidian-sync-*); [ -d "$D" ] || die "no top dir"
say "extracted $(basename "$TB") -> $D"
for f in opensidian-sync-server opensidian-syncd opensidian-sync-fipsinstall libexec/opensidian-sync-server \
         libexec/opensidian-syncd libexec/openssl lib/ossl-modules/fips.so README.md BUILDINFO.txt \
         licenses/LICENSE-GPL-3.0 licenses/LICENSE-OpenSSL-Apache-2.0.txt licenses/NOTICE-EFF-wordlist.txt licenses/CRATES.txt; do
  [ -e "$D/$f" ] || die "missing $f"
done
[ ! -e "$D/ssl/fipsmodule.cnf" ] || die "tarball ships a fipsmodule.cnf (must be generated on the target)"

say "launchers refuse before fipsinstall"
if "$D/opensidian-syncd" status >"$W/pre.out" 2>&1; then die "syncd ran without fipsinstall"; fi
grep -q 'opensidian-sync-fipsinstall' "$W/pre.out" || { cat "$W/pre.out"; die "no fipsinstall hint"; }
say "binary refuses without the FIPS provider (empty module dir)"
mkdir -p "$W/nomod"; : > "$W/nomod/openssl.cnf"
if env OPENSSL_CONF="$W/nomod/openssl.cnf" OPENSSL_MODULES="$W/nomod" "$D/libexec/opensidian-sync-server" vault create --data "$W/x" >"$W/nofips.out" 2>&1; then
  die "server started without the fips provider"; fi
grep -q 'refusing to start' "$W/nofips.out" || { cat "$W/nofips.out"; die "no refusal message"; }

say "opensidian-sync-fipsinstall"
"$D/opensidian-sync-fipsinstall"
[ -f "$D/ssl/fipsmodule.cnf" ] && [ -f "$D/ssl/openssl.cnf" ] || die "fipsinstall wrote no config"
"$D/opensidian-sync-server" --version; "$D/libexec/openssl" version

say "server: vault create + serve on 127.0.0.1:$PORT"
TOK=$("$D/opensidian-sync-server" vault create --data "$W/srv" --ttl-hours 1)
[ -n "$TOK" ] || die "no enrollment token"
"$D/opensidian-sync-server" serve --data "$W/srv" --listen "127.0.0.1:$PORT" > "$W/srv.log" 2>&1 & SRV=$!
for _ in $(seq 1 50); do grep -q listening "$W/srv.log" && break; kill -0 "$SRV" 2>/dev/null || { cat "$W/srv.log"; die "server exited"; }; sleep 0.2; done
grep -q "listening on http://127.0.0.1:$PORT" "$W/srv.log" || { cat "$W/srv.log"; die "server not listening"; }
URL=http://127.0.0.1:$PORT
mkdir -p "$W/a/vault" "$W/a/run" "$W/b/vault" "$W/b/run"; chmod 700 "$W/a/run" "$W/b/run"
A() { XDG_RUNTIME_DIR=$W/a/run "$D/opensidian-syncd" --state "$W/a/state" "$@"; }
B() { XDG_RUNTIME_DIR=$W/b/run "$D/opensidian-syncd" --state "$W/b/state" "$@"; }

say "client A: setup --enroll"
A setup --url "$URL" --vault "$W/a/vault" --name smoke-a --enroll "$TOK" > "$W/a/setup.json" 2> "$W/a/setup.err" || { cat "$W/a/setup.err" "$W/a/setup.json"; die "A setup"; }
jq -e '.server_ok == true and .cfg_gen == 0' "$W/a/setup.json" >/dev/null || { cat "$W/a/setup.json"; die "A setup status"; }
VID=$(jq -r .vault_id "$W/a/setup.json")
WORDS=$(sed -n 's/^  \([a-z][a-z -]*\)$/\1/p' "$W/a/setup.err" | head -1)
[ "$(echo "$WORDS" | wc -w)" -eq 12 ] || die "A setup did not print 12 backup words"
say "client A: status + sync-now"
A status > "$W/a/status.json"; jq -e --arg v "$VID" '.server_ok == true and .vault_id == $v and .device_name == "smoke-a"' "$W/a/status.json" >/dev/null || { cat "$W/a/status.json"; die "A status"; }
printf 'hello from A\n' > "$W/a/vault/note.md"; mkdir -p "$W/a/vault/sub"; head -c 200000 /dev/urandom > "$W/a/vault/sub/blob.bin"
A sync-now > "$W/a/sync1.json"; jq -e '.committed != null and .pushed >= 2' "$W/a/sync1.json" >/dev/null || { cat "$W/a/sync1.json"; die "A sync-now"; }

say "client B: setup --words (stdin)"
printf '%s\n' "$WORDS" | B setup --url "$URL" --vault "$W/b/vault" --name smoke-b --words > "$W/b/setup.json" 2> "$W/b/setup.err" || { cat "$W/b/setup.err" "$W/b/setup.json"; die "B setup --words"; }
jq -e --arg v "$VID" '.vault_id == $v and .active_devices == 2' "$W/b/setup.json" >/dev/null || { cat "$W/b/setup.json"; die "B joined the wrong vault"; }
B sync-now > "$W/b/sync1.json"; jq -e '.written == 2' "$W/b/sync1.json" >/dev/null || { cat "$W/b/sync1.json"; die "B sync-now"; }
cmp "$W/a/vault/note.md" "$W/b/vault/note.md" && cmp "$W/a/vault/sub/blob.bin" "$W/b/vault/sub/blob.bin" || die "B content != A"

say "B edits, A pulls"
printf 'hello from A\nedited on B\n' > "$W/b/vault/note.md"
B sync-now > /dev/null
A sync-now > "$W/a/sync2.json"; jq -e '.written == 1' "$W/a/sync2.json" >/dev/null || { cat "$W/a/sync2.json"; die "A pull"; }
cmp "$W/a/vault/note.md" "$W/b/vault/note.md" || die "A did not get B's edit"
B status | jq -e '.server_ok == true and .device_name == "smoke-b"' >/dev/null || die "B status"
kill "$SRV"; wait "$SRV" 2>/dev/null || true; SRV=""
say "PASS: server + 2 clients (enroll, status, sync-now, --words join, bidirectional sync) from $(basename "$TB")"
