# NEGATIVE: verified against an UNPINNED host keyring (not committed, not in keyrings.tsv)
KR=/usr/share/keyrings/ubuntu-archive-keyring.gpg
curl -4 -fsS -o "$W/InRelease" "https://example.org/dists/x/InRelease" || die "fetch"
gpgv --keyring "$KR" --output "$W/Release" "$W/InRelease" || die "signature"
