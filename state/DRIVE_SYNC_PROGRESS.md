# Google Drive synchronization — active implementation

User requested full Desktop sync; not complete.

Continuation66fa1d0: separate SQLite WAL/FULL outbox scheduler supports immutable payload insert, retries with capped backoff and auth_required pause. Restart/payload and revoke/reconnect tests2/2 pass; cargo check clean. Provider upload now reconciles existing snapshot key with independent byte comparison before returning prior ID. Queue is not wired to startup/upload yet, so automatic retries are NOT currently delivered. Network transaction lock is separate from capture DB but enqueue responsiveness and bounded worker must be validated before integration.

Verified: OAuth connected by user; prior upload implementation7bcaef4 builds. New discovery/download/restore code a3ec0ab cargo check passes only; not real Drive restore acceptance.

Implemented next pieces: paginated app-folder snapshot discovery, bounded downloads, audio size/SHA256 in new manifests, hash verification before restore, restored recording as separate copy rather than overwriting local edits.

Critical remaining work before enabling full sync:
- [ ] Wire immutable snapshot preparation and durable outbox into UI upload.
- [ ] Start bounded automatic retry worker on application startup; resume after auth.
- [ ] Make restore SQLite metadata+segments atomic and idempotent.
- [ ] Preserve original identity, device/revision, track offsets and organization.
- [ ] Resolve concurrent branches without data loss; explicit deletion policy.
- [ ] Provider contract tests and restart/retry/conflict integration tests.
- [ ] Full local regression matrix, independent review and Windows build.
- [ ] Real Google upload/restart/restore acceptance with authorized account; record external consent limits honestly.
- Restore metadata+segments must be one SQLite transaction; current enqueue/append is not atomic and must not be called from UI yet.
- Persist original identity/revision/device and applied snapshot IDs; current restore creates a copy each call, not idempotent sync.
- Include track offsets and organization structure; currently omitted from manifest.
- Durable outbox must contain immutable payload/revision, retry cursor, remote IDs and automatic bounded worker; existing job.json status is insufficient.
- Upload must be idempotent across restart/timeout (resumable session persistence or deterministic remote IDs).
- Conflict resolution retaining both branches, tombstones without automatic audio deletion, discovery cursor and UI status.
- Backend/provider mock contract tests plus isolated real user account upload/restart/restore tests; current compilation is not integration evidence.

Do not describe current snapshot backup as full synchronization. Windows16.74MiB installer completion notification relates to7bcaef4 before a3ec0ab; do not claim restore included.
