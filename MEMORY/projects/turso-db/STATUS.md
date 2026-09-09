# Turso Service — Current Status

**Last Updated:** 2026-09-08 (v1.4.0)
**Version:** 1.4.0 (bounded retry queue, HTTP integration tests, INSERT VALUES capture, config-in-state)

---

## Where We Are

- Build environment repaired on the dev machine (Windows / windows-gnu).
- v1.3.0 live on Render (`https://turso-db-8svn.onrender.com`): durable webhook queue,
  UserStore Supabase cache, orphan cleanup, CI hardening. Confirmed healthy at health
  endpoint (`auth_db: supabase://public.turso_users`, 3 users).
- **v1.4.0 (this batch):**
  - **Bounded pending deliveries** — `WEBHOOK_PENDING_MAX_ATTEMPTS` (default 10080) and
    `WEBHOOK_PENDING_TTL_SECS` (default 604800 = 7 days); exhausted/expired deliveries
    dropped; legacy pending files without `created_at` parse via serde default.
  - **HTTP route-handler integration tests** — in-process Router + tower `TestClient`:
    auth guards, login+DB lifecycle, 404-vs-401 ordering, ownership isolation, webhook
    URL/ownership validation, plan-based rate limits, one-time setup.
  - **Row-level capture incl. VALUES** — `changes[]` entries for `INSERT ... VALUES`
    now carry `values: { columns, rows }` (best effort, null for `INSERT ... SELECT`).
  - **Config-in-AppState refactor** — preloaded `Config` in state; handlers no longer
    re-read dotenv/env per request (hermetic tests; `login` still reads admin env vars).
  - Test count 49 → **63**; `cargo fmt`, `cargo clippy --all-targets`, `cargo test
    --all-targets` all clean.
- **Credential rotation still outstanding** (manual, boss): old JWT/admin/seed/Google
  values; ensure Render dashboard passwords are genuinely new.
- Render auto-deploy on push via CI `RENDER_DEPLOY_HOOK_URL`.

## What Is Uncommitted

Nothing — v1.4.0 is committed (`b6b045a`), pushed to `origin/main`, CI green, and
**live on Render** (health endpoint reports `version: 1.4.0`, healthy, 3 users).

## Blockers / Decisions Needed

- **Credential rotation** (manual, boss) — oldest open item.
- **Where does data live in prod?** Supabase set on Render (users + db files + analytics
  + webhooks); 1GB Render disk is a caching layer.

## Next Feature Candidates

- Full libsql replica sync (embedded replica, bidirectional conflict handling).
- UPDATE/DELETE value capture alongside the INSERT VALUES work.
- Per-delivery ack configuration, per-hook custom retry policy.

## Test Notes

- `cargo test --all-targets` → **63 tests**, all pass.
- New this batch: `captures_insert_values_with_columns`, `captures_multi_row_and_quoted_values`,
  `insert_without_values_is_null`, `change_events_attach_values_only_for_inserts`,
  `pending_limits_env_overrides`, `pending_delivery_drop_rules`,
  `legacy_pending_file_without_created_at_parses`; integration: `health_and_unauthenticated_guards`,
  `user_login_and_database_lifecycle`, `admin_only_and_ownership_isolation`,
  `missing_database_is_404_but_unauthorized_wins`, `webhooks_require_ownership_and_validate_urls`,
  `plan_based_per_user_rate_limit`, `setup_creates_hub_database_once`.