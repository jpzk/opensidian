# Flatpak reproducibility: what differs, what can be pinned, what to publish

Scope: `packaging/flatpak/dev.koto.opensidian.yml` + `packaging/flatpak/build.sh`, release asset
`opensidian-0.9.2-x86_64.flatpak` (github.com/jpzk/opensidian releases, tag v0.9.2).

Every claim carries a marker:

- VERIFIED = read in the pinned source, or run here with the output recorded below.
- UNVERIFIED = plausible, from a primary source or inference, not run or not read end-to-end.

Pinned sources (all links below use these tags; tag -> commit as cloned on 2026-11-09):

- ostree `v2025.7` -> 134d144cb52ff018cfedfb936b661189bccc76e6 (also the libostree that ran the experiments; Fedora 44 build ostree-2025.7-2.fc44)
- flatpak `1.17.6` -> 9b21874f1a175a9b7c79175a221fa043e202ca73 (the version that built v0.9.2: commit body says "Built with: Flatpak 1.17.6")
- flatpak-builder `1.4.7` -> a7a5aae277c99b7df068f413ab818854ed5c7af5 (the Fedora 44 base version that pairs with flatpak 1.17.6; which flatpak-builder built v0.9.2 is not recorded in the bundle: UNVERIFIED)
- flathub-infra/vorarbeiter 7be8485a1354ffb90c94782a9be8bf437b13ecb2, flathub-infra/flathub-repro-checker 94f645291720f858ebf272c62a43282464b01487

Spot checks against the newest tags (flatpak-builder 1.4.12, ostree v2026.6, flatpak 1.18.4) show the same code for the
three parts that matter most here: the SOURCE_DATE_EPOCH default, the delta superblock timestamp, and the build-bundle options. VERIFIED (grep of raw files, 2026-11-09).

## Ground truth: what is inside the v0.9.2 bundle

Measured on the release asset (VERIFIED, ran it, 2026-11-09):

