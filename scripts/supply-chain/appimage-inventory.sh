#!/bin/bash
# appimage-inventory.sh VER ARCH (run on a Linux box, never in CI secrets context): download the released portable AppImage, extract it, inventory every file,
# and record the direct NEEDED graph (LD_DEBUG=files "needed by") of every ELF in the bundle.
set -uo pipefail
V=${1:-0.9.6}; A=${2:-x86_64}; W=/var/tmp/appimg298/inv-$V-$A; mkdir -p $W; cd $W
F=opensidian-$V-$A-portable.AppImage
[ -s $F ] || curl -4 -fsSL -m 600 -o $F.part https://github.com/jpzk/opensidian/releases/download/v$V/$F && mv $F.part $F 2>/dev/null
ls -l $F; sha256sum $F | tee appimage.sha256
if [ "$A" = "$(uname -m)" ]; then rm -rf squashfs-root; chmod +x $F; ./$F --appimage-extract >/dev/null 2>&1; fi
[ -d squashfs-root ] || { echo "NO-EXTRACT"; exit 1; }
R=squashfs-root
(cd $R && find . -type f -o -type l | LC_ALL=C sort) > files.txt
(cd $R && find . -type f -name '*.so*' -o -type f -path '*webkit2gtk-4.1/*' | LC_ALL=C sort | xargs sha256sum) > libs.sha256
echo "files $(wc -l < files.txt)  so-or-helper $(wc -l < libs.sha256)  usr/lib .so: $(ls $R/usr/lib | grep -c '\.so')"
du -sb $R/usr/lib | tee lib-bytes.txt
# NEEDED graph: resolve with the bundle first (as AppRun does), host for the excludelist
: > needed.tsv; : > notfound.txt
for e in $R/usr/bin/opensidian $R/usr/lib/*.so* $R/usr/lib/webkit2gtk-4.1/WebKit*Process $R/usr/lib/webkit2gtk-4.1/injected-bundle/*.so; do
  [ -f "$e" ] || continue
  LD_LIBRARY_PATH=$PWD/$R/usr/lib LD_DEBUG=files /lib64/ld-linux-x86-64.so.2 --list "$e" 2>dbg.txt >ldd.txt
  grep -E 'needed by' dbg.txt | sed -E 's/^ *[0-9]+: *file=([^ ]+) *\[0\]; *needed by ([^ ]+).*/\2\t\1/' | sed "s#$PWD/$R/##g" >> needed.tsv
  grep 'not found' ldd.txt | sed "s#^#$(basename $e): #" >> notfound.txt
  awk -v e="$(basename $e)" '/=>/ {print e"\t"$1"\t"$3}' ldd.txt | sed "s#$PWD/$R/##g" >> resolved.tsv
done
LC_ALL=C sort -u needed.tsv -o needed.tsv; LC_ALL=C sort -u resolved.tsv -o resolved.tsv
echo "needed edges $(wc -l < needed.tsv); not-found lines $(wc -l < notfound.txt)"
# which resolved libs come from the HOST (not the bundle) = the current excludelist in effect on this host
awk -F'\t' '$3 !~ /^usr\/lib/ {print $2"\t"$3}' resolved.tsv | sort -u > from-host.tsv
echo "host-resolved distinct: $(wc -l < from-host.tsv)"
echo INV-DONE
