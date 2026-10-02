# Third-party notices

opensidian is **GPL-3.0-or-later** (see LICENSE). It bundles or derives from the
works listed here. Each section carries the upstream licence text as that
licence requires, and states how it sits with the GPL.

Compatibility summary: every dependency is permissive (MIT / Apache-2.0 / BSD /
ISC / Zlib / Unicode-3.0 / 0BSD / CC0) or weak copyleft (MPL-2.0, whose §3.3
grants GPL compatibility explicitly). Nothing here is GPL-incompatible. The
version is 3 and not 2 because `tao`, the crate that opens the app's window, is
Apache-2.0-only, which GPLv2 cannot link.
---

## Catppuccin (Mocha palette)

**Used in:** `ui/style.css` — the dark theme's colour tokens.
**Upstream:** https://github.com/catppuccin/catppuccin
**Licence:** MIT

Fourteen of the twenty-eight distinct colour values in `ui/style.css` are
byte-identical to the Catppuccin **Mocha** palette (231 occurrences in total):

| value     | Catppuccin name |
|-----------|-----------------|
| `#1e1e2e` | Base            |
| `#181825` | Mantle          |
| `#11111b` | Crust           |
| `#313244` | Surface0        |
| `#45475a` | Surface1        |
| `#585b70` | Surface2        |
| `#6c7086` | Overlay0        |
| `#a6adc8` | Subtext0        |
| `#bac2de` | Subtext1        |
| `#cdd6f4` | Text            |
| `#89b4fa` | Blue            |
| `#f9e2af` | Yellow          |
| `#f38ba8` | Red             |
| `#cba6f7` | Mauve           |

These were used without attribution up to and including v0.8. That was an
oversight, not a claim of authorship, and this file corrects it.

```
MIT License

Copyright (c) 2021 Catppuccin

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR OTHER DEALINGS IN THE SOFTWARE.
```

---

## Minimal (community theme, test fixture)

**Used in:** `src-tauri/tests/fixtures/themefs/Minimal/` — a REAL community
theme carried as a `cargo test` fixture only (it is `include_str!`d by tests
in `src-tauri/src/themefs.rs` and is NOT compiled into the app). It is exactly
what stock app 1.13.7's own community-theme installer wrote: `manifest.json`
(version 9.0.2) and `theme.css` (264,778 bytes, md5
`b73d22cec0325a10785d8ed6f21013b1`). The install transcript and screenshots
that prove that provenance are kept in the maintainer's local test harness and
are not distributed with this repository.

**Upstream:** https://github.com/kepano/obsidian-minimal
**Licence:** MIT (full text also at
`src-tauri/tests/fixtures/themefs/Minimal/LICENSE-obsidian-minimal`, fetched
from the upstream repository the day the fixture was made)

```
MIT License

Copyright (c) 2020-2024 Steph Ango (@kepano)

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

---

## Solarized (community theme, test fixture)

**Used in:** `src-tauri/tests/fixtures/themeone/Solarized/` — a REAL community
theme carried as a `cargo test` fixture only (`include_str!`d by a test in
`src-tauri/src/themefs.rs`; NOT compiled into the app). Installed by stock app
1.13.7's own community-theme installer: `manifest.json` (version 1.1.5) and
`theme.css` (11,856 bytes, md5 `3acabef0cc88d3e2fad5b7cda0b6c5b4`).

**Upstream:** https://github.com/harmtemolder/obsidian-solarized
**Licence:** MIT — full upstream text at
`src-tauri/tests/fixtures/themeone/Solarized/LICENSE-obsidian-solarized`
(copyright line as upstream states it: "Copyright (c) 2020 Steven Martin").

---

## Unresolved: CSS token contamination

Separately from the palette above, and NOT covered by any licence grant:

**46 CSS custom-property values in `ui/style.css` are byte-identical to values
extracted from `obsidian.asar`**, including derived formulas that do not arise
by convergence, e.g.:

```
--bold-weight: min(900, calc(var(--font-weight) + var(--bold-modifier)))
--heading-spacing: calc(var(--space-unit) * 8)
--checkbox-margin-inline-start: calc(var(--space-unit) * 3)
```

The stock app is proprietary software. These values are not ours to ship and were
not obtained by the black-box recon this project otherwise requires. Of the 48
custom properties compared, **46 are identical and 2 differ**.

The comparison transcript is deliberately NOT in this repository: it is itself
a record of values read out of the proprietary bundle, and committing it would
redistribute them. It is held out of tree by the maintainer.

This is disclosed, not resolved. Releases carrying it are marked **prerelease**
and must not be presented as a general-availability build. Resolving it means
re-deriving each of the 46 values from measurement or from an independent
source, and recording that provenance. A provenance check that enforces this
for NEW values exists, unmerged, in the maintainer's local test harness (which
is not distributed with this repository) and is not part of this tag.
