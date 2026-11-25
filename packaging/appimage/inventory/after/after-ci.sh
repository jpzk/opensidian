#!/bin/bash
# after-ci.sh RUN_ID — #298 before/after from a release.yml dry run (data only: gh + jq, no downloaded code runs).
# before = baseline-v096.tsv (v0.9.6 code-scanning alerts, osv-scanner tool, per arch: arch sev pkg@ver rule id)
# after  = the run's osv gate output (pinned osv-scanner v2.7.1 on parts/webkit.cdx.json) + its SBOM artifacts.
set -euo pipefail; R=${1:?run id}; D=$(cd "$(dirname "$0")" && pwd); T=$(mktemp -d); trap 'rm -rf $T' EXIT
: "${GH_TOKEN:?export GH_TOKEN}"; REPO=jpzk/opensidian; mkdir -p $T/old
for A in x86_64 aarch64; do F=opensidian-0.9.6-$A.cdx.json; gh release download v0.9.6 -R $REPO -D $T/old -p $F -p $F.sha256; (cd $T/old && sha256sum -c $F.sha256); done
q='[.components[] | select(any(.properties[]?; .name=="opensidian:bundled-libs")) | "\(.name)@\(.version)"] | sort | .[]'
for A in x86_64 aarch64; do
  J=$(gh run view $R -R $REPO --json jobs -q ".jobs[]|select(.name|test(\"AppImages.*$A\"))|.databaseId")
  curl -4 -fsSL -H "Authorization: Bearer $GH_TOKEN" https://api.github.com/repos/$REPO/actions/jobs/$J/logs | sed 's/\x1b\[[0-9;]*m//g' > $T/$A.log
  echo "== $A (job $J)"; grep -oE '\[appdir\] bundled [0-9]+ libs|appdir .*: [0-9]+ .so files checked|appimage-excl-check: .*|osv-scanner sha256: +[0-9a-f]+|summary: .*' $T/$A.log | sort -u
  awk '/no-fix-yet|fixed-in/{print $4"\t"$5}' $T/$A.log | sort -u > $T/after-$A.ids
  aid=$(gh api "repos/$REPO/actions/runs/$R/artifacts?per_page=100" -q ".artifacts[]|select(.name|test(\"^sbom-[0-9.]+-$A\$\"))|.id")
  gh api repos/$REPO/actions/artifacts/$aid/zip > $T/s.zip; unzip -o -q $T/s.zip -d $T/new-$A; F=$(ls $T/new-$A/*-$A.cdx.json)
  (cd $T/new-$A && sha256sum -c $(basename $F).sha256)
  echo "bundled source pkgs: v0.9.6 $(jq -r "$q" $T/old/opensidian-0.9.6-$A.cdx.json | wc -l) -> head $(jq -r "$q" $F | wc -l); removed:"
  comm -23 <(jq -r "$q" $T/old/opensidian-0.9.6-$A.cdx.json) <(jq -r "$q" $F) | sed 's/^/  /'
  echo "before (v0.9.6, all 106 alerts):      $(awk -F'\t' -v a=$A '$1==a{print $2}' $D/baseline-v096.tsv | sort | uniq -c | tr -s ' \n' ' ')"
  echo "before (Ubuntu/bundle part only):     $(awk -F'\t' -v a=$A '$1==a && $4~/^UBUNTU/{print $2}' $D/baseline-v096.tsv | sort | uniq -c | tr -s ' \n' ' ')"
  echo "after  (bundle part, sev by rule id): $(awk -F'\t' -v a=$A 'NR==FNR{k[$1"\t"$2]=1;next} $1==a && (($3"\t"$4) in k){print $2}' $T/after-$A.ids $D/baseline-v096.tsv | sort | uniq -c | tr -s ' \n' ' ')"
  echo "after ids total $(wc -l < $T/after-$A.ids), not in v0.9.6 baseline: $(awk -F'\t' -v a=$A 'NR==FNR{if($1==a)k[$3"\t"$4]=1;next} !(($1"\t"$2) in k)' $D/baseline-v096.tsv $T/after-$A.ids | wc -l)"
done
