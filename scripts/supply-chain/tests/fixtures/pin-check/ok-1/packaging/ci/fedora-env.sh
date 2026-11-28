#!/bin/bash
# fixture: the lock hash pin
LOCK_SHA256=548abd483eb7ec027ea7e2c3115c3716468329b1a757edc65bf1f104f6d35650
# fixture: `fedora-env.sh image` = the digest-checked OCI load (a VAR=$(bash .../fedora-env.sh image) is pinned)
DIGEST=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
IMG=localhost/fedora@$DIGEST
curl -4 -fsSL -o "$A" "$OCI_URL"
echo "$OCI_SHA  $A" | sha256sum -c -
ID=$(podman pull -q "oci:$C/oci") && podman tag "$ID" localhost/fedora:44
podman image exists "$IMG" || exit 1
