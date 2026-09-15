# Third-party notices

rustidian is **GPL-3.0-or-later** (see LICENSE). It bundles or derives from the
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

Obsidian is proprietary software. These values are not ours to ship and were
not obtained by the black-box recon this project otherwise requires. Of the 48
custom properties compared, **46 are identical and 2 differ**.

The comparison transcript is deliberately NOT in this repository: it is itself
a record of values read out of the proprietary bundle, and committing it would
redistribute them. It is held out of tree by the maintainer.

This is disclosed, not resolved. Releases carrying it are marked **prerelease**
and must not be presented as a general-availability build. Resolving it means
re-deriving each of the 46 values from measurement or from an independent
source, and recording that provenance. A provenance check that enforces this
for NEW values exists on the unmerged `goal/theme-mode` branch
(`scripts/theme-provenance-verify.sh`) and is not part of this tag.
