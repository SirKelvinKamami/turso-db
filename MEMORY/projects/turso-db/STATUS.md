# Turso Service — Current Status

**Last Updated:** 2026-08-28
**Version:** 1.0.0 (working tree has uncommitted changes, version not yet bumped)

---

## Where We Are

- Build environment repaired on the dev machine (Windows / windows-gnu).
- Security pass completed: Google ID-token verification via JWKS, graceful admin-password handling, committed secrets scrubbed.
- `/query` now returns real column names.
- `cargo fmt --check`, `cargo clippy --all-targets`, and `cargo build` all pass with zero warnings.
- No unit/integration tests exist yet (`cargo test` runs 0 tests).

## What Is Uncommitted

All of the below is in the working tree but NOT committed:

0. Multi-statement `execute`: `split_sql` splits on top-level `;` (string/identifier/comment aware); `execute` runs each statement and reports `ran N statements, X rows affected`. 8 unit tests added (`cargo test` now runs 8, all pass).
1. New file `src/google.rs` — Google ID token verification against Google JWKS
   (`https://www.googleapis.com/oauth2/v3/certs`, 1h cache, RS256, issuer + audience checks).
2. `src/routes.rs` — Google login flow, `GET /v1/auth/google-config`, admin password CONFIG_ERROR instead of panic, real column names in `/query`, analytics/plan handlers.
3. `src/db.rs` — `value_to_string`, `query_with_columns`, and delete now also removes `-wal`/`-shm` files.
4. `src/config.rs`, `src/models.rs`, `src/auth.rs`, `src/libsql.rs`, `src/analytics.rs`, `src/users.rs`, `src/plans.rs`, `src/supabase.rs`, `src/main.rs` — warning cleanup + Google config.
5. `static/dashboard.html` — Google sign-in button wired (`initGoogleSignIn`, `id_token` payload).
6. Secret scrub: `render.yaml`, `DEPLOY.md`, `admin.ps1`, `static/das-creatives-setup.js`, `static/turso-client.js`, `.env.example`.
7. `start.ps1` — fixed local build/start on this machine.
8. Whole-tree `cargo fmt` normalization.

## Blockers / Decisions Needed

- ~~**Port conflict:**~~ Resolved 2026-08-28: `BIND_ADDRESS=0.0.0.0:3100` in `D:\turso-service\.env` (untracked). Port 3000 remains owned by another project's Next.js dev server (node, PID 6068) — unchanged.
- **Render secrets:** `render.yaml` secrets now have `sync: false`. Secrets must be re-enterered in the Render dashboard, otherwise the next push-triggered deploy breaks.
- **No push yet:** everything is committed locally only (owner chose "commit only, no push").
- **Where does data live in prod?** Local files + optional Supabase. Prod Render instance has no persistent disk expectation yet (free tier).

## Next Feature Candidates

- Webhook / sync feature (user hinted, not yet specified).
- More unit tests (admin/plan/rate-limit logic).
- Extend multi-statement splitting to `/query` (currently execute only).

## Test Notes

- `cargo test --all-targets` → 8 tests, all pass (new `split_sql` unit tests).
- Manual smoke test on port 3100 verified: health, google-config, admin login,
  create db, single-statement execute (create + insert), query returning real
  column names (`id,first_name,age`) with correct rows, delete.
- Multi-statement execute verified live: `ran 3 statements, 3 rows affected`,
  result rows `[1,a;b]`, `[2,c]`, `[3,d]` — semicolons inside string literals preserved.