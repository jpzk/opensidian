# #298 — portable AppImage: bundle inventory, reachability, slimming, WebKitGTK 2.54 evaluation

Status: IN PROGRESS (iteration 2). Sections 1–2 final; 3–6 filled in by later iterations.

## 1. Inventory (v0.9.6 x86_64 portable, box-unpacked)

- Asset `opensidian-0.9.6-x86_64-portable.AppImage` sha256 `7a0d9938bbb4b9a3c42752415c235cf69946ed86dbb8d511cb6f3098c9778266`,
  unpacked on the box (Fedora 44) with `--appimage-extract` by `bin/box-inv.sh` (= product `scripts/supply-chain/appimage-inventory.sh`).
- 93 files; 87 libraries/helpers (83 `.so` in `usr/lib` + 3 WebKit helper processes + injected bundle); 215.9 MB in `usr/lib`.
- The 87 files map onto exactly the 51 source packages of the v0.9.6 SBOM. **No SBOM package contributes zero files**
  (so no alert can be dismissed as "package in SBOM but not shipped").
- Full per-file list (sha256, source package, version, direct NEEDED-by): product `packaging/appimage/inventory/inventory-v0.9.6-x86_64.tsv`;
  NEEDED graph (LD_DEBUG=files, no binutils on the box): `inv096/edges.tsv`.
- Non-library payload: `AppRun`, `usr/bin/opensidian`, `usr/lib/webkit2gtk-4.1/WebKit{Web,Network,GPU}Process`,
  `usr/share/glib-2.0/schemas/gschemas.compiled`, desktop file + icon. **No** command-line tools (no `xmlcatalog`, `mount`, `nsenter`,
  `dbus-daemon`, `journald`, `systemd-*-generator`), **no** GStreamer plugins, **no** gdk-pixbuf loader modules / `loaders.cache`,
  **no** glycin loaders, **no** GIO modules.

Alerted packages → bundled files (x86_64; aarch64 is the same set):

| package (alerts) | bundled files | who NEEDs them |
|---|---|---|
| webkit2gtk (21) | libwebkit2gtk-4.1.so.0, libjavascriptcoregtk-4.1.so.0, WebKit{Web,Network,GPU}Process, libwebkit2gtkinjectedbundle.so | opensidian |
| krb5 (19) | libgssapi_krb5.so.2, libkrb5.so.3, libk5crypto.so.3, libkrb5support.so.0 | **only** libsoup-3.0 (→ libgssapi_krb5) |
| libsoup3 (18) | libsoup-3.0.so.0 | libwebkit2gtk, opensidian |
| sqlite3 (11) | libsqlite3.so.0 | libsoup-3.0, libwebkit2gtk |
| util-linux (4) | libmount.so.1, libblkid.so.1 | libgio-2.0 (→ libmount → libblkid) |
| dbus (4) | libdbus-1.so.3 | libatk-bridge, libatspi, opensidian |
| gdk-pixbuf (3) | libgdk_pixbuf-2.0.so.0 | gtk/gdk, webkit, opensidian |
| cairo (3) | libcairo.so.2, libcairo-gobject.so.2 | gtk, pango, webkit, opensidian |
| systemd (2) | libsystemd.so.0, libudev.so.1 | webkit/JSC/helpers + libdbus (libsystemd); libgudev (libudev) |
| jpeg-xl (2) | libjxl.so.0.11, libjxl_cms.so.0.11 | libwebkit2gtk |
| libxml2 (1) | libxml2.so.16 | libwebkit2gtk, libxslt |
| libxslt (1) | libxslt.so.1 | libwebkit2gtk |
| gst-plugins-base1.0 (1) | libgst{allocators,app,audio,fft,gl,pbutils,tag,video}-1.0 (8 libs; **no libgstrtp**) | libwebkit2gtk |
| libjpeg-turbo (1) | libjpeg.so.8 | webkit, helpers |
| pixman (1) | libpixman-1.so.0 | libcairo |
| glib2.0 (1) | libglib/gobject/gio/gmodule-2.0 | everything |

