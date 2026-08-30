# Turso Service — Current Status

**Last Updated:** 2026-08-31
**Version:** 1.2.0 (webhooks: retries, row-level events, headers, Supabase mirror, sync receiver)

---

## Where We Are

- Build environment repaired on the dev machine (Windows / windows-gnu).
- Security pass completed: Google ID-token verification via JWKS, graceful admin-password handling, committed secrets scrubbed.
- `/query` returns real column names; multi-statement `execute`; `delete_database` cleans orphan WAL/shm files.
- **Webhooks mature (v1.2.0):** outbound `write` webhooks with HMAC signing, retry/backoff,
  per-statement `changes` classification, custom delivery headers, optional Supabase
  mirror, and a loop-free `/v1/sync/{id}` receiver for push replication between instances.
- **Git history rewritten** (54 commits, `filter-branch` + `gc`): all four leaked literals
  purged from the object database (verified); rewritten main sits ahead 3 / behind 0 of
  origin/main so a normal push works.
- `cargo fmt --check`, `cargo clippy --all-targets`, `cargo build`, `cargo test` — all pass
  (22 tests), zero warnings.

## What Is Uncommitted

Nothing — everything (v1.1.0 + v1.2.0 + rewritten history) is committed locally.
Three commits ahead of `origin/main`; push intentionally withheld until the boss
re-enters Render secrets and rotates credentials.

## Blockers / Decisions Needed

- ~~**Port conflict:**~~ Resolved 2026-08-28: `BIND_ADDRESS=0.0.0.0:3100` in `D:\turso-service\.env` (untracked). Port 3000 remains owned by another project's Next.js dev server (node, PID 6068) — unchanged.
- **Render secrets:** `render.yaml` secrets now have `sync: false`. Secrets must be re-enterered in the Render dashboard, otherwise the next push-triggered deploy breaks.
- **Push pending:** history rewritten (filter-branch) + feature commits ready; push is fast-forward-able (ahead 3 / behind 0). Owner approval required — Render secrets must be set first so the auto-deploy boots with real env.
- **Credential rotation:** old JWT/admin/seed/Google values were exposed; rotate before/at push (guide delivered 2026-08-29; renewal steps in SESSION_LOG).
- **Where does data live in prod?** Local files + optional Supabase. Prod Render instance has no persistent disk expectation yet (free tier).

## Next Feature Candidates

- Durable (disk/Supabase) webhook delivery queue spanning restarts.
- Row-level change capture including VALUES (SQLite change-tracking/framing layer).
- Supabase cleanup of orphaned webhook rows.
- Full libsql replica sync (embedded replica, bidirectional conflict handling).
- More unit tests (admin/plan/rate-limit logic).

## Test Notes

- `cargo test --all-targets` → 22 tests, all pass (8 `split_sql` + 14 webhooks).
- Live loopback delivery test proves HMAC signature + headers + payload reach an HTTP target.
- In-process flaky/always-fail axum handlers verify retry-until-success and give-up-after-backoff.
- Manual smoke: Node receiver verified signature (`sha256=`, constant-time) and classified a
  batch into insert+update; a `BAD=2` receiver forced exactly 3 attempts; a webhook→
  `/v1/sync/{db}` chain replicated A→B with no loop.