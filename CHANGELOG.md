# Changelog

All notable changes to this project are documented here.

## [1.4.0] - 2026-09-08

### Added
- **Bounded pending deliveries** — `WEBHOOK_PENDING_MAX_ATTEMPTS` (default 10080) and
  `WEBHOOK_PENDING_TTL_SECS` (default 604800 = 7 days) bound the durable retry queue;
  exhausted/expired deliveries are dropped with a warning. Legacy pending files without
  `created_at` parse automatically.
- **HTTP route integration tests** — axum Router built in-process with tower
  `TestClient`: auth guards (401 without token / non-admin on admin routes), user login
  + database lifecycle, 404-vs-401 ordering, ownership isolation, webhook URL/ownership
  validation, plan-based per-user rate limiting, and one-time `POST /v1/setup`.
- **Row-level change capture with values** — for `INSERT ... VALUES` statements the
  webhook `changes[]` entry now carries `"values": { "columns": [...], "rows": [[...]] }`
  (best effort; `null` otherwise, e.g. `INSERT ... SELECT`).
- **Config-in-`AppState` refactor** — handlers receive preloaded `Config` via state
  instead of re-reading dotenv/env per request, keeping tests hermetic and startup fast.

### Changed
- Test count: 49 → 63.

## [1.3.0] - 2026-08-31

### Added
- **Durable webhook delivery queue** — undelivered webhooks are persisted to
  `DATA_DIR/pending_deliveries/` and retried on a background timer, surviving process
  restarts. Queued deliveries are purged when their webhook or database is deleted.
- **Unit tests** for auth/JWT (valid, wrong-secret, garbage, expired tokens; Bearer
  extraction; admin case-insensitivity), plans (parsing, limits ordering, roundtrips),
  rate limiter (window, per-key isolation, custom limits, reset), UserStore (create,
  password verify, duplicate rejection, API-key ensure/rotate, plan set, delete, row
  parsing), and config seed parsing. **Test count: 22 → 49.**
- **UserStore Supabase cache** — users are loaded into an in-memory cache on startup;
  reads hit the cache first and fall back to Supabase only on cache miss or network
  failure, so auth keeps working if Supabase is unreachable.
- **Webhook orphan cleanup** — deleting a database now also deletes its
  `turso_webhooks` row from Supabase.
- **CI hardening** — `.github/workflows/ci.yml` now runs `cargo fmt --check`,
  `cargo clippy -- -D warnings`, `cargo test --all-targets`, and `cargo build
  --release` on push/PR (previously only build + a masked fmt check).

### Changed
- Webhook failure logging now records that a delivery was queued for later retry.

## [1.2.0] - 2026-08-31

### Added
- Webhook retry/backoff queue: failed deliveries retried `1s/2s/4s/8s` (5 attempts),
  off the write path.
- Row-level `changes[]` in webhook payloads: best-effort `op` (insert/update/delete/ddl/
  other) + `table` classification per statement.
- Custom per-webhook delivery headers (validated at creation).
- Optional Supabase mirror for webhooks (`turso_webhooks` table) with cold-start load.
- `POST /v1/sync/{id}` — loop-free sync receiver that applies webhook statements to a
  target database for push replication between instances.
- `scripts/webhook-receiver.js` — zero-dependency Node reference receiver that
  verifies `X-Turso-Signature` (HMAC, constant-time) and exposes `/last` + `/count`.
- Git history rewritten+scrubbed; leaked secrets removed from all objects.

## [1.1.0] - 2026-08-29

### Added
- Outbound write webhooks with HMAC-SHA256 signing, CRUD REST API, and add-on file
  persistence.
- `change_events` payload array; event matching (`write`, `*`); URL/event/header
  validation.

## [1.0.0] - 2026-08-28

### Added
- `/v1/auth/login` (admin + users), JWT auth with configurable expiry.
- User management: signup, list, delete, plan assignment, API keys.
- Database CRUD, `/query`, `/execute` (multi-statement), libsql/HRANA v2 pipeline.
- Google sign-in via verified JWKS ID tokens.
- Rate limiting (IP + per-user), plan-based database caps.
- Analytics tracking (volume by endpoint) with 30s Supabase flush.
- Optional Supabase persistence: users, database registry + file storage, analytics,
  webhooks.
- Static dashboard, admin script, deployment docs.