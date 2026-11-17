<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ui/vendor/mermaid — mermaid 11.17.2, vendored

Loaded on demand from 'self' by ui/plugins/api-functions.js (loadMermaid -> window.mermaid) the first time
a note with a mermaid code block is shown (or a plugin calls loadMermaid). Unmodified upstream bytes;
no network and no eval under the existing CSP (script-src 'self'; the bundle's only Function(...) is lodash's "return this" global fallback, never reached because globalThis exists). Notes without a mermaid block never load it.

- package: mermaid
- version: 11.17.2 (exact pin)
- published: 2026-08-25T11:52:39.930Z (npm registry `time["11.17.2"]`)
- vendored: 2026-11-17 (12 weeks after publish; 6-week rule: on or before 2026-10-06)
- newer releases at vendoring time, not taken: 12.0.0 (2026-09-10), 12.1.0 (2026-10-02), 12.2.0 / 12.3.0 (< 6 weeks).
  11.17.2 is the last release of the mature 11.x line; 12.x is a new major (new parser stack, +50% bundle size).
- license: MIT (upstream LICENSE copied beside it); bundled third-party code is listed in THIRD-PARTY.md
- tarball: https://registry.npmjs.org/mermaid/-/mermaid-11.17.2.tgz
- tarball integrity (npm dist.integrity, verified on download): `sha512-V6K3C8EBdEsPFZXSKMJe6ppQOENxuHARr9GvHX4hh47lAbhMRD9qf4oEK7LoaRQxULMa80/qt5gHO73aCleBBg==`
- tarball sha256: `6ad2f42c3fc26bbf9e45cbb6d11898972573ea52b33a5f4ff51952899f950ffd`
- files taken from the tarball: package/dist/mermaid.min.js (IIFE build, sets globalThis.mermaid), package/LICENSE
- LICENSES-bundled.txt (ours): licence texts of the 74 packages bundled inside mermaid.min.js, each taken from that package's own npm tarball

## Known-vulnerability check (GitHub advisory database, 2026-11-17)

mermaid: every advisory is fixed at or before 11.16.1 (latest batch GHSA-rhh3-jpg6-66xh, GHSA-c4c3-pg64-4m4v,
GHSA-6x64-9x62-f2gx, GHSA-3rrr-jr9j-h3q3, GHSA-2v8p-3f2j-5mp7: fixed in 11.16.1). 11.17.2 has no open advisory.

Bundled DOMPurify is 3.4.12 (its own version string in the bundle). Advisories that still cover 3.4.12:
- GHSA-55q2-fjhq-7xh7, GHSA-mrv2-3pqr-j5jr, GHSA-6688-9rhm-gjv2: IN_PLACE mode only. Mermaid sanitizes strings, never
  nodes in place (IN_PLACE is a no-op for string input).
- GHSA-cg2g-xh64-v4q7: SAFE_FOR_TEMPLATES only (template expressions in shadow roots). Mermaid never sets it.
- GHSA-c6gr-qvv4-5v3f: PARSER_MEDIA_TYPE application/xhtml+xml only. Mermaid never sets it.
The only path to those options is mermaid's `dompurifyConfig` setting; loadMermaid adds `dompurifyConfig` (and the other
config keys that change sanitizing, styling or fonts) to mermaid's `secure` list, so a `%%{init}%%` directive in a note
cannot set it. The fixed DOMPurify (3.4.17, 2026-10-22) is younger than 6 weeks, so no release that bundles it qualifies for the pin yet.

## sha256 (machine-checkable copy: SHA256SUMS)

Check: `cd ui/vendor/mermaid && sha256sum -c SHA256SUMS`

```
581ed7d74bd9048d0e3a91363927d72ef22942d7722546b27f7cc29e35390eb8  mermaid.min.js
ec9fb67dcb25eccc416ed56e1aab819222c805a2a4bfe4cb19e7556bf2ffde80  LICENSE
bdefdbd50d5123e1e84d6a8338485a5a1591965ea9da4f2c8e687ae5fb169660  LICENSES-bundled.txt
```
