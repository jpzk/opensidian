# NEGATIVE: verified against a keyring that is itself downloaded (same path as the committed one)
KR=$HERE/../supply-chain/keys/archive.gpg
curl -4 -fsS -o "$KR" "https://example.org/archive.gpg"
curl -4 -fsS -o "$W/InRelease" "https://example.org/dists/x/InRelease" || die "fetch"
gpgv --keyring "$KR" --output "$W/Release" "$W/InRelease" || die "signature"
