#!/bin/bash
# fixture: smoke-run.sh shapes - the image from fedora-env.sh image, a local .deb install
F44=$(bash "$ROOT/packaging/ci/fedora-env.sh" image) || exit 1
podman run -d --name x --network=host -v "$ROOT:/w" "$F44" sleep infinity
sudo apt-get install -y --no-install-recommends "./$PKG"
