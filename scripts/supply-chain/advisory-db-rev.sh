#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# advisory-db-rev.sh <label> <dir>... — record the revision of the advisory DB a scanner fetched
# THIS run (operator decision 2026-11-19 15:49Z: advisory data fetched fresh at scan time, fetched
# revision recorded in the gate log, a failed fetch fails the gate, no stale-cache pass).
# For every <dir> that is a git checkout: logs "<label> advisory DB: <dir> <commit> <commit date>".
# Fails closed when no <dir> is a git checkout (nothing fetched) or the newest commit is older than
# ADVISORY_MAX_AGE_DAYS (default 14: the RustSec DB gets commits several times a week; an older head
# means a stale mirror/cache, not fresh data). env: NOW (epoch, tests only).
set -euo pipefail
label=${1:?usage: advisory-db-rev.sh <label> <dir>...}; shift
max=${ADVISORY_MAX_AGE_DAYS:-14}
now=${NOW:-$(date -u +%s)}
n=0
for d in "$@"; do
  [ -d "$d" ] || continue
  rev=$(git -C "$d" rev-parse --verify -q HEAD 2>/dev/null) || continue
  ts=$(git -C "$d" log -1 --format=%ct "$rev")
  echo "$label advisory DB: $d $rev $(date -u -d "@$ts" +%Y-%m-%dT%H:%M:%SZ) (fetched by this run, checked $(date -u -d "@$now" +%Y-%m-%dT%H:%M:%SZ))"
  [ $(( (now - ts) / 86400 )) -le "$max" ] || { echo "::error::$label advisory DB $d head $rev is older than $max days: stale, failing the gate" >&2; exit 1; }
  n=$((n + 1))
done
[ "$n" -gt 0 ] || { echo "::error::$label: no fetched advisory DB checkout in: $* (fetch failed or the DB moved): failing the gate" >&2; exit 1; }
