# Turso Service — Current Status

**Last Updated:** 2026-09-09 (v1.5.0)
**Version:** 1.5.0 (per-webhook retry policy, UPDATE/DELETE value capture)

---

## Where We Are

- Build environment repaired on the dev machine (Windows / windows-gnu).
- v1.4.0 live on Render (`https://turso-db-8svn.onrender.com`): bounded pending queue,
  HTTP route integration tests, INSERT VALUES capture, config-in-AppState. Health
  verified (`version: 1.4.0`, `auth_db: supabase://public.turso_users`, 3 users).
- **v1.5.0 (this batch):**
  - **Per-webhook custom retry policy** — `CreateWebhookRequest.retry`
    (`{ max_attempts, backoff_ms }`, validated at creation) controls the immediate
    in-memory burst; echoed in webhook GET/POST. Pending-queue bounds stay global env.
  - **UPDATE/DELETE value capture** — `changes[]` `values` now covers updates
    (`set` map + optional `where`) and deletes (`where`, null for full-table sweep).
  - Test count 63 → **69**; fmt/clippy/test all clean.
- **Credential rotation still outstanding** (manual, boss): old JWT/admin/seed/Google
  values; ensure Render dashboard passwords are genuinely new.
- Render auto-deploy on push via CI `RENDER_DEPLOY_HOOK_URL`.

## What Is Uncommitted

v1.5.0 batch ready for commit + push (Cargo.toml/lock bump, CHANGELOG, DEPLOY.md,
webhook retry policy, UPDATE/DELETE capture, MEMORY).

## Blockers / Decisions Needed

- **Credential rotation** (manual, boss) — oldest open item.
- **Where does data live in prod?** Supabase set on Render (users + db files + analytics
  + webhooks); 1GB Render disk is a caching layer.

## Next Feature Candidates

- Full libsql replica sync (embedded replica, bidirectional conflict handling) — the
  largest remaining item, needs its own planning session.
- Change-tracking/framing layer BEFORE execute (true row-level values, not statement
  parsing).
- Webhook PATCH endpoint (edit url/events/headers/retry without recreating).

## Test Notes

- `cargo test --all-targets` → **69 tests**, all pass.
- New: `captures_update_set_and_where`, `captures_update_without_where_and_function_values`,
  `captures_delete_where_or_null`, `change_events_attach_values_per_op`,
  `retry_policy_validation_and_defaults`, `hook_burst_respects_policy`,
  `webhooks_accept_custom_retry_policy` (integration).