- sha256 of the file: `b99e4770156e253206218fbb998b01e06574920d4069736a694819c0e24641e6` (same as SHA256SUMS and the GitHub asset digest)
- size: 8567224 bytes
- ostree commit: `9786eba571fd28ea16c49a27bfa1800e78a454fcb93956ed66afec1af8e1027c`
- root dirtree: `657186ce21f8544c7f7b5fa8018e860c5232e603272d20bc30437e9ab0c879ab`
- root dirmeta: `446a0ef11b7cc167f3b603e585c7eeeeb675faa412d5ec73f62988eb0b6c5488`
- **content digest: `e78208b343e3019a637ec21cd4cfd651cfc2041015b3f908ce025532a94ca265`**
- commit timestamp: 1793390843 = 2026-10-30 20:07:23Z
- delta superblock timestamp: 1793390860 = 2026-10-30 20:07:40Z, at file offset 8564320 (big-endian u64); the commit's own timestamp copy sits at offset 8565136
- `files/manifest.json` inside the tree: `"source-date-epoch" : 1793387998` (2026-10-30 19:19:58Z, i.e. the manifest file's mtime on the build box), `"runtime-commit" : "2bb946ea4ebb050f6a32f90e979feb975b08280d1ff1aac881dec3d1361ad84d"`, `"sdk-commit" : "faf37bdc962236ef8e61fd0b289fd311f7961cc5a7b4bc11a6e29506d7874003"`; no commit is recorded for the rust-stable extension
- `files/share/app-info/xmls/dev.koto.opensidian.xml.gz` gzip header mtime field = 0
- `files/lib/debug` is empty (the Debug extension is a separate ref that is not in the bundle)

How it was measured: see Q4 (the same commands are in `packaging/flatpak/content-digest.sh -v`).

## Q1. Which fields make two bundles of identical content differ

A `.flatpak` file is an ostree static delta "superblock" from the empty commit to the app commit, with every part inlined and
some extra header keys. flatpak `build_bundle()` builds the header dict and calls `ostree_repo_static_delta_generate()` with
`inline-parts=TRUE` (https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-bundle.c#L221-L340). VERIFIED (read).
The superblock layout is `(a{sv} metadata, t timestamp, ay from, ay to, commit, ay, parts, fallbacks)`
(https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-repo-static-delta-private.h#L73-L104). VERIFIED (read).

So the bytes of a bundle are three layers: (1) bundle header, (2) delta superblock framing + compressed parts, (3) the commit
object, which in turn points at (4) the content tree (VERIFIED: read the two links above, and `content-digest.sh` walks exactly these layers). Fields per layer:

### Layer 1: bundle header (written by flatpak `build_bundle()`)

- `flatpak` magic `0xe5890001` and `ref` (app/dev.koto.opensidian/x86_64/master): constant for us. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-bundle.c#L251-L254 VERIFIED (read).
- `metadata`: a copy of the tree's `/metadata` file. Only changes if content changes. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-bundle.c#L274 VERIFIED (read).
- `appdata` + `icon-64` / `icon-128`: copies of `files/share/app-info/xmls/*.xml.gz` and the app-info PNGs. Derived from content. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-bundle.c#L172-L205 and #L280-L290 VERIFIED (read).
- `origin` (`--repo-url`) and `runtime-repo` (`--runtime-repo`): command-line strings; we pass the Flathub flatpakrepo URL. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-bundle.c#L293-L297 VERIFIED (read).
- `collection-id`: the export repo's collection id, empty for us. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-bundle.c#L299-L301 VERIFIED (read).
- `gpg-keys` (`--gpg-keys`): optional, not used by us. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-bundle.c#L303-L317 VERIFIED (read).
- The output path is passed as a delta param `filename` but is NOT embedded: two runs writing to different paths differed in only the 8 timestamp bytes (experiment below). VERIFIED (ran).

### Layer 2: delta superblock and parts (written by ostree `_ostree_repo_static_delta_...` compilation)

- **superblock timestamp = wall clock at bundle time**, `g_date_time_new_now_utc()`, no override, no SOURCE_DATE_EPOCH. https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-repo-static-delta-compilation.c#L1480-L1503 VERIFIED (read, and ran: see experiment). Same code in v2026.6. VERIFIED (grep).
- compression: flatpak hardcodes `compression='x'` (xz) https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-bundle.c#L319-L325 and ostree hardcodes the xz encoder with preset 8 + CRC64 https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-lzma-compressor.c#L170 VERIFIED (read). Output bytes therefore depend on the liblzma version on the bundling machine. UNVERIFIED (not tested across liblzma versions).
- object order inside parts: iteration order of GHashTables of reachable objects (`generate_delta_lowlatency`) https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-repo-static-delta-compilation.c#L1050-L1100 VERIFIED (read). With the same ostree + same content it came out identical twice (experiment). Across ostree/GLib versions: UNVERIFIED.
- part split / fallback: flatpak passes `min-fallback-size=0`, `bsdiff-enabled=FALSE`, `inline-parts=TRUE`, `include-detached=TRUE` (same lines). Constant. VERIFIED (read).
- detached metadata (signatures) is included if present (`include-detached`); we do not GPG-sign the commit. VERIFIED (read flags); absence of commitmeta in v0.9.2: VERIFIED (only one .commit and no .commitmeta object after apply).

Experiment (VERIFIED, ran 2026-11-09, ostree 2025.7): from the commit imported out of the v0.9.2 bundle I generated the same
from-scratch delta twice, 16 s apart, to two different file names:

    ostree static-delta generate --repo=r --empty --to=9786eba5... --inline \
      --min-fallback-size=0 --disable-bsdiff --filename=$PWD/d1.delta   # then d2.delta
    sh bytediff.sh d1.delta d2.delta
    8546439 6f 7f

(`bytediff.sh` is a `cmp -l` stand-in, diffutils is not installed here: `od -An -v -tx1 -w1` each file, `paste`, and `awk '$1!=$2{print NR-1, $1, $2}'`.)

Exactly ONE byte differed (8549336-byte files): offset 8546432..8546439 decodes as big-endian 0x6af2036f = 17:31:27Z vs
0x6af2037f = 17:31:43Z, the superblock timestamp. Everything else, including xz output and object order, was identical. VERIFIED (ran).

### Layer 3: the commit object (written by flatpak `build-export` via `ostree_repo_write_commit*`)

Commit format `(a{sv} metadata, ay parent, a(say) related, s subject, s body, t timestamp, ay root-dirtree, ay root-dirmeta)`
https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-core.h#L140-L154 VERIFIED (read). Any field change changes
the commit checksum and therefore the bundle bytes.

- **timestamp**: `build-export --timestamp` uses `ostree_repo_write_commit_with_time`; without it `ostree_repo_write_commit`, which takes `SOURCE_DATE_EPOCH` from the environment if set, else wall clock. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-export.c#L1088-L1110 and https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-repo-commit.c#L2882-L2913 VERIFIED (read). v0.9.2 used wall clock (20:07:23Z, not the 19:19:58Z source-date-epoch). VERIFIED (ran `ostree show`).
- **parent**: build-export resolves the existing ref in the target repo and uses it as parent. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-export.c#L950-L962 VERIFIED (read). `build.sh` keeps `dist/flatpak/repo` between runs, so a second build gets the first build's commit as parent. v0.9.2 had an empty parent (`@ay []` in `ostree show --raw`). VERIFIED (ran); the parent-on-rebuild effect itself: UNVERIFIED (not run with flatpak).
- **subject / body**: default subject "Export <id>", default body includes "Built with: Flatpak <version>" (PACKAGE_STRING), overridable with `--subject` / `--body` (flatpak-builder passes its own `--subject`/`--body` through). https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-export.c#L929-L944 and https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-main.c#L122-L123 VERIFIED (read). So the flatpak VERSION of the builder is baked into every bundle.
- **metadata dict**: `xa.ref`, `ostree.ref-binding`, `ostree.collection-binding`, `xa.metadata` (copy of /metadata), `xa.installed-size`, `xa.download-size`, optional `xa.token-type`, `xa.subsets`, end-of-life keys. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-export.c#L1050-L1085 VERIFIED (read). `xa.download-size` is summed from the export repo's compressed (archive-z2, zlib) object sizes by `flatpak_repo_collect_sizes` https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-export.c#L1047 so it depends on zlib output, not only on content. VERIFIED (read); zlib-version sensitivity UNVERIFIED.
- ostree may add `ostree.sizes` (`add_size_index_to_metadata`) when the repo config enables it https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-repo-commit.c#L2915-L2927 VERIFIED (read); not present in v0.9.2. VERIFIED (ran).

### Layer 4: the content tree (what `content-digest.sh` hashes)

ostree stores per file only uid, gid, mode, rdev, symlink target, xattrs and bytes (`(uuuusa(ayay))`), per directory uid,
gid, mode, xattrs; **no mtimes** (checkouts use OSTREE_TIMESTAMP=0). https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-core-private.h#L50-L62 and https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-core.h#L106-L139 and #L184-L190 VERIFIED (read).
flatpak build-export commits with `SKIP_XATTRS` and canonicalised permissions https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-export.c#L1004 VERIFIED (read). So only real content can move the digest. What moves real content in our build:

- **`files/manifest.json` -> `source-date-epoch`**: flatpak-builder sets it to the manifest file's mtime unless `--override-source-date-epoch` is given https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-main.c#L769-L772 and copies it into the manifest it serialises into the app https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-manifest.c#L1693 VERIFIED (read; and the value in v0.9.2 is a checkout mtime, not a commit time: VERIFIED ran). A fresh `git clone` gives a new mtime -> new digest. Flathub confirms the same problem: https://github.com/flathub/flathub/issues/2617 (closed 2025-05-26 with the fixed-epoch fix below).
- **`SOURCE_DATE_EPOCH` inside the sandbox**: exported to every build command from the same value https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-context.c#L1142-L1151 VERIFIED (read). Anything that embeds it (cmake, appstream, rustc build scripts) follows it.
- **`files/manifest.json` -> `runtime-commit` / `sdk-commit`**: filled from `flatpak info --show-commit` of whatever runtime/SDK is installed on the build host at build time, i.e. an update of org.gnome.Sdk//49 changes the digest even if nothing else does. https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-manifest.c#L1704-L1734 VERIFIED (read).
- **`files/share/app-info/*`**: output of `appstreamcli compose` run by flatpak-builder https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-manifest.c#L2425-L2450 and #L3056-L3105 VERIFIED (read). Our xml.gz has gzip mtime 0 VERIFIED (ran od). The PNGs are re-rendered by appstream; byte-stability across appstream versions UNVERIFIED. Flathub's own checker deletes `manifest.json` and `share/app-info` before diffing, i.e. Flathub treats both as known-nondeterministic (see Q5). VERIFIED (read).
- **debuginfo / build-id**: by default flatpak-builder post-processes every module with DEBUGINFO (+ compression) unless `strip`/`no-debuginfo` is set https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-module.c#L2252-L2262 : `debugedit -l` collects/rewrites source paths and `eu-strip --remove-comment --reloc-debug-sections -f` splits debug info into `/lib/debug` (the separate Debug ref) https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-post-process.c#L382-L530 and https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-utils.c#L548-L590 VERIFIED (read). The export excludes `/lib/debug/*` except `/lib/debug/app` https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-main.c#L1081-L1093 VERIFIED (read). Flathub's reproducibility checker has reported "File has been modified after NT_GNU_BUILD_ID has been applied" diffs in stripped libraries (https://github.com/flathub/flathub/issues/8270). Whether our rust binary's GNU build-id is stable across two builds: UNVERIFIED (needs a double build).
- **rustc output** (`files/bin/opensidian`, 22.6 MB): depends on the exact rust-stable extension version, the absolute build path (fixed at `/run/build/opensidian` inside the sandbox, VERIFIED in our manifest), `CARGO_HOME` (fixed) and codegen determinism. UNVERIFIED.
- **brotli (cmake)**: from a sha256-pinned tarball; compiler from the SDK commit. UNVERIFIED for bit-stability.
- **file mtimes**: zeroed by flatpak-builder on every cache commit https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-cache.c#L598-L602 and anyway not stored by ostree. Cannot move the digest. VERIFIED (read).

## Q2. What can be pinned with existing flags

- `flatpak build-export --timestamp=TIMESTAMP`: exists in 1.17.6, parsed with `parse_datetime`, sets the commit timestamp (and the appstream branch timestamp when `--update-appstream`). https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-export.c#L76 and #L1088-L1110 VERIFIED (read). Not reachable through flatpak-builder: its `do_export()` builds the build-export argv itself and has no `--timestamp` pass-through https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-main.c#L200-L268 VERIFIED (read). Two ways round that:
  - run flatpak-builder WITHOUT `--repo` (just build into `dist/flatpak/build`) and call `flatpak build-export --timestamp=@N ...` ourselves. UNVERIFIED (not run).
  - export `SOURCE_DATE_EPOCH=N` in flatpak-builder's own environment: flatpak-builder spawns `flatpak build-export` with a GSubprocessLauncher that inherits the environment https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-flatpak-utils.c#L551-L570 and `ostree_repo_write_commit` then uses it. VERIFIED (read the full chain); UNVERIFIED (not run).
- `flatpak build-export --subject / --body`: replace the default body that embeds the flatpak version. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-export.c#L60-L61 VERIFIED (read). flatpak-builder has the same two flags and passes them through https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-main.c#L122-L123 and #L228-L232 VERIFIED (read).
- parent: no flag; avoid it by exporting into a fresh (deleted) repo each release build. VERIFIED (read: parent comes only from the existing ref, build-export.c#L950-L962).
- `flatpak-builder --override-source-date-epoch=N`: exists in 1.4.7 (and 1.4.12), int64, replaces the manifest-mtime default; flows into the sandbox env and into `files/manifest.json`. https://github.com/flatpak/flatpak-builder/blob/1.4.7/src/builder-main.c#L147 and #L769-L772 VERIFIED (read). It does NOT set the commit timestamp (that is build-export's process). VERIFIED (read).
- `flatpak build-bundle`: options in 1.17.6 are `--runtime`, `--arch`, `--repo-url`, `--runtime-repo`, `--gpg-keys`, `--gpg-sign`, `--gpg-homedir`, `--from-commit`, `--oci`, `--oci-layer-compress`. https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-build-bundle.c#L58-L72 VERIFIED (read). **None controls the superblock timestamp or the compressor**; same in 1.18.4. VERIFIED (grep). Byte-identical bundles therefore need either libfaketime around `flatpak build-bundle` (UNVERIFIED) or rewriting the 8 timestamp bytes after the fact (the superblock itself is not checksummed, only the objects it carries; UNVERIFIED that `flatpak install` accepts a patched file, though ostree's apply path does not hash the superblock: https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-repo-static-delta-core.c#L395-L615 read, UNVERIFIED end-to-end).
- `ostree static-delta generate`: no timestamp option either (`--help` lists none). VERIFIED (ran).
- SOURCE_DATE_EPOCH support summary: ostree commit writer YES (env) VERIFIED (read); ostree delta superblock NO VERIFIED (read+ran); flatpak build-export YES via ostree, or `--timestamp` VERIFIED (read); flatpak build-bundle NO VERIFIED (read); flatpak-builder sets it for the sandbox from manifest mtime or the override flag VERIFIED (read).

## Q3. Pinning the inputs

- **SDK and runtime to an exact commit.** The commits that built v0.9.2 are recorded in the bundle (`runtime-commit` 2bb946ea..., `sdk-commit` faf37bdc..., see ground truth). Re-create them with
  `flatpak update --commit=<sha> org.gnome.Sdk//49` (and the same for org.gnome.Platform//49), then `flatpak mask org.gnome.Sdk//49` so nothing moves them. `--commit` exists in 1.17.6 https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-update.c#L57 and `mask` https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-mask.c#L40-L70 VERIFIED (read). This is exactly what Flathub's checker does (`update --commit=` for each pinned ref) https://github.com/flathub-infra/flathub-repro-checker/blob/94f645291720f858ebf272c62a43282464b01487/flathub_repro_checker/flatpak.py#L224-L237 VERIFIED (read). Commit history is browsable with `flatpak remote-info --log flathub org.gnome.Sdk//49` (`--log`, `--commit` options: https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-remote-info.c#L55-L60 VERIFIED read). How long Flathub keeps old commits fetchable: UNVERIFIED.
- The manifest keys `runtime-commit` / `sdk-commit` are NOT input pins: flatpak-builder overwrites them from the installed runtime at build time (builder-manifest.c#L1704-L1734 above). VERIFIED (read).
- **rust-stable sdk extension.** The bundle does not record its commit (only `"sdk-extensions": ["org.freedesktop.Sdk.Extension.rust-stable"]`). VERIFIED (ran). Pin it the same way: record `flatpak info --show-commit org.freedesktop.Sdk.Extension.rust-stable//25.08` at release time, `flatpak update --commit=...` + `mask` on rebuild, and write the rustc version (`/usr/lib/sdk/rust-stable/bin/rustc -V`) into the release notes. `--show-commit`: https://github.com/flatpak/flatpak/blob/1.17.6/app/flatpak-builtins-info.c#L60 VERIFIED (read). Not done for v0.9.2, so the v0.9.2 rust toolchain is currently only identifiable by branch 25.08: UNVERIFIED which commit.
- **flatpak-builder sources.** brotli is an `archive` with `sha256` (flatpak-builder refuses a mismatch) and every crate in `cargo-sources.json` is a sha256-pinned crates.io archive generated from `Cargo.lock`. VERIFIED (read our manifest + manifest.json in the bundle). The weak spot is the module's own source: `type: dir, path: ../..` has no checksum, and `manifest.json` records only the path. VERIFIED (ran). For releases switch to `type: git` with `tag:` + full `commit:` (the manifest comment already proposes this), or record the git commit in the release notes and build from a clean checkout of it.
- **host tools.** flatpak and flatpak-builder versions shape the commit body and the bundle (xz via liblzma, zlib download-size). Pin them (the box cloud-init already pins flatpak/flatpak-builder by version for the Ubuntu box; v0.9.2 was built by flatpak 1.17.6). VERIFIED (read cloud-init + commit body).
- **SOURCE_DATE_EPOCH.** Use the release tag's commit time, `git log -1 --format=%ct <tag>`, for `--override-source-date-epoch` and for `SOURCE_DATE_EPOCH` in the environment of build-export. Flathub instead uses the constant 1321009871 (2011-11-11 11:11:11Z, borrowed from BuildStream) for every build https://github.com/flathub-infra/vorarbeiter/blob/7be8485a1354ffb90c94782a9be8bf437b13ecb2/justfile#L223-L234 VERIFIED (read), reasoning in https://github.com/flathub/flathub/issues/2617 (admin comment 2025-05-26). A constant is simpler and survives tag rewrites; either works.

## Q4. Content-level verification with stock tools

The ostree "content checksum" is defined as `SHA256(root dirtree checksum || root dirmeta checksum)` over the 32-byte binary
values (`ostree_commit_get_content_checksum`) https://github.com/ostreedev/ostree/blob/v2025.7/src/libostree/ostree-core.c#L2419-L2463 VERIFIED (read). It
covers every file's bytes, uid/gid/mode/xattrs, symlinks and the directory layout, and NOTHING from layers 1-3: not the commit
timestamp, parent, subject/body, xa.* metadata, nor the bundle header, superblock timestamp or compression. VERIFIED (read the
format definitions in Q1 layer 4).

With stock ostree only (no root, no flatpak, no network):

    ostree init --repo=r --mode=bare-user-only
    ostree static-delta apply-offline --repo=r opensidian-0.9.2-x86_64.flatpak
    ostree show --repo=r <the one commit in r/objects/*/*.commit>   # prints ContentChecksum:

`apply-offline` passes `skip_validation=FALSE`, so every object is checked against its checksum while it is written
https://github.com/ostreedev/ostree/blob/v2025.7/src/ostree/ot-builtin-static-delta.c#L563-L575 VERIFIED (read).

`packaging/flatpak/content-digest.sh` wraps this, and additionally recomputes the digest itself from
`ostree ls -C -d <commit> /` (dirtree + dirmeta hex -> binary -> sha256sum) and fails if the two disagree. Run on the v0.9.2
asset, twice in a row (VERIFIED, ran 2026-11-09, ostree 2025.7 from Fedora 44 `--repo=fedora` rpms, see "Tooling"):

    $ sha256sum opensidian-0.9.2-x86_64.flatpak
    b99e4770156e253206218fbb998b01e06574920d4069736a694819c0e24641e6  opensidian-0.9.2-x86_64.flatpak
    $ sh packaging/flatpak/content-digest.sh opensidian-0.9.2-x86_64.flatpak
    e78208b343e3019a637ec21cd4cfd651cfc2041015b3f908ce025532a94ca265
    $ sh packaging/flatpak/content-digest.sh opensidian-0.9.2-x86_64.flatpak
    e78208b343e3019a637ec21cd4cfd651cfc2041015b3f908ce025532a94ca265

Independence from timestamps: the digest is computed from objects that contain no time field (Q1 layer 4), so a rebuilt
bundle with a different commit time, parent, body or superblock time has the same digest iff the tree is identical. VERIFIED
(read). Practical caveat: our tree is NOT timestamp-free today, because `files/manifest.json` contains `source-date-epoch` =
manifest mtime (Q1). Pin it (Q2/Q3) or the digest changes on every fresh checkout. VERIFIED (read + ran).

An installed copy can be checked the same way without the bundle, because flatpak keeps the commit:
`ostree show --repo=/var/lib/flatpak/repo "$(flatpak info --show-commit dev.koto.opensidian)" | grep ContentChecksum`
(use `~/.local/share/flatpak/repo` for `--user` installs). UNVERIFIED (not run; no flatpak installation here).

## Q5. State of the art

- flatpak-builder has had an open "Reproducible build support" issue since 2017, no implementation: https://github.com/flatpak/flatpak-builder/issues/251 (open as of 2026-11-09). VERIFIED (read via API).
- Flathub pins `--override-source-date-epoch 1321009871` in its production build recipe https://github.com/flathub-infra/vorarbeiter/blob/7be8485a1354ffb90c94782a9be8bf437b13ecb2/justfile#L223-L234 VERIFIED (read).
- Flathub runs a rebuild-and-diffoscope checker, `flathub-repro-checker`, from a `reprocheck` workflow https://github.com/flathub-infra/vorarbeiter/blob/7be8485a1354ffb90c94782a9be8bf437b13ecb2/.github/workflows/reprocheck.yml VERIFIED (read). The checker reads `/app/manifest.json` of the published build, pins runtime/SDK with `flatpak update --commit=`, rebuilds with the same fixed epoch, then removes `manifest.json` and `share/app-info` from both trees and runs `diffoscope --exclude-directory-metadata=recursive` on the checked-out files https://github.com/flathub-infra/flathub-repro-checker/blob/94f645291720f858ebf272c62a43282464b01487/flathub_repro_checker/flatpak.py#L224-L237 , #L360-L390 and https://github.com/flathub-infra/flathub-repro-checker/blob/94f645291720f858ebf272c62a43282464b01487/flathub_repro_checker/repro.py#L20-L60 , #L145-L170 VERIFIED (read). So Flathub's notion of "reproducible" is **file-tree level minus manifest.json and app-info**, not bundle bytes and not even ostree commit or content checksum.
- Results so far are mixed: the 2025-07 status report lists several unreproducible apps https://github.com/flathub-infra/reproducibility-tests/issues/1 ; a 2026 report cites build-id/debugedit diffs in a cmake-built library https://github.com/flathub/flathub/issues/8270 . VERIFIED (read). Per that issue, status is shown on builds.flathub.org. UNVERIFIED (site not checked).
- Nobody found doing byte-identical `.flatpak` bundles; given the unoverridable superblock timestamp (Q1 layer 2) that is expected. VERIFIED that no flag exists (Q2); "nobody" = UNVERIFIED (absence of evidence from issue search only).

## Q6. Recommendation

**Recommendation: option B (publish the content digest), with the three cheap pins that make it rebuildable.**

Options considered:

- A, pin-wrapper for a byte-identical bundle. Needs everything in B plus a fixed commit timestamp, fixed subject/body, fresh repo, pinned flatpak + liblzma + zlib versions on the bundling host, and libfaketime or an 8-byte post-patch of the superblock time. Pros: plain `sha256sum` verifies. Cons: fragile (any host package update breaks it), the post-patch is an unsupported hack, effort ~1-2 days plus ongoing breakage. Not worth it.
- B, content digest. The digest is immune to layers 1-3 by construction (Q4), works with stock `ostree`, and is what a user can also check on an installed app. Cons: needs `ostree` on the verifier; the tree itself must still be deterministic for an independent REBUILD to match (manifest.json epoch + runtime commits are fixed by the pins; rustc/debugedit determinism is UNVERIFIED until a double build).
- C, Flathub-style tree diff (diffoscope minus manifest.json/app-info). Good diagnostic for WHY two builds differ, but no single number to publish and needs diffoscope (python). Use it only as a debugging tool, not as the published check.

What to change (effort ~0.5 day, plus ~1-2 h box time to prove it):

1. `build.sh`: `--override-source-date-epoch "$SDE"` with `SDE=$(git log -1 --format=%ct HEAD)` (or Flathub's 1321009871), and `SOURCE_DATE_EPOCH="$SDE"` exported for the flatpak-builder process; delete `dist/flatpak/repo` before the release export (no parent); `--subject`/`--body` fixed strings. ~1 h.
2. Release step: record `flatpak info --show-commit` for org.gnome.Sdk//49, org.gnome.Platform//49 and org.freedesktop.Sdk.Extension.rust-stable//25.08 plus the git commit into a `flatpak-inputs.txt` release asset; rebuilders `flatpak update --commit=... && flatpak mask ...`. ~1 h.
3. Release step: `sh packaging/flatpak/content-digest.sh opensidian-X-x86_64.flatpak` -> add a line `<digest>  opensidian-X-x86_64.flatpak.ostree-content` to a `CONTENTSUMS` file next to SHA256SUMS (same GPG signing as SHA256SUMS). ~1 h.
4. Prove it: build the same commit twice on the box (fresh clones, different times) and compare digests; if they differ, `ostree diff` the two commits to name the file (expected suspects: rust binary build-id, app-info PNGs). ~half day including fixes.

What a user runs to verify a downloaded bundle (needs only `ostree`, e.g. `sudo dnf install ostree` / `sudo apt install ostree`):

    sha256sum -c --ignore-missing SHA256SUMS                       # file integrity, as today
    sh packaging/flatpak/content-digest.sh opensidian-0.9.2-x86_64.flatpak
    # must print e78208b343e3019a637ec21cd4cfd651cfc2041015b3f908ce025532a94ca265
    # (for later releases: compare with the line in CONTENTSUMS)

What a user runs to verify an installed app (UNVERIFIED, see Q4):

    ostree show --repo=/var/lib/flatpak/repo "$(flatpak info --show-commit dev.koto.opensidian)" | grep ContentChecksum

What an independent rebuilder runs (after the pins in 1-2 land; UNVERIFIED until step 4 is done):

    git clone https://github.com/jpzk/opensidian && cd opensidian && git checkout vX
    # flatpak update --commit=<sha> + flatpak mask for each line of flatpak-inputs.txt
    packaging/flatpak/build.sh
    sh packaging/flatpak/content-digest.sh dist/flatpak/opensidian.flatpak   # == CONTENTSUMS

## Tooling used here (reproducible itself)

- ostree 2025.7 was NOT installed: `sudo dnf install --assumeno --repo=fedora ostree` wanted to downgrade systemd-libs and pull systemd + dracut. Instead `dnf download --repo=fedora ostree ostree-libs gpgme composefs-libs` (built 2026-01-16..20, signatures checked with `rpm -K`), unpacked with `rpm2archive | tar` into a private prefix and run with `LD_LIBRARY_PATH`. `OSTREE=/path/to/ostree` points the script at it.
- No flatpak / flatpak-builder was run here; every claim about their runtime behaviour is from source and marked accordingly.

## TLDR

- Two bundles of the same tree differ in: superblock timestamp (unpinnable), commit timestamp, parent, body (flatpak version), xa.download-size (zlib), xz output (liblzma version). The tree itself differs on every fresh checkout because `files/manifest.json` stores `source-date-epoch` = manifest mtime, and on every runtime update because it stores runtime/sdk commits.
- Publish the ostree content digest (`content-digest.sh`), pin SDE + runtime/SDK/rust-extension commits, then prove determinism with a double build. v0.9.2 digest: e78208b343e3019a637ec21cd4cfd651cfc2041015b3f908ce025532a94ca265.
