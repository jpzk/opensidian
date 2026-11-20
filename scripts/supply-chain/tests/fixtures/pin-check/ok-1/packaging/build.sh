DIGEST=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
IMG=docker.io/library/debian@$DIGEST
FED=localhost/fedora@$DIGEST
TOOLS="dpkg-dev=1.22.22 lintian=2.122.0"
RSVG=librsvg2-tools-2.62.0-1.fc44
podman image exists "$IMG" || podman pull -q "$IMG"
podman run --rm --network=host -v $W:/w "$IMG" bash -c 'apt-get -y -q install '"$TOOLS"' >/dev/null'
dnf -y -q --repo=fedora install '"$RSVG"' rpm-build-6.0.1-2.fc44 >/tmp/dnf.log 2>&1
curl -4 -fsSL -o "$A" "$OCI_URL"
echo "$OCI_SHA  $A" | sha256sum -c -
rm -rf $C/oci && mkdir $C/oci && tar xJf "$A" -C $C/oci
ID=$(podman pull -q oci:$C/oci) && podman tag "$ID" localhost/fedora:44
podman image exists "$FED" || exit 1
