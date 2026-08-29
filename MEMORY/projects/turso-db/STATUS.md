# Turso Service — Current Status

**Last Updated:** 2026-08-29
**Version:** 1.1.0 (webhook/sync feature; uncommitted until this session's commit)

---

## Where We Are

- Build environment repaired on the dev machine (Windows / windows-gnu).
- Security pass completed: Google ID-token verification via JWKS, graceful admin-password handling, committed secrets scrubbed.
- `/query` returns real column names; multi-statement `execute`; `delete_database` cleans orphan WAL/shm files.
- **Webhook/sync feature shipped (v1.1.0):** outbound `write` webhooks with HMAC signing, wired to both `/execute` and the libsql pipeline; CRUD API + file persistence + 16 passing tests.
- `cargo fmt --check`, `cargo clippy --all-targets`, `cargo build`, `cargo test` — all pass, zero warnings.

## What Is Uncommitted

This session's webhook feature is committed locally only (not pushed):

1. `src/webhooks.rs` — webhook store + dispatch (HMAC signing, fire-and-forget).
2. `src/routes.rs` — webhook CRUD routes + dispatch from `execute_query`.
3. `src/libsql.rs` — pipeline accumulates writes and dispatches.
4. `src/db.rs` — `ExecuteReport`, `pub(crate)` `split_sql`, `sql_is_query`.
5. `src/models.rs` — `CreateWebhookRequest`, `WebhookResponse`.
6. `Cargo.toml`/lock — `hmac`, `sha2`, `hex`; version `1.1.0`.
7. `DEPLOY.md` — webhook docs.

(All of the 2026-08-28 security/columns/lint work and the MEMORY structure are already committed as `bf74f20` + `0974b57`.)

## Blockers / Decisions Needed

- ~~**Port conflict:**~~ Resolved 2026-08-28: `BIND_ADDRESS=0.0.0.0:3100` in `D:\turso-service\.env` (untracked). Port 3000 remains owned by another project's Next.js dev server (node, PID 6068) — unchanged.
- **Render secrets:** `render.yaml` secrets now have `sync: false`. Secrets must be re-enterered in the Render dashboard, otherwise the next push-triggered deploy breaks.
- **No push yet:** everything is committed locally only (owner chose "commit only, no push").
- **Where does data live in prod?** Local files + optional Supabase. Prod Render instance has no persistent disk expectation yet (free tier).

## Next Feature Candidates

- Row-level insert/update/delete webhook events (SQL analysis / change capture pre-execute).
- Webhook retry/backoff queue + delivery logs.
- Supabase-backed webhook persistence.
- Full libsql HTTP replica sync (proper bidirectional sync, larger effort).
- More unit tests (admin/plan/rate-limit logic).

## Test Notes

- `cargo test --all-targets` → 16 tests, all pass (8 `split_sql` + 8 webhook).
- Live loopback delivery test proves HMAC signature + headers + payload reach an HTTP target.
- Manual smoke test verified the webhook CRUD API, validation errors, non-blocking
  dispatch (~114ms for a 2-statement batch), and cleanup on DB delete.