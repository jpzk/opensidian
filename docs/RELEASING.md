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

`.github/workflows/release.yml` runs on GitHub-hosted `ubuntu-latest` runners:

- `appimage`: one cargo build, slim + portable AppImage
- `wrap` (rpm, deb): wraps the same binary
- `flatpak`: builds in its own sandbox
- `sums`: `SHA256SUMS` over the five files
- `draft-release`: creates the **draft** release for the tag

Every artifact gets a build provenance attestation.

## 3. Draft with 6 assets

The draft `opensidian <ver>` for tag `v<ver>` has these assets:

- `opensidian-<ver>-x86_64-slim.AppImage`
- `opensidian-<ver>-x86_64-portable.AppImage`
- `opensidian-<ver>-x86_64.flatpak`
- `opensidian-<ver>-x86_64.rpm`
- `opensidian-<ver>-x86_64.deb`
- `SHA256SUMS`

The body is the SHA256SUMS content plus a "NOT SIGNED YET" note.

If the workflow is re-run while the release is still a draft, the assets are
re-uploaded (`--clobber`). If the release for the tag is already published,
the job fails and does not touch it.

## 4. Verify, sign, publish (operator, offline machine)

Download everything from the draft:

    TAG=v0.9.3
    mkdir rel && cd rel
    gh release download "$TAG" --repo jpzk/opensidian

Check the files against SHA256SUMS, and check each file's provenance:

    sha256sum -c SHA256SUMS
    for f in opensidian-*; do
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
