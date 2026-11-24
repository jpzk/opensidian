# #298 before/after: v0.9.6 portable AppImage vs the slimmed bundle

`GH_TOKEN=... bash after-ci.sh <release.yml run id>` — data only (gh + jq): the run's AppImage build-log line
`[appdir] bundled N libs`, its pinned osv-scanner gate output on `parts/webkit.cdx.json`, and its sha256-verified
SBOM artifacts, against the v0.9.6 release SBOMs and `baseline-v096.tsv` (the open osv-scanner code-scanning alerts
on v0.9.6: arch, severity, package@version, rule id, alert number).

Run 45167393287 (head 34cb378, dry_run=true) -> `after-ci-45167393287.out`, per arch (x86_64 = aarch64):

- bundled libs 83 (v0.9.6 run 44731292971) -> 75
- bundled source packages 51 -> 47 (dbus, krb5, sqlite3, util-linux removed; nothing else changed)
- bundle-part osv findings 93 -> 55, all a subset of v0.9.6's (0 new)
  - before: 2 critical, 40 high, 49 medium, 2 low
  - after:  2 critical, 25 high, 27 medium, 1 low
- all alerts on the merged SBOM (adds 13 Rust-crate alerts, unchanged): 106 -> 68
