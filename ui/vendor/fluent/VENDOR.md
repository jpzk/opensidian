<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# ui/vendor/fluent — @fluent/bundle 0.19.1, vendored

Project Fluent's runtime (issue #220, the UI translation engine). Loaded as a
classic script from 'self' by ui/index.html before ui/i18n.js; it defines
window.FluentBundle (FluentBundle, FluentResource). Unmodified upstream bytes
(the package's UMD build `index.js`, renamed fluent-bundle.js); no network,
no eval / new Function, no dependency, served under the existing CSP
(script-src 'self'). No npm at build time.

The same .ftl files (repo-root locales/<lang>/main.ftl) are read on the Rust side
by the fluent-bundle crate (src-tauri/src/i18n.rs), so both sides share one
source of truth.

- package: @fluent/bundle
- version: 0.19.1 (exact pin)
- published: 2025-04-02T13:40:39.598Z (npm registry `time["0.19.1"]`; 6-week rule: on or before 2026-10-06 for a 2026-11-17 pin)
- license: Apache-2.0 (package.json "license"; the tarball ships no LICENSE file, so the
  LICENSE beside it is upstream's from https://github.com/projectfluent/fluent.js at tag
  `@fluent/bundle@0.19.1`, gitHead 88582193c29a689241a4f7b2ca7086c802840264)
- tarball: https://registry.npmjs.org/@fluent/bundle/-/bundle-0.19.1.tgz
- tarball integrity (npm dist.integrity, verified on download): `sha512-SWJLZrPamDPsJlFFOW1nkgN0j0rbPbmSdmK0XAoXlyqKieLtMVl4vzng3aR5pwKoUx0scug8+YY2oct3fdfy9A==`
- tarball sha1 (npm dist.shasum): `d9931a0be577fc6ebebd97d73c6d15d20fa5f54c`
- tarball sha256: `8af8e91a347fa98a9f2469cab95096ac113eff165c82231033a0f98e10d49a2c`
- files taken: package/index.js -> fluent-bundle.js; LICENSE (upstream repo, see above)

## sha256 (machine-checkable copy: SHA256SUMS)

Check: `cd ui/vendor/fluent && sha256sum -c SHA256SUMS`

```
08fc26d9b0d7e43c8c96eeb4757a94cc48ce2b4c36b5547ebc415dc7dc94f39c  fluent-bundle.js
cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30  LICENSE
```
