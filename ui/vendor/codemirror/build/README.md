<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ui/vendor/codemirror/build — build inputs of cm6.js

`cm6.js` is an esbuild bundle: no upstream npm file matches its bytes, so its dependency list
comes from these committed inputs, not from prose (issue #188).

- `package.json`, `package-lock.json`, `entry.js`, `banner.txt`: the ORIGINAL recipe cm6.js was
  built from, copied byte-for-byte from jpzk/opensidian-harness `bin/hz/cm6-vendor/` at commit
  d255ab8ba352043aad2846124287203080fdca88 (2026-10-13). Not reconstructed. `banner.txt` differs
  from the original only in its two comment lines that named the retired prose notes (cm6.js
  carries the same banner; its sha256 changed accordingly, old and new are in vendor.lock.json).
- `build.sh`: NEW, a repo-local rewrite of the original recipe's build/check modes (the original
  drove a remote build box over ssh). `CMP=../cm6.js sh build.sh` rebuilds with
  `npm ci --ignore-scripts` + esbuild and asserts byte identity; the opt-in `cm6-rebuild` job of
  `.github/workflows/supply-chain.yml` runs it.
- `package-lock.json` is the source of truth for the SBOM's CodeMirror components
  (`packages[*].version` + `integrity`; the 22 non-dev entries are bundled into cm6.js, the 27
  dev entries are esbuild 0.28.2 and its per-platform binaries, build-only, never shipped).
- `scripts/supply-chain/vendor-verify.sh` checks every entry's version/integrity/resolved against
  the npm registry and the cm6.js sha256 against `ui/vendor/vendor.lock.json`.
- esbuild flags: `--bundle --format=iife --platform=browser --target=es2020 --minify
  --charset=utf8 --legal-comments=none --banner:js="$(cat banner.txt)"`.
