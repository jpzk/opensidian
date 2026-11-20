# Releasing opensidian

CI builds and stages a release as a **draft**. It never publishes. The GPG
signature is made offline by the operator, and only after that does the
operator publish the draft by hand. The signing key never touches GitHub.

## 1. Push the tag

Bump the version in `src-tauri/tauri.conf.json` (and the flatpak metainfo
`<release>`), merge, then:

    git tag v0.9.3
    git push origin v0.9.3

## 2. CI builds

`.github/workflows/release.yml` runs on GitHub-hosted runners pinned by label,
one matrix leg per arch, natively (no cross-compiling): x86_64 on
`ubuntu-26.04`, aarch64 on `ubuntu-26.04-arm`. Per arch:

- `appimage`: one cargo build, slim + portable AppImage
- `wrap` (rpm, deb): wraps the same binary (same-binary gate per arch)
- `flatpak`: builds in its own sandbox (`--arch=<arch>`)

Then:

- `sums`: `SHA256SUMS` over the ten files
- `draft-release`: creates the **draft** release for the tag

Every artifact gets a build provenance attestation.

### Dry run (no release)

    gh workflow run release.yml --repo jpzk/opensidian --ref <branch> -f dry_run=true

builds, attests and uploads all ten artifacts plus `SHA256SUMS` as workflow
artifacts only. `draft-release` is skipped (`dry_run` defaults to true), and it
never runs for a ref that is not a `v*` tag or a `v<ver>-release` branch anyway.

## 3. Draft with 15 assets

The draft `opensidian <ver>` for tag `v<ver>` has these assets, for
`<arch>` = `x86_64` and `aarch64`:

- `opensidian-<ver>-<arch>-slim.AppImage`
- `opensidian-<ver>-<arch>-portable.AppImage`
- `opensidian-<ver>-<arch>.flatpak`
- `opensidian-<ver>-<arch>.rpm`
- `opensidian-<ver>-<arch>.deb`
- `SHA256SUMS` (the x86_64 five first, then the aarch64 five)

The body is the SHA256SUMS content plus a "NOT SIGNED YET" note.

If the workflow is re-run while the release is still a draft, the assets are
re-uploaded (`--clobber`). If the release for the tag is already published,
the job fails and does not touch it.

The draft also carries the **SBOM**, one per arch: `opensidian-<ver>-<arch>.cdx.json` +
`opensidian-<ver>-<arch>.cdx.json.sha256` (15 assets in total). Every remaining supply-chain gate
(cargo audit / deny / vet, pin-check, vendored JS, glib vendor, VEX, gate self-tests) and the
appimage job's SBOM, license-policy and WebKit bundle CVE gates must be green, or `draft-release`
does not run.

## 3b. SBOM — where it comes from

The SBOM is generated **in the release build environment**, not on the runner host: the
`appimage` job of `release.yml` (x86_64 and aarch64) runs `packaging/ci/appimage-build.sh` with
`SBOM_TAG=<tag>`, and inside that same digest-pinned ubuntu:26.04 container (snapshot.ubuntu.com
apt pins), right after `cargo build`, it runs

    scripts/supply-chain/sbom-local.sh --release --webkit native -v <ver> -t <tag> --arch <arch> \
      --bin src-tauri/target/release/opensidian --pkgset dist/pkgset-<ver>-<arch>.tsv

which merges four parts and **fails the job** on any problem (nothing is written):

- rust: `Cargo.lock` + `cargo metadata` (licenses; crates outside the `<arch>-unknown-linux-gnu`
  release resolve are kept with `scope: excluded`);
- ui/vendor: the vendored JS, incl. the 74 npm packages bundled in `mermaid.min.js`;
- flatpak: the runtime and the brotli module;
- WebKit bundle: the portable AppImage's library closure (+ the built binary's own `ldd`
  closure), read from the dpkg database it was bundled from, with each `.deb`'s SHA-256 and its
  DEP-5 license.

Then `sbom-validate.sh --release` (CycloneDX 1.6 structure, unique bom-refs, every dependency ref
resolves, `opensidian:sbom-local:skipped` empty, `dirty` false) and `license-policy.sh` (deny.toml's
allow list + ISC; anything else only with a listed, reasoned exception in
`scripts/supply-chain/license-policy.json`) must pass. The WebKit part is then scanned on the host
with osv-scanner and gated by `osv-fixed-gate.sh` (blocking). The SBOM is attested with
`actions/attest-build-provenance`, like every other asset. A `workflow_dispatch` dry run builds
the same SBOMs and uploads them only as artifacts (`sbom-<ver>-<arch>`, logs in `sbom-logs-*`);
it does not attest them.

Locally, `scripts/supply-chain/sbom-local.sh -o dist/sbom` still builds a non-release SBOM
(`--webkit podman` recreates the build container, or `--skip webkit`, recorded in the SBOM).

## 4. Verify, sign, publish (operator, offline machine)

Download everything from the draft:

    TAG=v0.9.3
    mkdir rel && cd rel
    gh release download "$TAG" --repo jpzk/opensidian

Check the files against SHA256SUMS, and check each file's provenance:

    sha256sum -c SHA256SUMS
    for f in opensidian-*; do
      case "$f" in *.cdx.json.sha256) continue ;; esac   # covered by its .cdx.json
      gh attestation verify "$f" --repo jpzk/opensidian
    done
    gh attestation verify SHA256SUMS --repo jpzk/opensidian

Check each SBOM against its .sha256 and look at what it records (both must hold before publishing):

    for A in x86_64 aarch64; do
      sha256sum -c opensidian-*-$A.cdx.json.sha256
      jq -r '.metadata.component.purl, (.metadata.properties[] | "\(.name)=\(.value)")' opensidian-*-$A.cdx.json
    done   # purl pkg:github/jpzk/opensidian@<tag>, skipped= (empty), dirty=false, commit=<the draft's commit>

Sign SHA256SUMS with the release key:

    gpg --armor --detach-sign SHA256SUMS
    gpg --verify SHA256SUMS.asc SHA256SUMS

Upload the signature to the draft:

    gh release upload "$TAG" SHA256SUMS.asc --repo jpzk/opensidian

Publish (this is the only step that makes the release public):

    gh release edit "$TAG" --draft=false --repo jpzk/opensidian

Optionally replace the "NOT SIGNED YET" line in the release body.

## Release branch (alternative to pushing the tag)

A branch named `v<ver>-release` (listed in the workflow's `push.branches`)
builds the same set and stages the draft for tag `v<ver>` with
`--target <branch commit>`. No tag is pushed: GitHub creates `v<ver>` at that
commit when the draft is published. The branch name must match the version in
`tauri.conf.json` and the flatpak metainfo `<release>`, or the build fails.

The draft body is `.github/release-notes/v<ver>.md` (when present) followed by
the SHA256SUMS block and the run URL.

Publishing in step 4 also needs `--latest` if it should become the latest
release (CI creates drafts with `--latest=false`).
