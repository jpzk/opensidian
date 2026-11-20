# NEGATIVE: signed index fetched, no signature verification at all (committed keyring present but unused)
KR=$HERE/../supply-chain/keys/archive.gpg
curl -4 -fsS -o "$W/InRelease" "https://example.org/dists/x/InRelease" || die "fetch"
grep '^Date:' "$W/InRelease"
