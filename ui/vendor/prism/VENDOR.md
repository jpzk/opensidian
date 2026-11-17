<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ui/vendor/prism — prismjs 1.30.0, vendored

Loaded on demand from 'self' by ui/plugins/api-functions.js (loadPrism -> window.Prism, manual mode: never auto-highlights the app;
the app's own fenced-code highlighting, ui/codehl.js #186, loads it through that same loader). Unmodified upstream bytes;
no network, no eval, served under the existing CSP (script-src 'self').

- package: prismjs
- version: 1.30.0 (exact pin)
- published: 2025-03-10T17:56:20.460Z (npm registry `time["1.30.0"]`; 6-week rule: on or before 2026-09-06 for this pin)
- license: MIT (upstream LICENSE copied beside it)
- tarball: https://registry.npmjs.org/prismjs/-/prismjs-1.30.0.tgz
- tarball integrity (npm dist.integrity, verified on download): `sha512-DEvV2ZF2r2/63V+tK8hQvrR2ZGn10srHbXviTlcv7Kpzw8jWiNTqbVgjO3IY8RxrrOUF8VPMQQFysYYYv0YZxw==`
- tarball sha256: `ac16a9106a28c53b6a6313993c816a3a02525fdfefd7614fc1703df915a9fc11`
- files taken from the tarball: package/prism.js (core + markup, css, clike, javascript), package/LICENSE,
  and (#186) package/components/prism-<id>.min.js for every other language of package/components.json
  (293 files, unmodified) concatenated into prism-components.js by components.sh (dependency order,
  one newline after each file; `sh components.sh <extracted package dir> | sha256sum` reproduces it).
  No plugin files (autoloader, file-highlight, ...): nothing that fetches or highlights on its own.
  prism-components.js was scanned for eval / new Function / fetch / XMLHttpRequest / importScripts:
  only matches inside keyword regexes (Nix fetchTarball, NSIS Function, ... ), no call.

## sha256 (machine-checkable copy: SHA256SUMS)

Check: `cd ui/vendor/prism && sha256sum -c SHA256SUMS`

```
b801451d9b4cbf1857715a97ecae442e26c111bab6e19fee0e83dfda70cc2900  prism.js
2b947f0901a7ffcf08a89957da9783c0e9c6e72cb6ce8e959f501ab5409e4d2b  LICENSE
1c7b134fa7169e1ecc565dc96658526c13d7768b39482f92ad5dcaca899bd1eb  prism-components.js
b5d1e322d6a674bd12f1ba5d1b1c438134da4caec738542899c54148f71cc427  components.sh
```
