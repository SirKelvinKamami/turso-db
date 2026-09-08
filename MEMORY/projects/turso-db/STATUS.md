# Turso Service — Current Status

**Last Updated:** 2026-08-31 (v1.3.0)
**Version:** 1.3.0 (durable webhook queue, auth/plan/rate-limit/user test coverage, UserStore cache, CI hardening)

---

## Where We Are

- Build environment repaired on the dev machine (Windows / windows-gnu).
- Security pass completed: Google ID-token verification via JWKS, graceful admin-password handling, committed secrets scrubbed.
- `/query` returns real column names; multi-statement `execute`; `delete_database` cleans orphan WAL/shm files.
- **v1.2.0 live on Render** (`https://turso-db-8svn.onrender.com`): webhooks with HMAC
  signing, retry/backoff, per-statement `changes`, custom headers, Supabase mirror,
  loop-free `/v1/sync/{id}` receiver. Supabase persistence confirmed at runtime
  (`auth_db: supabase://public.turso_users`).
- **v1.3.0 (this batch):**
  - **Durable webhook queue** — failed deliveries persisted to `data/pending_deliveries/`
    and retried every 60s, surviving restarts; purged when webhook/DB deleted.
  - **Test coverage 22→49** — auth/JWT, plans, rate limiter, UserStore, config seeds.
  - **UserStore Supabase cache** — warm cache at startup; reads hit memory first, fall
    back to Supabase only on miss/network failure (auth survives Supabase outages).
  - **Webhook orphan cleanup** — DB delete also removes `turso_webhooks` row.
  - **CI hardened** — fmt + clippy `-D warnings` + test + build on push/PR.
- **Git history rewritten** (filter-branch + gc): all four leaked literals purged from
  the object DB; main fully pushed to GitHub (`fd70683` was v1.2.0; v1.3.0 next).
- `cargo fmt --check`, `cargo clippy --all-targets`, `cargo build`, `cargo test` — all
  pass (49 tests), zero warnings.

## What Is Uncommitted

The v1.3.0 batch (webhooks.rs, users.rs, auth.rs, plans.rs, ratelimit.rs, config.rs,
main.rs, Cargo.toml/lock, ci.yml, CHANGELOG.md, DEPLOY.md). Ready to commit + push.

## Blockers / Decisions Needed

- **Credential rotation** (manual, boss): old JWT/admin/seed/Google values predate the
  July scrubs; ensure the Render dashboard passwords are genuinely new, not reuse.
- **Render deploy:** auto-deploy on push is active via the CI `RENDER_DEPLOY_HOOK_URL`
  secret (and `autoDeploy: true`). Confirm that GitHub secret exists or the CI deploy
  step will fail (tests still gate the push).
- **Where does data live in prod?** Supabase now set on Render (users + db files +
  analytics + webhooks). 1GB Render disk is a caching layer.

## Next Feature Candidates

- Row-level change capture including VALUES (SQLite change-tracking/framing layer).
- Full libsql replica sync (embedded replica, bidirectional conflict handling).
- HTTP route-handler integration tests (auth guards, ownership, plan limits over the wire).
- Bounded retry-slot/TTL for pending webhook deliveries (avoid unbounded file growth
  for permanently-dead receivers).

## Test Notes

- `cargo test --all-targets` → **49 tests**, all pass.
- New: JWT validity/expiry/wrong-secret, Bearer extraction, admin case-insensitivity;
  plan parsing/limits; rate-limiter windows+isolation+reset; UserStore create/verify/
  duplicates/api-key/plan/delete; config seed parsing; durable-queue persist+retry.
- Existing: webhook delivery loopback (signature/headers/payload), retry-until-success,
  give-up, pending-file roundtrip, `split_sql` suite, manual A→B sync smoke (v1.2.0).