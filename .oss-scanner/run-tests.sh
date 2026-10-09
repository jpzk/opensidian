#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# opensidian-test [cargo test args]: run the test suite the way it is meant to run, offline,
# headless (Xvfb), as the unprivileged user "scanner" with its own HOME.
#   - As root, two tests are void: root ignores the 0555 mode a refused-write test relies on,
#     and the folder picker must deny /root, which is root's own HOME.
#   - The scanner's image layer sets HOME=/root; a non-root user inheriting it cannot create
#     the vault lock directory and the vault/sandbox tests fail. HOME is reset here.
set -e
if [ "$(id -u)" = 0 ]; then
  exec runuser -u scanner -- env HOME=/home/scanner "$0" "$@"
fi
cd /src/src-tauri
export CARGO_NET_OFFLINE=true
exec xvfb-run -a cargo test --release --locked "$@"
