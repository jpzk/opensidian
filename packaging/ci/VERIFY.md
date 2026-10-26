# Verifying an opensidian release (v0.9.1 and later)

Every release carries two independent proofs. Check both.

1. Operator GPG signature over SHA256SUMS (key fingerprint in the README):

       gpg --verify SHA256SUMS.asc SHA256SUMS && sha256sum -c SHA256SUMS

2. GitHub artifact attestation (SLSA build provenance) for each asset and for SHA256SUMS,
   proving the file was built by `.github/workflows/release.yml` in this repository on a
   GitHub-hosted runner:

       gh attestation verify <file> \
         --repo jpzk/opensidian \
         --signer-workflow jpzk/opensidian/.github/workflows/release.yml \
         --deny-self-hosted-runners

   Files: `opensidian-<v>-x86_64-slim.AppImage`, `opensidian-<v>-x86_64-portable.AppImage`,
   `opensidian-<v>-x86_64.flatpak`, `opensidian-<v>-x86_64.rpm`, `opensidian-<v>-x86_64.deb`,
   `SHA256SUMS`.

The maintainer's publish step refuses to publish a release unless both checks pass for every file.