(The other x86_64 alerts — 14 RUSTSEC on crates of the opensidian binary — are out of scope for the bundle.)

## 2. Reachability per alerted package (evidence, not guesses)

Evidence files: `evidence/reach.out` (bin/box-reach.sh) and `evidence/reach2.out` (bin/box-reach2.sh), run on the box
against the box-unpacked bundle (libs re-verified `sha256sum -c libs.sha256` → LIBS-SHA-OK) and against pinned upstream
source tarballs (sha256 recorded in the output; webkitgtk-2.52.6.tar.xz `179a2ea3…116fc` matches webkitgtk.org `.sums`).
"imports X" = the symbol name is in the library's dynamic string table (`grep -a -c`), i.e. the call is linked.
Absence of a string in a stripped binary is used only as corroboration, never alone.

| package | CVE / class | present? | reachable from opensidian? | evidence | verdict |
|---|---|---|---|---|---|
| webkit2gtk | Skia: CVE-2024-43091 (SkEmbossMaskFilter), CVE-2024-43767 (SkBlurMaskFilterImpl) | yes — both .cpp in `Source/ThirdParty/skia/CMakeLists.txt` (l.544, l.102); `SkEmbossMaskFilter`/`SkBlurMaskFilter` strings in libwebkit2gtk | yes — WebKitGTK 2.52 renders with Skia (`USE_SKIA` ON, OptionsGTK.cmake l.153); CSS/canvas blur, emboss via SVG/canvas filters | reach.out, reach2.out | affected (fix status vs. WebKit's Skia snapshot: §4) |
| webkit2gtk | Skia: CVE-2024-43768 (SkDeflate) | yes — `src/pdf/SkDeflate.cpp` compiled (l.606, 21 pdf files); `SkPDF` strings ×10 | only through Skia PDF output (printing) | reach2.out | affected |
| webkit2gtk | Skia: **CVE-2025-26416** (SkBmpStandardCodec, critical) | **no** — file exists in the tarball but is **not** in the Skia CMakeLists source list (compiled codecs = SkCodec core, Jpeg*, Png*, Swizzler, Sampler, Tiff utility only); 0 `SkBmpStandardCodec` strings in libwebkit2gtk. WebKit decodes BMP with its own `BMPImageDecoder` | — | reach2.out "codec files compiled" | **not_affected: code_not_present** |
| webkit2gtk | **CVE-2025-48622** (dng_misc_opcodes.cpp, Adobe DNG SDK) | **no** — no `dng_*` file anywhere in webkitgtk-2.52.6 Skia tree; 0 `dng_` strings | — | reach.out, reach2.out | **not_affected: code_not_present** |
| webkit2gtk | Chrome-Skia CVE-2024-8193/8198, 2025-0436/0444, 2026-3538/3909/3931; Huawei CVE-2025-54627 | Skia core compiled | yes (renderer) | Skia in-tree | affected |
| webkit2gtk | ANGLE CVE-2026-3536/5283/10018/10019/15109 | yes — `USE_ANGLE ${ENABLE_WEBGL}` (l.316); `ANGLE` strings ×20 | yes if a page uses WebGL | reach.out | affected |
| webkit2gtk | CVE-2025-66286 (send-request API), CVE-2026-43670 (CSP AudioWorklet), CVE-2026-78376 (UAF) | yes | yes (WebKit core) | — | affected |
| libsoup3 | 18 CVEs: WebSocket, HTTP/1+2 parsing, multipart, cookies/PSL, cache, NTLM, CRLF; **server-side** SoupServer CVE-2026-2436 + SoupAuthDomainDigest CVE-2026-3099 | yes (single .so) | client paths: yes (WebKitNetworkProcess + opensidian `soup_session_new`). Server: `soup_server_new` imported by libwebkit2gtk (remote inspector / automation); opensidian imports `webkit_web_context_set_automation_allowed` → not provably unreachable | reach.out | affected (all 18; fail closed) |
| krb5 | 19 CVEs (GSSAPI/krb5 lib) | yes (4 libs) | only via libsoup `gss_init_sec_context` (imported ×1; "Negotiate" auth when a server sends `WWW-Authenticate: Negotiate`). opensidian imports no `gss_*` | reach.out | affected → **removed from bundle (§3)**: its own deps com_err/keyutils are already host-provided (EXCL), so bundling krb5 is inconsistent; upstream excludelist marks it "disputed"; Fedora 44 has all 4 sonames |
| sqlite3 | 11 CVEs | yes | yes — libwebkit2gtk imports `sqlite3_open_v2` (localstorage.sqlite3, IndexedDB ×9); libsoup has cookie-jar-db/HSTS-db | reach.out | affected → **removed from bundle (§3)**, host libsqlite3.so.0 (F44: 3.51.2) |
| util-linux | CVE-2026-76642 (mount helper/post-mount hooks), 78408 (nsenter), 78409 (X-mount.subdir), 78410 (restricted bind mount, SUID mount(8)) | libmount/libblkid yes; mount(8)/nsenter **not shipped** | libgio imports only `mnt_table_parse*` / `mnt_monitor*` (mount-table monitoring); **0** `mnt_context_*` imports → never mounts | reach.out | not reachable → **removed from bundle (§3)** (util-linux is on every Linux; excludelist already has libuuid) |
| dbus | CVE-2026-120749/128885/128886/128987 | libdbus-1 client only; **no dbus-daemon** | all four are dbus-daemon / server-transport flaws | reach.out, inventory | **removed from bundle (§3)** — client lib must match the host bus anyway |
| systemd | CVE-2026-121876 (systemd-gpt-auto-generator), CVE-2026-40228 (systemd-journald wall forwarding) | libsystemd/libudev only; **neither binary shipped** | webkit/JSC import `sd_journal_send` (client logging) — the bug is in the journald daemon, not the client | reach.out | **not_affected: code_not_present** (stays bundled: libsystemd/libudev are absent on non-systemd distros) |
| libxml2 | CVE-2026-11979 (`xmlcatalog --shell` usershell()) | lib yes, **xmlcatalog tool not shipped** | — | inventory non-lib payload | **not_affected: code_not_present** (stays bundled: host Fedora 44 only has libxml2.so.2, webkit NEEDs .so.16) |
| libxslt | CVE-2025-7425 (key()/tree fragments) | yes | yes — libwebkit2gtk imports `xsltApplyStylesheet` (XSLTProcessor / `<?xml-stylesheet?>`) | reach.out | affected |
| gst-plugins-base1.0 | CVE-2026-125712 (RTCP SDES parser) | **no** — the parser lives in libgstrtp-1.0, which is **not** bundled; 0 `gst_rtcp`/`GstRTCP` strings in all 10 bundled gst libs; libwebkit2gtk imports no `gst_rtcp*` | — | reach.out, reach2.out | **not_affected: code_not_present** |
| jpeg-xl | CVE-2026-52584 (`DecodeImageAPNG`) | **no** — APNG decoder is `lib/extras/dec/apng.cc` (jxl_extras, STATIC EXCLUDE_FROM_ALL in libjxl-0.11.1/lib/jxl_extras.cmake), not libjxl.so; 0 `APNG`/`acTL` strings in libjxl.so.0.11 | — | reach2.out | **not_affected: code_not_present** |
| jpeg-xl | CVE-2026-82522 (64-bit box size truncated to 32-bit size_t) | yes | decoder reachable (webkit imports `JxlDecoderCreate`); truncation needs 32-bit size_t — we ship only x86_64/aarch64 | reach.out | affected (fail closed; not a code_not_present case) |
| libjpeg-turbo | CVE-2018-10126 (`jpeg_fdct_16x16`, IJG libjpeg 9 SmartScale) | **no** — `jpeg_fdct_16x16` occurs in **0** files of libjpeg-turbo-2.1.5 source (only `jfdctint.c` islow); 0 strings in libjpeg.so.8 | — | reach2.out | **not_affected: code_not_present** |
| gdk-pixbuf | ICO (16768), ICNS (18090), JPEG ICC (81893) | core lib yes; loader modules **not shipped**; Ubuntu build options not provable from the binary (io-ico.c/io-icns.c/io-jpeg.c exist in 2.44.5 source, may be built in) | uncertain | reach2.out | affected (fail closed) |
| cairo | CVE-2017-7475, 2018-18064, 2025-50422 | yes | yes (GTK/WebKit drawing, FT fonts) | — | affected |
| pixman | CVE-2023-37769 (combine_inner, float combiner) | yes (library code; the stress-test binary is only the trigger) | via cairo | — | affected |
| glib2.0 | CVE-2026-86469 (g_file_replace) | yes | yes | — | affected |

## 3. Slimming (product 34cb378 on fix/298-appimage-bundle)

**Change.** `packaging/ci/build-appimage.sh` EXCL += `libgssapi_krb5 libkrb5.so libk5crypto libkrb5support libsqlite3
libdbus-1 libmount libblkid` (host-provided; reasons in the EXCL comment and §2). `webkit-bundle-sbom.sh` already derives
the bundled set from the AppDir/EXCL, so the SBOM follows. Kept bundled (decided, §2): webkit2gtk/libsoup3/gtk/glib/gst,
libxml2 (.so.16 absent on Fedora 44), systemd (absent on non-systemd hosts), harfbuzz/fribidi (no alerts; mixing host
harfbuzz with bundled harfbuzz-icu is worse).

**Guard.** `scripts/supply-chain/appimage-excl-check.sh` (14 guarded sonames must stay in EXCL; `--appdir`: no EXCL'd
.so/symlink in the AppDir; `--sbom`: no EXCL'd soname in `opensidian:bundled-libs`). Runs in build-appimage.sh (fails the
build) and in release.yml before the osv gate; test `tests/appimage-excl-check.test.sh` (supply-chain gate self-tests);
red (8 FAIL) against origin/develop's recipe.

**Measured — release.yml dry run 45167393287 on 34cb378 (both AppImage jobs green), vs v0.9.6 release run 44731292971:**
same build-log line, same pinned osv-scanner v2.7.1 gate on `parts/webkit.cdx.json` (the bundle part).

- bundled libs (`[appdir] bundled N libs`): v0.9.6 **83** -> head **75** (x86_64 and aarch64; -8 = the EXCL additions;
  matches the box unpack of v0.9.6: 83 .so in usr/lib, §1)
- bundled source packages in the SBOM: **51 -> 47** per arch; removed exactly dbus 1.16.2-2ubuntu4, krb5 1.22.1-2ubuntu4.1,
  sqlite3 3.46.1-9ubuntu0.3, util-linux 2.41.3-3ubuntu2.2; nothing added, every other version identical
- osv gate on the bundle part: v0.9.6 **93** findings -> head **55** per arch (`summary: 0 with a fixed version, N without`);
  all 55 head ids are a subset of the v0.9.6 alert set (0 new)
- by severity (severity of each rule id from the v0.9.6 code-scanning alerts):
  - bundle part, before: 2 critical, 40 high, 49 medium, 2 low (93)
  - bundle part, after:  2 critical, 25 high, 27 medium, 1 low (55)  -> -15 high, -22 medium, -1 low
  - whole-SBOM alerts (incl. 13 Rust-crate alerts, unchanged by this work): 106 -> 68 per arch
    (2 critical, 27 high, 29 medium, 1 low, 9 warning)

Not reduced, by design: webkit2gtk 21 (2 critical) and libsoup3 18 — the engine itself; see §4 (2.54.x) and §5 (VEX).

**Reproduce** (data only on this VM: gh + jq, nothing downloaded is executed):
`bin/after-ci.sh 45167393287` -> `evidence/after-ci-45167393287.out` (and `evidence/ci-45167393287-{x86,a64}.gate.txt`).
v0.9.6 side: `baseline-v096.tsv` (from `gh api repos/jpzk/opensidian/code-scanning/alerts?tool_name=osv-scanner&state=open`)
and run 44731292971's log line `bundled 83 libs` / `93 without`.
Full rescan with one fresh DB on the box (pinned osv-scanner via osv-install.sh + the product's sbom-scan.sh, both sides):
`bin/box-after.sh <4 artifact URLs>` (item 6, same lease as the smoke).

Other red jobs in 45167393287 are not from #298: cargo audit/deny (new RUSTSEC-2026-0592 url, RUSTSEC-2026-0594, every
open branch, handled by fix/297-rust-cve-bump) and the macOS SBOM scans (same Rust crates).
## 4. WebKitGTK 2.54.x evaluation
Status: the CVE delta and the API check are done (desk work, data in `wsa/`). The box build cost is **pending**,
because the box controller has been unreachable since 12:08Z.

**Sources:** fetched 2026-11-23 into `wsa/`:
- webkitgtk.org/security.html
- WSA-2025-0008 … WSA-2026-0007
- the 2.54 highlights and 2.54.0 release notes

How the map was built: `wsa/wsa-map.tsv` is (advisory, CVE, "before X") parsed from each advisory's "Versions
affected" lines. `wsa/delta.tsv` joins it against the 21 webkit2gtk alerts per arch in `baseline-v096.tsv`.
Advisory text is treated as data.

**CVE delta for the 21 alerted webkit2gtk CVEs (per arch):**
- **19 are fixed upstream in 2.54.0** (WSA-2026-0006, 29 Sep 2026, "before 2.54.0"; the upstream wording is
  "before 2.54.0 or earlier"). That covers:
  - both criticals, CVE-2024-43091 (SkEmbossMaskFilter) and CVE-2025-26416 (SkBmpStandardCodec, already
    VEX not_affected)
  - 12 of 13 highs: 2024-43767/43768/8193/8198, 2025-0436/54627, 2026-10019/3536/3538/3909/3931/43670
  - 5 of 6 mediums: 2025-0444/48622, 2026-10018/15109/5283
  - They are mostly Skia CVEs, either Chrome-origin or Android-origin. In 2.54.0 the Skia snapshot was refreshed.
- **2 are not in any WSA:**
  - CVE-2026-78376 (high, UAF). The Ubuntu record's only fix ref is WebKit PR #72142 = commit 9fa946d9, titled
    "Clean up jsObjectCall style". That commit is on `main` and is not an ancestor of `webkitgtk-2.54.0` or
    `webkitgtk-2.54.3` (GitHub compare: diverged). So the upstream fix status is unknown, and we treat it as
    affected on 2.54.x too.
  - CVE-2025-66286 (medium, WebPage::send-request API design flaw). No upstream fix reference. Affected on both.
- **Expected effect on the bundle gate:** webkit2gtk goes from 21 to 2 alerts/arch, i.e. 2c+13h+6m → 0c+1h+1m.
  This is not measured. The SBOM would carry a non-Ubuntu purl (generic/github), so osv matches by CVE alias, not
  by the Ubuntu feed. That has to be re-measured on a real build.

**What the alert count hides:**
- Upstream says **2.52.6 is affected by 234 CVEs fixed in 2.54.0 and 24 fixed in 2.54.3** (WSA-2026-0006/-0007).
- The Ubuntu feed only alerts 21 of them against 2.52.6-0ubuntu0.26.04.2.
- So the v0.9.6 SBOM's 21 alerts understate the real WebKit exposure by about 10×. The 2.52 branch gets no more
  upstream releases after 2.52.6 (Aug 2026), and Ubuntu 26.04 has nothing newer.

**API / behaviour compatibility (release notes):**
- 2.54 still ships the GTK3 `webkit2gtk-4.1` API (libsoup3) that Opensidian links. No API removal affects us.
- Changes that affect us:
  - Cairo 2D rendering removed, and the compositor moved to Skia. That is more Skia code reachable, not less.
  - The GTK3 `HARDWARE_ACCELERATION_POLICY_ON_DEMAND` is deprecated, and hardware acceleration is always used.
    That is a portability risk for the AppImage on weak or odd GPU hosts, and needs a distro-matrix run.

**Build inputs (pinned, provenance):**
- webkitgtk-2.54.3.tar.xz: 50 038 244 B, sha256 `2779c1b46f88762e8400650271ba14452dc4e30b37059981552d8b562b7d0862`
  (webkitgtk.org `.sums`, Last-Modified 2026-11-18)
- For comparison, 2.52.6 is 65 746 000 B, sha `179a2ea3…116fc`.

**Build cost:** still TODO on the box, which is unreachable. Planned measurement, under the box-wide build lock with
CARGO_BUILD_JOBS=8 equivalent `-j8`:
- `cmake -GNinja -DPORT=GTK -DUSE_GTK4=OFF -DENABLE_DOCUMENTATION=OFF -DENABLE_INTROSPECTION=OFF` in an ubuntu:26.04 build
  container
- record wall time, peak RSS, installed size of libwebkit2gtk-4.1.so + libjavascriptcoregtk, and whether two builds
  are bit-identical (reproducibility)
- The box lease used so far has no podman/gcc (item 5), so this may have to be a CI job instead.

**Interim assessment:**
- The CVE delta is clearly better: 19 of 21 alerts, both criticals, and about 258 upstream-known CVEs.
- The cost is a from-source WebKit build per arch (hours), plus maintaining our own security backports. Today Ubuntu
  does that work for us, but Ubuntu has stopped at 2.52.6.
## 5. VEX
8 `not_affected` entries added to `supply-chain/vex.cdx.json` (product, ids = the `UBUNTU-CVE-*` rule ids osv reports
for the Ubuntu-sourced bundle; no CVE alias references, so they only cover the Ubuntu-ecosystem findings). Each one
is **code_not_present**, the evidence is committed under `packaging/appimage/inventory/`, and every entry expires
2027-02-23 (re-triage):
- UBUNTU-CVE-2025-26416 (webkit2gtk, critical): Skia `SkBmpStandardCodec.cpp` is not in WebKit's Skia CMake list,
  and the bundled lib has 0 strings for it.
- UBUNTU-CVE-2025-48622 (webkit2gtk, medium): there is no Adobe DNG SDK in the WebKitGTK tree, and 0 `dng_` strings.
- UBUNTU-CVE-2026-125712 (gst-plugins-base, high): the RTCP parser lives in libgstrtp-1.0, which is not bundled on
  either arch; 0 `gst_rtcp` strings in the 10 bundled gst libs.
- UBUNTU-CVE-2026-52584 (jpeg-xl, high): `DecodeImageAPNG` is in jxl_extras (STATIC EXCLUDE_FROM_ALL), not in
  libjxl.so; 0 strings.
- UBUNTU-CVE-2018-10126 (libjpeg-turbo, medium): `jpeg_fdct_16x16` appears in 0 libjpeg-turbo-2.1.5 source files.
- UBUNTU-CVE-2026-11979 (libxml2, high): the bug is in the `xmlcatalog` tool, which is not shipped (only libxml2.so.16).
- UBUNTU-CVE-2026-121876 (systemd, high) and UBUNTU-CVE-2026-40228 (systemd, low): the gpt-auto-generator and
  journald binaries are not shipped; only the libsystemd/libudev client libraries are.

**Fail closed:** the four "file not shipped" premises (xmlcatalog, systemd-journald, systemd-gpt-auto-generator,
libgstrtp-1.0) are listed in `scripts/supply-chain/appimage-excl-check.sh` `VEX_ABSENT_RE`. Both `--appdir` (which
fails the AppImage build) and `--sbom` (release osv gate) go red if any of them is bundled, and
`tests/appimage-excl-check.test.sh` covers each one, a dangling symlink, near-miss names and the SBOM case. The four
source-level premises (Skia CMake list, DNG, jxl_extras, libjpeg-turbo source) must be re-checked on any bump of
those packages; the entry expiry forces that.

**Still `affected`:** everything else, deliberately. That includes webkit2gtk CVE-2024-43091 (critical), all
libsoup3 alerts (server paths are not claimed: webkit imports `soup_server_new`, and opensidian imports the
automation API), harfbuzz/cairo/pixman/glib/gdk-pixbuf, and the rest of the gst/jxl/libxml2/libxslt/systemd alerts,
because their code is present and reachable or the evidence is not conclusive.

Effect per arch on the bundle osv gate: 55 after slimming → 47 after VEX
(removed 1 critical, 4 high, 2 medium, 1 low → 1c / 21h / 25m / 0l).
Verified locally: `vex-check.sh` PASS (19 entries), `vex-check.test.sh` ALL PASS, `vex-osv-config.sh` emits the
8 UBUNTU ids, `appimage-excl-check.test.sh` all ok, the guard is OK on both slimmed SBOMs, pin-check 0 findings, and
osv-json / sbom-scan / webkit-bundle-sbom tests ALL PASS.
## 6. Recommendation for 0.9.7

**Ship in 0.9.7 (this PR):**
1. The slimming (§3): 8 host-provided sonames (krb5/GSSAPI, sqlite3, dbus, libmount/libblkid) leave the bundle;
   83 → 75 libs, bundle osv gate 93 → 55 per arch. Guarded by `appimage-excl-check.sh` + its test.
2. The 8 VEX `code_not_present` entries (§5), each with a premise that the guard re-checks (`VEX_ABSENT_RE`): if the
   file comes back, the build fails, so the not_affected claim can't go stale without anyone noticing. Gate 55 → 47 per arch.

**Conditions before merge (fail closed):**
- The harness T1 smoke (brand insdate top5func title fileops vaultbleed) has to be green on the box for the slimmed
  AppImage, with the same reds as develop (ledger item 6). It is **not done**: the box controller has been unreachable
  since about 12:00Z on 2026-11-23. Host-provided krb5/sqlite3/dbus/libmount mean the app now needs those libs on
  the host. They are in every glibc desktop base (AppImage excludelist practice), but that is only a claim until the
  smoke run proves it.
- CI on head: the cargo audit/deny reds come from RUSTSEC-2026-0592/-0594 and are fixed by #303. Rebase once #303
  lands, then CI has to be green, including supply-chain/pin-check.

**Do NOT ship in 0.9.7: our own WebKitGTK 2.54.3 build.** Reasons:
- It would fix 19 of the 21 alerted webkit CVEs plus about 258 that the Ubuntu feed doesn't flag. That is real value
  (§4).
- But build cost and reproducibility on the box are not measured. We would be taking on a 50 MB C++ engine
  build in our own supply chain, plus mandatory GPU acceleration on weak hosts. That cost is not known yet.
- Target 0.9.8 instead. Steps: (a) measure the build on the box (time, size, two builds compared for byte-identical
  output); (b) run the distro matrix smoke with HW accel forced; (c) only then switch the bundle's webkit2gtk to the
  pinned tarball (sha256 2779c1b4…0862).
- Until then, the webkit2gtk and libsoup3 alerts stay **affected**. No VEX for them, because the code is present
  and reachable (§2).
