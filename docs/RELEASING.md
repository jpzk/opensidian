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

## 3. Draft with 11 assets

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

CI attaches **no SBOM** (operator decision 2026-11-19: the SBOM runs locally only). Every
remaining supply-chain gate (cargo audit / deny / vet, pin-check, vendored JS, WebKit bundle CVE,
glib vendor, VEX, gate self-tests) must still be green, or `draft-release` does not run.

## 3b. SBOM (local) — before publishing

Build the SBOM on the release build environment, from the exact commit the draft was built from
(the "Built by ... from commit <sha>" line in the draft body), with a clean tree:

    git fetch origin && git checkout --detach <sha>
    scripts/supply-chain/sbom-local.sh -o dist/sbom
    # -> dist/sbom/opensidian-<ver>.cdx.json + opensidian-<ver>.cdx.json.sha256, logs in dist/sbom/logs/

It merges four parts (rust, vendored JS, flatpak, WebKit bundle) and **fails closed** if any of them
fails. The WebKit part needs either Ubuntu 26.04 with the pinned set installed
(`scripts/supply-chain/webkit-apt.sh`, then `--webkit native`) or podman (`--webkit podman`, the
pinned ubuntu image of `appimage-build.sh`); `auto` picks one. `--bin <opensidian ELF>` (from the
portable AppImage) adds the binary's own library closure. A part can only be left out with
`--skip <part>`; that is recorded in the SBOM (`opensidian:sbom-local:skipped`) and must be stated
in the release notes. Do not publish an SBOM whose `opensidian:sbom-local:dirty` is `true`.

Check and scan it (non-deb findings must be 0):

    F=dist/sbom/opensidian-<ver>.cdx.json
    (cd dist/sbom && sha256sum -c opensidian-<ver>.cdx.json.sha256)
    jq -r '.metadata.properties[] | "\(.name)=\(.value)"' "$F"
    scripts/supply-chain/vex-osv-config.sh > "$HOME/osv-scanner.toml"
    osv-scanner scan source --config "$HOME/osv-scanner.toml" -L "$F"

Upload both files to the draft (the draft-release job ignores these two names, so re-running it
afterwards still passes):

    gh release upload "$TAG" "$F" "$F.sha256" --repo jpzk/opensidian

The SBOM is not in SHA256SUMS and is not attested by GitHub. If its integrity has to be provable,
sign it in step 4 as well (`gpg --armor --detach-sign opensidian-<ver>.cdx.json`, upload the `.asc`).

## 4. Verify, sign, publish (operator, offline machine)

Download everything from the draft:

    TAG=v0.9.3
    mkdir rel && cd rel
    gh release download "$TAG" --repo jpzk/opensidian

Check the files against SHA256SUMS, and check each file's provenance:

    sha256sum -c SHA256SUMS
    for f in opensidian-*; do
      case "$f" in *.cdx.json*) continue ;; esac   # local SBOM (3b): no GitHub attestation
      gh attestation verify "$f" --repo jpzk/opensidian
    done
    gh attestation verify SHA256SUMS --repo jpzk/opensidian

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
