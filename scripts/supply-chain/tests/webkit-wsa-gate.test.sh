#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -uo pipefail
D=$(cd "$(dirname "$0")/.." && pwd); T=$(mktemp -d); trap 'rm -rf "$T"' EXIT; fail=0
mk() { printf '{"components":[%s]}\n' "$1" > "$T/s.json"; }
c() { printf '{"name":"webkit2gtk4.1","purl":"pkg:rpm/fedora/webkit2gtk4.1@%s?arch=x86_64&distro=fedora-44"}' "$1"; }
t() { local want=$1 desc=$2; shift 2; "$@" >"$T/o" 2>&1; local rc=$?
  if { [ "$want" = 0 ] && [ $rc = 0 ]; } || { [ "$want" != 0 ] && [ $rc != 0 ]; }; then echo "ok   $desc"; else echo "FAIL $desc rc=$rc"; cat "$T/o"; fail=1; fi; }
mk "$(c 2.54.3-2.fc44)";  t 0 "2.54.3-2.fc44 passes every WSA" "$D/webkit-wsa-gate.sh" "$T/s.json"
mk "$(c 2.54.1-1.fc44)";  t 1 "2.54.1 fails (WSA-2026-0007)" "$D/webkit-wsa-gate.sh" "$T/s.json"
mk "$(c 2.52.6-1.fc44)";  t 1 "2.52.6 fails (WSA-2026-0006)" "$D/webkit-wsa-gate.sh" "$T/s.json"
mk "$(c 2.60.0-1.fc45)";  t 0 "newer 2.60.0 passes" "$D/webkit-wsa-gate.sh" "$T/s.json"
mk '{"name":"x","purl":"pkg:rpm/fedora/glib2@2.86.0-1.fc44"}'; t 1 "missing webkit fails closed" "$D/webkit-wsa-gate.sh" "$T/s.json"
mk "$(c 2.54.3-2.fc44),$(c 2.54.1-1.fc44)"; t 1 "two webkit purls fail closed" "$D/webkit-wsa-gate.sh" "$T/s.json"
mk "$(c junk)";           t 1 "unparsable version fails closed" "$D/webkit-wsa-gate.sh" "$T/s.json"
mk "$(c 2.54.3-2.fc44)"; printf '# only comments\n' > "$T/e.tsv"; t 1 "empty table fails closed" "$D/webkit-wsa-gate.sh" "$T/s.json" "$T/e.tsv"
mk "$(c 2.54.3-2.fc44)"; printf 'WSA-X\t2027-01-01\t2.56.0\n' > "$T/n.tsv"; t 1 "new advisory above bundle fails" "$D/webkit-wsa-gate.sh" "$T/s.json" "$T/n.tsv"
exit $fail
