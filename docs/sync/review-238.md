# CodeRabbit review of #238 (5aab838) — major findings triage

Critical: 0 (the review posted no inline/critical comments). Major: 68.
Status: FIXED = changed in this branch; OPEN = valid or plausible, not yet
changed — every OPEN code fix needs a compile + test run on the build box,
which is gone since 2026-11-18 22:31Z (no cargo on the agent VM).
Each OPEN item is a follow-up and blocks goal_done (criterion 8).

- crates/sync-crypto/tests/wycheproof.rs:12-12 Preserve the pinned fixture bytes on checkout. — FIXED (42235fe: .gitattributes marks crates/sync-crypto/vectors/** and wordlists/** -text; the test still asserts the pinned sha256)
- crates/sync-core/src/vault.rs:708-709 Recover a published envelope when the cfg append fails. — OPEN
- crates/sync-core/src/vault.rs:883-883 Check the key epoch before activating a paired device. — FIXED (22fe9be: pair_join returns NeedWords when the cfg head vk_epoch is newer than the envelope epoch, same gate as from_daily/refresh_chain)
- crates/sync-core/src/vault.rs:509-509 Reject chain refreshes that roll back the accepted cfg chain. — FIXED (22fe9be: refresh refuses a chain that is shorter than or forks the accepted cfg chain, envelope key epoch going back refused; test refresh_refuses_a_rolled_back_cfg_chain)
- crates/sync-core/src/keystore.rs:140-140 Make directory durability handling platform-specific. — FIXED (42235fe: directory fsync cfg(unix); on Windows the rename is the durability point)
- crates/sync-core/src/keystore.rs:134-134 Use a unique temporary file for each write. — FIXED (42235fe: create_new temp named pid.seq.nanos.attempt, removed on failure; test keystore_concurrent_puts_use_unique_temp_files)
- crates/sync-core/src/keystore.rs:105-105 Windows keystore entries have no owner-only access control. — FIXED (42235fe: fail closed, FileKeystore::open refuses off unix instead of writing secrets without owner-only access; a Windows ACL/DPAPI keystore is a follow-up)
- crates/sync-core/src/transport/fs.rs:250-250 Reject unsupported USB volumes during replica setup. — OPEN
- crates/sync-core/src/transport/fs.rs:164-164 Read the replica log from the requested cursor. — OPEN
- crates/sync-core/src/transport/fs.rs:233-235 Repair the list index before returning Identical. — OPEN
- crates/sync-core/src/transport/http.rs:262-263 Enforce a deadline for the whole replica request. — FIXED (Deadline stream: per-op timeout = min(idle, time left), whole request <= timeout + REQUEST_BUDGET 300 s, TLS wraps it; test whole_request_deadline_beats_a_trickling_peer)
- crates/sync-core/src/transport/http.rs:259-260 Check the resolved address before opening a plain HTTP connection. — FIXED (plain http skips every resolved non-loopback address; test plain_http_connects_to_loopback_addresses_only)
- crates/sync-core/src/engine.rs:436-440 Detect ancestry cycles before expanding parents. — FIXED (22fe9be: cyclic parents stay pending, ready_and_gen terminates; test parent_cycle_is_pending_not_a_hang)
- crates/sync-core/src/engine.rs:644-644 Skip the push phase for replicas that fail during pull. — FIXED (8172d27: a replica whose get fails mid-pull is listed as None, so no push; test replica_failing_in_pull_is_not_pushed_to)
- crates/sync-core/src/engine.rs:262-262 Use a Windows-compatible state persistence path. — FIXED (42235fe: parent-dir fsync cfg(unix) only)
- crates/sync-core/src/engine.rs:723-723 Order new merge commits after their parents. — OPEN
- crates/sync-core/src/engine.rs:577-579 Keep the live head consistent when saving state fails. — OPEN
- crates/sync-core/src/engine.rs:613-615 Try another replica after verification rejects an object. — FIXED (8172d27: the reject skip is per replica; test bad_copy_on_one_replica_does_not_block_the_good_copy)
- crates/sync-core/src/engine.rs:662-720 Check a commit's declared tree_hash before the engine stores it as head. — FIXED (22fe9be: a tip is adopted only after its declared tree_hash matches the rebuilt tree, else Report.invalid; test commit_with_a_lying_tree_hash_never_becomes_head)
- crates/sync-core/src/merge/dmp.rs:351-351 Do not apply a hunk to a different occurrence when the original was edited. — OPEN
- crates/sync-core/src/merge/mod.rs:333-339 Reject merges that place a file and a directory at the same path. — OPEN
- crates/sync-server/src/http.rs:395-395 Stream upload bodies instead of retaining one full buffer per connection. — FIXED (server-wide body budget: each request reserves its worst case from a shared semaphore before the first body byte, released when the handler drops the bytes; a 16 MiB object still needs one buffer because the header + sha256 are checked before publish; test body_budget_is_shared_across_connections)
- crates/sync-server/src/http.rs:380-380 Bound total storage for anonymous envelopes. — FIXED (max_env_object 64 KiB per env object, max_env_total 256 MiB for the whole tokenless namespace -> 507; test env_object_and_namespace_caps)
- crates/sync-server/src/http.rs:516-528 Make the destroy receipt recoverable after the purge commits. — OPEN
- crates/sync-server/src/http.rs:155-155 Apply a deadline to request bodies. — FIXED (body_timeout_secs 60 s for budget wait + whole body read; test stalled_body_hits_the_deadline)
- crates/sync-server/src/lib.rs:161-161 Publish the receipt key only after writing it in full. — OPEN
- crates/sync-server/src/store.rs:298-303 Give each PUT its own staging file. — FIXED (pid + process-wide counter in the name, create_new)
- crates/sync-server/src/store.rs:382-383 Make the published object path durable before committing the index. — OPEN
- crates/sync-server/src/store.rs:169-170 Restore staged vault files when the destroy transaction did not commit. — OPEN
- crates/sync-server/src/store.rs:624-627 Keep object tombstones after reclaiming their bytes. — OPEN
- crates/sync-server/src/store.rs:628-631 Debit quota only for the object that GC actually removed. — OPEN
- crates/sync-core/src/history/mod.rs:417-417 Sync the store directory after creating index.log. — OPEN
- crates/sync-core/src/history/mod.rs:913-914 Recover or invalidate the journal after an append error. — OPEN
- crates/sync-core/src/history/mod.rs:567-568 Keep capture and restore inside the vault when a parent is a symlink. — FIXED (d0c2b51: sync-core vaultfs anchored read/write; tests vaultfs::*, history::restore_*)
- crates/sync-core/src/history/mod.rs:571-573 Create restore staging files without replacing an existing file. — FIXED (d0c2b51: sync-core vaultfs anchored read/write; tests vaultfs::*, history::restore_*)
- crates/sync-core/src/history/mod.rs:773-773 Do not leave the old journal writable after a replacement. — OPEN
- crates/sync-core/src/history/mod.rs:675-681 Preserve rename records needed to prevent old paths from reappearing. — OPEN
- crates/sync-core/src/history/mod.rs:566-566 Stop restore when history skips the current file. — FIXED (d0c2b51: sync-core vaultfs anchored read/write; tests vaultfs::*, history::restore_*)
- crates/sync-core/src/history/mod.rs:412-412 Discard the torn tail before decoding UTF-8. — OPEN
- crates/sync-core/src/history/mod.rs:611-612 Keep history-write failures from being ignored during sync merges. — OPEN
- crates/sync-core/src/history/mod.rs:42-43 Serialize daemon history writers on the engine lock. — OPEN
- crates/sync-core/src/history/cas.rs:58-58 Make new shard directories durable before journaling their blobs. — OPEN
- crates/sync-core/src/history/cas.rs:55-56 Verify existing blobs before treating a snapshot as recoverable. — OPEN
- crates/syncd/src/api.rs:150-150 Use a compact wire representation for history bytes. — OPEN
- crates/sync-core/src/history/fsx.rs:41-43 Do not collect blobs after a failed journal directory sync. — OPEN
- crates/syncd/src/local.rs:321-321 Retry pairing only while the reply is still available. — OPEN
- crates/syncd/src/local.rs:245-245 Persist an absolute CA file path. — OPEN
- crates/syncd/src/local.rs:441-442 Return the verified receipt even if local cleanup fails. — OPEN
- crates/syncd/src/local.rs:498-498 Apply file and directory replacements in dependency order. — OPEN
- crates/syncd/src/local.rs:503-503 Keep the disk comparison valid through the write or delete. — OPEN
- crates/syncd/src/local.rs:191-191 Keep keystore entries outside the synced vault. — FIXED (reject either dir containing the other) — uncompiled
- crates/syncd/src/local.rs:248-249 Make setup resumable after the enrollment token is consumed. — OPEN
- crates/syncd/src/local.rs:485-486 Resume pending disk updates before creating local edits. — OPEN
- crates/syncd/src/local.rs:461-486 Commit large local edit sets in several batches, or sync stalls for vaults with more than about 12 MiB of changed inline content. — OPEN
- crates/syncd/src/ipc.rs:104-104 Do not unlink a regular file at the socket path. — FIXED (stale path removed only if it is a socket) — uncompiled
- crates/syncd/src/ipc.rs:154-158 Authenticate the daemon before sending backup words. — FIXED (client checks socket + 0700 dir owned by own uid before connect and before every request) — uncompiled
- crates/syncd/src/workdir.rs:84-85 Check the file that sync actually reads. — FIXED (d0c2b51: sync-core vaultfs anchored read/write; tests vaultfs::*, history::restore_*)
- crates/syncd/src/workdir.rs:106-108 Anchor materialization to the vault directory. — FIXED (d0c2b51: sync-core vaultfs anchored read/write; tests vaultfs::*, history::restore_*)
- src-tauri/src/syncui.rs:135-137 Apply the app's offline policy to desktop sync. — FIXED (7b8bfc3: LocalEngine::with_net_gate, every server connection asks it first; the desktop passes net_allowed (offline mode/lockdown) to every sync command and sync_setup checks it before connecting; test net_gate_refuses_every_server_connection)
- src-tauri/src/main.rs:2164-2164 Notify the main window when history restores a note. — FIXED (e35b1d8: notify_restored emits vault-changed modified/added after the write; the editor re-reads the open tab via onVaultChanged)
- src-tauri/src/main.rs:2186-2186 Restore the newest stored bytes, not the oldest. — FIXED (dropped .rev(); versions() is newest first) — uncompiled
- ui/sync.js:173-173 Discard stale history responses before updating the shared view. — FIXED (4284e77: viewGen bumped by every clearView; history_list/diff/deleted replies for an older view are dropped)
- docs/sync/spec.md:217-220 Include rsig_pub in the cfg wire layout. — FIXED (docs)
- docs/sync/spec.md:554-555 Do not promise snapshots when history hooks fail. — FIXED (docs)
- docs/sync/spec.md:252-254 Update the pairing wire layout. — FIXED (docs)
- docs/sync/spec.md:297-303 Rotate the phrase before publishing a new epoch to a former device. — DOCUMENTED as open residual (spec §4.6 notes, threat-model R6); code fix = phrase rotation on revoke, OPEN
- docs/sync/key-tree.md:3-4 Remove the validation claim. — FIXED (docs)
- src-tauri/src/main.rs:8381-8382 Keep the vault open when the user closes Sync. (outside diff) — FIXED (93ca6cc: on_window_event returns early for SYNC_LABEL; test sync_window_close_skips_winsize_and_quit)
