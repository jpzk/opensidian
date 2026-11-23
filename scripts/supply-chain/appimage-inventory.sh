#!/bin/bash
# appimage-inventory.sh VER ARCH (run on a Linux box, never in CI secrets context): download the released portable AppImage, extract it, inventory every file,
# and record the direct NEEDED graph (LD_DEBUG=files "needed by") of every ELF in the bundle.
# Fail closed (#307): ARCH must be the host arch (the host's ld.so is what lists the bundle), every output is
# truncated per run (a rerun never keeps stale edges), and any download/extract/loader failure aborts before INV-DONE.
set -uo pipefail
die() { echo "INV-FAIL: $*" >&2; exit 1; }
V=${1:-0.9.6}; A=${2:-x86_64}; W=/var/tmp/appimg298/inv-$V-$A; mkdir -p $W; cd $W || die "cd $W"
case "$A" in
  x86_64)  LDSO=/lib64/ld-linux-x86-64.so.2 ;;
  aarch64) LDSO=/lib/ld-linux-aarch64.so.1 ;;
  *) die "unsupported arch $A" ;;
esac
[ "$A" = "$(uname -m)" ] || die "arch $A != host $(uname -m): the host loader cannot inspect it; run on a $A box"
[ -x "$LDSO" ] || die "loader $LDSO missing"
F=opensidian-$V-$A-portable.AppImage
if [ ! -s $F ]; then
  rm -f $F.part
  curl -4 -fsSL -m 600 -o $F.part https://github.com/jpzk/opensidian/releases/download/v$V/$F || { rm -f $F.part; die "download $F"; }
  mv $F.part $F || die "mv $F"
fi
ls -l $F; sha256sum $F | tee appimage.sha256 || die "sha256sum $F"
rm -rf squashfs-root; chmod +x $F || die "chmod $F"; ./$F --appimage-extract >/dev/null 2>&1 || die "extract $F"
[ -d squashfs-root ] || die "NO-EXTRACT"
R=squashfs-root
(cd $R && find . -type f -o -type l | LC_ALL=C sort) > files.txt
(cd $R && find . -type f -name '*.so*' -o -type f -path '*webkit2gtk-4.1/*' | LC_ALL=C sort | xargs sha256sum) > libs.sha256
echo "files $(wc -l < files.txt)  so-or-helper $(wc -l < libs.sha256)  usr/lib .so: $(ls $R/usr/lib | grep -c '\.so')"
du -sb $R/usr/lib | tee lib-bytes.txt
# NEEDED graph: resolve with the bundle first (as AppRun does), host for the excludelist
: > needed.tsv; : > notfound.txt; : > resolved.tsv; : > from-host.tsv; lfail=0
for e in $R/usr/bin/opensidian $R/usr/lib/*.so* $R/usr/lib/webkit2gtk-4.1/WebKit*Process $R/usr/lib/webkit2gtk-4.1/injected-bundle/*.so; do
  [ -f "$e" ] || continue
  LD_LIBRARY_PATH=$PWD/$R/usr/lib LD_DEBUG=files $LDSO --list "$e" 2>dbg.txt >ldd.txt || { echo "loader failed: $e" >&2; lfail=$((lfail+1)); continue; }
  grep -E 'needed by' dbg.txt | sed -E 's/^ *[0-9]+: *file=([^ ]+) *\[0\]; *needed by ([^ ]+).*/\2\t\1/' | sed "s#$PWD/$R/##g" >> needed.tsv
  grep 'not found' ldd.txt | sed "s#^#$(basename $e): #" >> notfound.txt
  awk -v e="$(basename $e)" '/=>/ {print e"\t"$1"\t"$3}' ldd.txt | sed "s#$PWD/$R/##g" >> resolved.tsv
done
[ "$lfail" = 0 ] || die "$lfail bundle entries could not be listed by $LDSO"
LC_ALL=C sort -u needed.tsv -o needed.tsv; LC_ALL=C sort -u resolved.tsv -o resolved.tsv
echo "needed edges $(wc -l < needed.tsv); not-found lines $(wc -l < notfound.txt)"
# which resolved libs come from the HOST (not the bundle) = the current excludelist in effect on this host
awk -F'\t' '$3 !~ /^usr\/lib/ {print $2"\t"$3}' resolved.tsv | sort -u > from-host.tsv
echo "host-resolved distinct: $(wc -l < from-host.tsv)"
echo INV-DONE
