# NEGATIVE: verified against the pinned keyring but the failure is ignored
KR=$HERE/../supply-chain/keys/archive.gpg
curl -4 -fsS -o "$W/InRelease" "https://example.org/dists/x/InRelease" || die "fetch"
gpgv --keyring "$KR" --output "$W/Release" "$W/InRelease" || true
