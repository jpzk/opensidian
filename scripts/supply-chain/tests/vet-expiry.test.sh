#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vet-expiry.sh (#304 review) against generated configs (mktemp, read-only on the checkout) and
# the repository config at fixed dates: a past EXPIRES / "revisit by" is rc=1 naming the crate, the
# day itself and later dates are rc=0, a marker without a date is rc=1, a block without notes is
# ignored; the repository's pulldown-cmark 0.13.6 + tokio 1.53.3 exemptions (EXPIRES 2027-02-23) are
# accepted up to that day and refused the day after.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); g=$here/vet-expiry.sh
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
run() { set +e; VET_EXPIRY_TODAY=$1 bash "$g" "$2" >"$t/out" 2>&1; rc=$?; set -e; }
want() { local name=$1 today=$2 cfg=$3 wrc=$4 pat=${5:-}
  run "$today" "$cfg"
  if [ "$rc" = "$wrc" ] && { [ -z "$pat" ] || grep -qE "$pat" "$t/out"; }; then ok "$name: rc=$rc"; else cat "$t/out"; bad "$name: rc=$rc (want $wrc${pat:+ + /$pat/})"; fi; }
cat > "$t/c.toml" <<'T'
[policy.opensidian]
audit-as-crates-io = false

[[exemptions.alpha]]
version = "1.0.0"
criteria = "safe-to-deploy"
notes = "security fix; EXPIRES 2027-02-23: replace with an imported audit"

[[exemptions.beta]]
version = "2.0.0"
criteria = "safe-to-deploy"
notes = "transitive; temporary, revisit by 2027-01-10"

[[exemptions.gamma]]
version = "3.0.0"
criteria = "safe-to-deploy"
T
want "future dates"        2026-11-23 "$t/c.toml" 0 ', 0 finding\(s\)$'
want "on the expiry day"   2027-01-10 "$t/c.toml" 0 '2 dated exemption expiries'
want "revisit by passed"   2027-01-11 "$t/c.toml" 1 '^VET expired beta 2\.0\.0 .*:12: 2027-01-10 \(today 2027-01-11\)$'
run 2027-02-24 "$t/c.toml"; [ "$rc" = 1 ] && [ "$(grep -c '^VET expired' "$t/out")" = 2 ] && ! grep -q gamma "$t/out" && ok "both expired, notes-less block ignored" || { cat "$t/out"; bad "both expired"; }
sed 's/EXPIRES 2027-02-23/EXPIRES soon/' "$t/c.toml" > "$t/d.toml"
want "marker without a date" 2026-11-23 "$t/d.toml" 1 '^VET baddate alpha 1\.0\.0 '
sed 's/EXPIRES 2027-02-23/EXPIRES 2027-2-3/' "$t/c.toml" > "$t/e.toml"
want "malformed date"        2026-11-23 "$t/e.toml" 1 '^VET baddate alpha '
want "bad today"             yesterday  "$t/c.toml" 2
want "no config"             2026-11-23 "$t/none.toml" 2
# the repository config: the exemptions the review named
repo=$here/../../src-tauri/supply-chain/config.toml
run 2027-02-23 "$repo"; for c in "pulldown-cmark 0.13.6" "tokio 1.53.3"; do grep -q "^VET [a-z]* $c " "$t/out" && { cat "$t/out"; bad "repo: $c refused on its EXPIRES day"; } || ok "repo: $c valid up to 2027-02-23"; done
run 2027-02-24 "$repo"
for c in "pulldown-cmark 0.13.6" "tokio 1.53.3"; do grep -q "^VET expired $c .*: 2027-02-23 (today 2027-02-24)$" "$t/out" && ok "repo: $c refused after its EXPIRES" || { cat "$t/out"; bad "repo: $c not refused after 2027-02-23"; }; done
[ "$rc" = 1 ] || bad "repo after expiry: rc=$rc (want 1)"
[ $fails = 0 ] && echo "vet-expiry.test: all ok" || { echo "vet-expiry.test: FAILED"; exit 1; }
