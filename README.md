# Opensidian Sync (#238, PR #238) — screenshots

Product feat/sync **c1e091c** (debug build, OpenSSL 3.5.9 FIPS provider, CMVP #5586), harness phase/238-sync **a07509ed**.
Taken on the shared box (hz-176764494) by the harness phases `syncset` and `synchist` on a boxd display lease
(box-run.sh BOX_BX=1 t1, 1280x800 display; the sync window is 760x640). Server: a local opensidian-sync-server on 127.0.0.1;
the vault was created by the CLI and joined by the app with its 12 backup words. Run 20261119T132907Z: synce2e, syncset,
synchist, syncplg all PASS.

- `syncset-1-setup-form.png` — Sync tab before setup: server URL, device name, create (enrollment token) or join (12 words), CA file.
- `syncset-2-configured.png` — after "Join with the 12 backup words": connected, 2 devices, key epoch 0, conflict mode "Automatically merge", Reveal backup words, "Vault compromised?" (burn and move, no revocation) below the fold.
- `syncset-3-conflict-mode.png` — conflict mode switched to "Create conflict file" and re-read from the engine after a tab switch.
- `synchist-1-versions.png` — File history tab: versions of Plan.md recorded from external edits.
- `synchist-2-diff.png` — a version compared with the note on disk (deleted / inserted runs), "Restore this version".
- `synchist-3-restored.png` — after the two-step restore: the picked text is on disk, the replaced text kept as a new version ("saved").

Shots that show backup words (reveal, quiz) are not published, even from this throwaway test vault.
