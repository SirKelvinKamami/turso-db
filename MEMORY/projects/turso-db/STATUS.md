# Turso Service — Current Status

**Last Updated:** 2026-09-10 (v1.6.0)
**Version:** 1.6.0 (change-framing around writes, webhook PATCH)

---

## Where We Are

- Build environment healthy on the dev machine (Windows / windows-gnu).
- v1.5.0 live on Render (`https://turso-db-8svn.onrender.com`): per-webhook retry
  policy, UPDATE/DELETE values in `changes[]`. Health verified (`version: 1.5.0`,
  `auth_db: supabase://public.turso_users`, 3 users).
- **v1.6.0 (this batch, ~shipped):**
  - **True change-framing** — write batches run in `BEGIN IMMEDIATE … COMMIT/ROLLBACK`;
    each write statement gets `before` (WHERE-matched rows, UPDATE/DELETE) and `after`
    (`RETURNING rowid, *`) snapshots carried as `changes[].frame` in webhook payloads.
    Best-effort: degrades to plain execution for frame-unfriendly tables
    (WITHOUT ROWID), capped at 100 rows/frame. Failed batches now roll back entirely
    (was per-statement autocommit).
  - **Webhook PATCH** — `PATCH /v1/databases/{id}/webhooks/{hook_id}` edits
    url/events/headers in place; `secret`/`retry` use explicit-null semantics
    (omit → unchanged, `null` → clear, value → set).
  - **Replica sync plan** written: `MEMORY/projects/turso-db/SYNC_PLAN.md` (uses
    turso 0.7.2's in-tree `turso::sync` engine; opt-in, off by default; boss decision
    needed on backend + token sourcing before implementation).
  - Test count 69 → **75**; fmt/clippy/test all clean.
- **Credential rotation still outstanding** (manual, boss): old JWT/admin/seed/Google
  values; ensure Render dashboard passwords are genuinely new.
- Render auto-deploy on push via CI `RENDER_DEPLOY_HOOK_URL`.

## What Is Uncommitted

This batch is committed and pushed to `origin/main` (CI green + Render deploy fired).
Leak-literal scan clean before commit.

## Blockers / Decisions Needed

- **Credential rotation** (manual, boss) — oldest open item.
- **Replica sync go/no-go** — SYNC_PLAN open questions: primary backend (Turso Cloud
  vs sibling instance), auth token sourcing, read-replica vs bidirectional, staging.
- **Where does data live in prod?** Supabase set on Render (users + db files + analytics
  + webhooks); 1GB Render disk is a caching layer.

## Next Feature Candidates

- Implement `SYNC_PLAN.md` (phase 1 = read replicas, `SYNC_ENABLED=false` default →
  staging → boss approval).
- Wire change-frames into the libsql pipeline path (currently passes empty frames).
- Per-DB read-replica staleness surface (`GET` stats) for operator visibility.

## Test Notes

- `cargo test --all-targets` → **75 tests**, all pass.
- New in v1.6.0: `frames_capture_before_and_after_rows`,
  `batch_rolls_back_entirely_on_error`, `unframeable_tables_degrade_to_plain_execution`,
  `returning_clause_supported` (regression probe), `payload_includes_frames_when_present`.
- Kept: `patches_update_and_clear_webhook_fields` (integration).