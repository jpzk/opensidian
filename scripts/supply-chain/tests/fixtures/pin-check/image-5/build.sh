DIGEST=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
IMG=localhost/fedora@$DIGEST
echo "$OCI_SHA  $A" | sha256sum -c -
ID=$(podman pull -q oci:$C/oci) && podman tag "$ID" localhost/fedora:44
