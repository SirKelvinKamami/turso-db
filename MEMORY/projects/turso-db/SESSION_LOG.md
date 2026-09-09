# Turso Service — Session Log

Log of AI working sessions. Newest first.

---

## 2026-09-10 — v1.6.0: change-framing around writes, webhook PATCH, replica-sync plan

**Model/session:** opencode (big-pickle)

### Objective
Work the next feature candidates after v1.5.0 shipped: true change-framing BEFORE
execute (row-level before/after snapshots, replacing statement-parsing claims),
the webhook PATCH endpoint, then close the batch (version bump + MEMORY) and ship
(commit + push + CI + Render verify). Also write the standing planning doc for full
libsql replica sync.

### What was built (src/db.rs, src/webhooks.rs, src/models.rs, src/routes.rs, src/libsql.rs)
- **True change-framing (`src/db.rs`):** `execute` now wraps the whole write batch in
  `BEGIN IMMEDIATE … COMMIT`/`ROLLBACK` (control statements skipped). Per statement,
  `read_frame` snapshots matched rows before UPDATE/DELETE via
  `SELECT rowid, * FROM "<table>" WHERE <raw where>`, and after every write via
  `RETURNING rowid, *` (INSERT/UPDATE get `after`, DELETE gets `before`). Snapshots
  stringified like query output, capped at `FRAME_CAP_ROWS = 100`. `ExecuteReport`
  gained `frames: Vec<StatementFrame>`; a `has_rows()` shallow check decides whether a
  frame is attached to `changes[].frame` in the payload. Batch atomicity is a
  side-effect of the same change: a failed statement rolls back the entire batch
  (previously per-statement autocommit). Frame capture failures degrade to plain
  execution (WITHOUT ROWID tables: no implicit rowid, so no before/after).
- **`src/webhooks.rs`:** new `RowFrame`/`StatementFrame` types (Serialize/Deserialize),
  `where_text_for` (UPDATE uses a new `find_top_level_word` — quote/paren/comment
  aware — since UPDATE has no "after-keyword FROM"; DELETE reuses `capture_where_suffix`).
  `change_events(statements, Option<&[StatementFrame]>)`, `build_payload(..., frames)`,
  `dispatch(..., frames)`. All statement parsing for *values* (INSERT VALUES /
  update set/where / delete where) is preserved additively.
- **Webhook PATCH (`src/models.rs`, `src/routes.rs`):** `UpdateWebhookRequest` with
  double-option `secret`/`retry`; a new `optional_field` module implements a custom
  serde `Visitor` (`visit_none` → `Some(None)`) so JSON `null` means "clear" while an
  absent key means "leave" (the plain `Option<Option<T>>` + `#[serde(default)]` idiom
  conflates null with absent). `WebhookStore::update` validates url/events/headers/
  retry before mutating. Route: `patch(update_webhook).delete(delete_webhook)`
  (was `delete` only). 200 + updated webhook; 404 unknown hook; 400 validation;
  forbidden template untouched. Integration test
  `patches_update_and_clear_webhook_fields`.
- **libsql pipeline (`src/libsql.rs`):** deliver path passes `Vec::new()` frames —
  no framing through the libsql protocol yet (noted limitation).
- **Replica sync plan (`MEMORY/projects/turso-db/SYNC_PLAN.md`):** planning doc,
  no code. Verified turso 0.7.2 ships a real embedded-replica engine
  (`turso::sync::{Builder,Database}`: `push`/`pull`/`checkpoint`/`stats`/`connect`,
  auth-token callbacks, long-poll). Documents the type split
  (`turso::Database` vs `turso::sync::Database`), write-path push-after-commit
  ordering, conflict semantics (read-replica first; LWW for bidirectional), testing,
  rollout gates, and open questions for the boss (primary backend, token sourcing,
  read-vs-bidirectional, staging target).

### Bugs / gotchas fixed during tests
- **Non-`Send` error across an await broke every handler:** the first framing draft
  held a plain `Box<dyn Error>` across the `COMMIT` await → `!Send` future → "the
  trait bound `…: Handler` is not satisfied" for all three handlers awaiting
  `db_manager.execute`. Fixed by `FrameError(String)` (`Display` + `Error` +
  value-in-box `Box<dyn Error + Send>`) internally, coercing to plain
  `Box<dyn Error>` only at the routing boundary via `map_err`.
- **This serde (1.0.229) moved error type to a method generic:** the `Visitor` impl
  used `Self::Error` (old serde), but this version's `visit_none<E>`/`visit_some<D>`
  take the error as a type parameter → `E0308`/missing-associated-type. Rewrote the
  visitor with generic `E`/`D2` signatures (`T::deserialize(d).map(|v| Some(Some(v)))`).

### Tests / verification
- `cargo test --all-targets` → **75 pass** (was 69). New:
  `frames_capture_before_and_after_rows`, `batch_rolls_back_entirely_on_error`,
  `unframeable_tables_degrade_to_plain_execution`, `returning_clause_supported`,
  `payload_includes_frames_when_present`, plus integration
  `patches_update_and_clear_webhook_fields`.
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean.
- Leak-literal scan clean before commit.

### Notes / limitations
- Framing is best-effort and always readable by consumers who ignore `frame`; `values`
  and `frame` are independent (statement-parsed vs engine-traced).
- libsql-pipeline deliveries carry no frames yet (route `dispatch_webhooks` uses
  `report.frames`; pipeline passes empty). Replica `pull()` applies remote writes
  without frames and without re-dispatching webhooks (loop-free).
- Batch atomicity is now the contract: a mixed success/failure batch returns a rollback
  instead of partial application.

### Files changed
`src/db.rs`, `src/webhooks.rs`, `src/models.rs`, `src/routes.rs`, `src/libsql.rs`,
`Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, `DEPLOY.md`,
`MEMORY/projects/turso-db/{STATUS,SESSION_LOG,SYNC_PLAN}.md`, new daily report,
new lessons file. Push = `git push origin main` (Render auto-deploys via CI hook).

---

## 2026-09-09 — v1.5.0: per-webhook retry policy, UPDATE/DELETE value capture

**Model/session:** opencode (big-pickle)

### Objective
Work the next feature candidates after v1.4.0 shipped: per-hook custom retry policy
(+ per-delivery retry config), UPDATE/DELETE row-level capture alongside the INSERT
VALUES work. (Full libsql replica sync deferred — needs its own planning session.)

### What was built (`src/webhooks.rs`, `src/routes.rs`, `src/models.rs`)
- **Per-webhook retry policy:** new `RetryPolicy { max_attempts: u32, backoff_ms: Vec<u64> }`
  (serde Serialize/Deserialize, `Default` = 5 attempts / 1s,2s,4s,8s) as an optional
  `Webhook.retry` field (`#[serde(default)]`, legacy files parse). `WebhookStore::add`
  gained a `retry` param validated by `validate_retry` (max_attempts 1..=100, at most 20
  delays each 1..=600000ms). `Webhook.in_memory_delays()` builds the per-hook backoff
  schedule capped at `max_attempts - 1`; `burst_attempts()` seeds the pending queue.
  `deliver`, `retry_pending_once`, and `persist_pending` now use the hook policy
  instead of the hardcoded `DEFAULT_BACKOFF`. `CreateWebhookRequest.retry` +
  `WebhookResponse.retry` echo it over the API. Pending-queue bounds (env TTL/max)
  intentionally remain global.
- **UPDATE/DELETE value capture:** new `capture_update_details` (`{ set: {col: val},
  where: "<raw>" }`, WHERE omitted when absent) and `capture_delete_details`
  (`{ where: "<raw>" }`, null for a full-table `DELETE FROM t`). New tokenizer
  helpers `split_top_level` / `find_top_level` (quote + paren/bracket depth aware) and
  `keyword_pos` (word-boundary keyword index, matching `after_keyword` semantics).
  `change_events` attaches `values` for insert/update/delete. Fixed a real bug during
  tests: `find_top_level` indices were computed on the trimmed part but applied to the
  untrimmed slice → values came back prefixed with `= `; rebinding `part` to the
  trimmed slice fixed it.

### Tests / verification
- `cargo test --all-targets` → **69 pass** (was 63). New: update/delete capture
  suites, `change_events_attach_values_per_op`, retry validation + burst, and the
  `webhooks_accept_custom_retry_policy` integration test (201 with policy echoed, 400
  on bad max_attempts, GET roundtrip).
- `cargo fmt --check` and `cargo clippy --all-targets` clean.

### Notes / limitations
- Values capture is best-effort statement parsing (WHERE text is raw, no expression
  evaluation); UPDATE captures the literal SET expressions, not post-execution rows.
- Retry policy governs the immediate burst only; durable pending retries still follow
  the global `WEBHOOK_PENDING_*` env bounds.

### Files changed
`src/webhooks.rs`, `src/routes.rs`, `src/models.rs`, `Cargo.toml`, `Cargo.lock`,
`CHANGELOG.md`, `DEPLOY.md`, MEMORY files. Push = `git push origin main` (Render
auto-deploys via CI hook).

---

## 2026-09-08 — v1.4.0: bounded retry queue, HTTP integration tests, INSERT VALUES capture, config-in-state

**Model/session:** opencode (big-pickle)

### Objective
Continue the "Nice to have" list after v1.3.0 shipped: bounded TTL/max-attempts for
pending deliveries, HTTP route-handler integration tests, row-level change capture
incl. VALUES for INSERT, then ship the batch (bump + docs + commit + push).

### What was built
- **Bounded pending deliveries (`src/webhooks.rs`):** `PendingDelivery.created_at`
  (RFC3339; `#[serde(default = "clock_now_rfc3339")]` so legacy files without it parse),
  `persist_pending` stamps it, `retry_pending_once` enforces `pending_limits()` —
  `WEBHOOK_PENDING_MAX_ATTEMPTS` (default 10080) and `WEBHOOK_PENDING_TTL_SECS`
  (default 604800) — dropping exhausted/expired deliveries with a warn, and increments
  `attempts` + rewrites the file on failure.
- **HTTP route-handler integration tests (`src/routes.rs mod integration_tests`):**
  in-process Router with tower `TestClient`. Helpers `build_ctx()` (async; creates
  `Config` + temp dir + UserStore + Router), `send()`, `test_config()`, `TestCtx
  { app, user_store, dir }`. Tests: health + unauthenticated guards (401s on
  `/users`/admin/health-with-token path), login + DB lifecycle (create→query→delete),
  admin-only + ownership isolation (user A cannot touch B's db), missing-db 404-vs-401
  ordering, webhook URL/ownership validation, plan-based per-user rate limit
  (Free user + 100 executes → 429), one-time setup (second POST → 409, hub db created).
  Admin tokens minted directly via `create_token` (login still needs env).
- **Config-in-AppState refactor:** `Config` now lives in `AppState`; `api_routes(config,
  ...)` signature, `main.rs` wires `config.clone()`; `authenticate(&state, &headers)`,
  `authenticate_admin(&state, &headers)`, `google_config(State<AppState>)`. Handlers no
  longer call `Config::load()` per request — no per-request dotenv reads, hermetic
  tests. (One build error fixed: `.clone()` around `google_client_id`; one accidental
  edit removed `volume_raw`, restored.)
- **Row-level change capture incl. VALUES:** `capture_insert_values(sql)` →
  `{ columns, rows }` parsed from the `VALUES` clause (tokenizer handles '
  'quoted strings', '' escapes, double-quote/backtick/bracket identifiers, nested
  parenthesized calls, multi-row tuples, comma-separated rows). `change_events` adds
  `values` only for `insert` ops; `null` when unparsable (`INSERT ... SELECT`).
- **Version/docs:** Cargo.toml/lock `1.3.0 → 1.4.0`; CHANGELOG 1.4.0 entry; DEPLOY.md
  payload example shows `values`, queue-bounds env vars documented; daily report.
- **Deps (dev):** `tower = { version = "0.5", features = ["util"] }`,
  `http-body-util = "0.1"`.

### Tests / verification
- `cargo test --all-targets` → **63 pass** (was 49). Integration run caught two real
  details: `create_database` returns 200 (no explicit status tuple) and query rows are
  `Vec<Vec<String>>` (id arrives as `"1"`).
- `cargo fmt --check` and `cargo clippy --all-targets` clean (ci runs clippy with `-D
  warnings`; fixed `collapsible_if` in webhooks + `useless format!` in a test).

### Notes / limitations
- VALUES capture is best-effort statement parsing, not real change-framing before
  execute — UPDATE/DELETE value capture and true pre-execute framing remain future
  work. `values` is additive to the payload (backward compatible).
- 7-day TTL of pending deliveries means a receiver down >7 days loses events (by
  design, bounded file growth).

### Files changed
`src/webhooks.rs`, `src/routes.rs`, `src/main.rs`, `Cargo.toml`, `Cargo.lock`,
`CHANGELOG.md`, `DEPLOY.md`, MEMORY files. Push = `git push origin main` (Render
auto-deploys via CI hook).

---

## 2026-08-31 (cont.) — v1.3.0: durable webhook queue, test coverage, UserStore cache, CI polish

**Model/session:** opencode (big-pickle)

### Objective
Work the "Important" and "Nice to have" feature lists after the v1.2.0 launch:
durable webhooks, expanded unit tests, UserStore resilience, Supabase orphan cleanup,
CI hardening, changelog + version bump.

### What was built (commit pending `…`)
- **Durable webhook queue (`src/webhooks.rs`):** after in-memory backoff is exhausted,
  the delivery is serialized to `DATA_DIR/pending_deliveries/<uuid>.json` as a
  `PendingDelivery` (full webhook snapshot + base64-free JSON payload). A background
  timer (`spawn_retry_loop`, 60s) replays pending deliveries via
  `retry_pending_once` (backed by `deliver_with_backoff`); files are deleted on
  success. `remove`/`remove_all` purge matching pending files so deleted resources
  stop retrying. Supabase parity: deleting a DB now also deletes its `turso_webhooks`
  row (orphan cleanup).
- **UserStore cache (`src/users.rs`):** new `load_all()` warms the in-memory map from
  Supabase at startup (wired in `main.rs`); `get_user` reads the cache first and falls
  back to Supabase only on miss; `list_users` returns the cache if Supabase is
  unreachable; writes (`create_user`/`set_api_key`/`set_plan`/`delete_user`) keep the
  cache in sync. Auth now survives a Supabase outage.
- **Tests 22→49:**
  - `auth.rs`: token roundtrip/wrong-secret/garbage/expired (direct JWT encode for a
    guaranteed-past `exp` — jsonwebtoken's 60s leeway broke the 0-hour variant),
    Bearer extraction, admin case-insensitivity, unique API keys.
  - `plans.rs`: parsing case-insensitivity, unknown→Free, ordered limits, as_str
    roundtrip, default-Plan.
  - `ratelimit.rs`: admits-up-to-max, per-key isolation, custom limit override,
    window reset (backdated `Instant` via internal state), accessors.
  - `users.rs`: create+verify, duplicate rejection, mem-cache `get_user`, API-key
    ensure/rotate, set_plan/delete, `from_row` defaults.
  - `config.rs`: extracted `parse_seed_users()` (kept module-local, not public API) and
    tested good/malformed/empty inputs.
  - `webhooks.rs`: pending delivery serialization roundtrip + durable-queue test
    (persist → fresh store reloads → retry succeeds against flaky endpoint → file
    removed).
- **CI (`.github/workflows/ci.yml`):** removed `fmt --check || true` (it masked
  failures), added `cargo clippy --all-targets -- -D warnings`, `cargo test
  --all-targets`, kept the Render deploy hook step. Trailing `cargo build --release`
  retained as the last gate.
- **Version/docs:** `1.3.0` in Cargo.toml (+lock), new `CHANGELOG.md` (1.0→1.3),
  DEPLOY.md gain "Durable queue" bullet, MEMORY STATUS/SESSION_LOG/daily updated.

### Tests / verification
- `cargo build`, `cargo fmt --check`, `cargo clippy --all-targets` (0 warnings),
  `cargo test --all-targets` → **49 pass**.
- Runtime: Render still serving v1.2.0 at `https://turso-db-8svn.onrender.com`
  (healthy, `supabase://public.turso_users`) — v1.3.0 will land via push → auto-deploy.

### Notes / limitations
- Pending deliveries retry every 60s **forever** for truly-dead receivers; a bounded
  TTL/max-retry-slot is a follow-up (files grow one-per-undelivered batch otherwise).
- Supabase write paths for webhooks are still fire-and-forget (best-effort mirror);
  the local file + pending dir are the durable source of truth.
- Lockstep: the CI `RENDER_DEPLOY_HOOK_URL` GitHub secret must exist or the deploy
  step of CI fails (build/test still gate the push).

### Files changed
`src/webhooks.rs`, `src/users.rs`, `src/auth.rs`, `src/plans.rs`, `src/ratelimit.rs`,
`src/config.rs`, `src/main.rs`, `Cargo.toml`, `Cargo.lock`,
`.github/workflows/ci.yml`, `CHANGELOG.md`, `DEPLOY.md`, MEMORY files.

---

## 2026-08-31 — Webhooks v1.2.0: retries, row-level events, headers, Supabase mirror, sync receiver (all-of-the-above batch)

**Model/session:** opencode (big-pickle)

### Objective
Finish the webhook roadmap: retry/backoff queue, row-level change events, custom
delivery headers, optional Supabase persistence, a loop-free sync receiver for
push-based instance replication, a recipient helper script, git history cleanup,
and push preparation.

### What was built (commit `…` after this entry)
- **`src/webhooks.rs`:**
  - Retry/backoff queue: fire-and-forget delivery now retries on failure/non-2xx
    (`1s/2s/4s/8s`, 5 attempts total), all off the request path.
  - Per-statement `changes` in the payload: best-effort parser `classify_write`
    returns `op` (insert/update/delete/ddl/other/null) + `table` (handles quoted
    ids, `IF NOT EXISTS`, `UPDATE OR <behavior>`, comments). CTEs → null.
  - Custom webhook `headers` map (validated at creation) applied at delivery —
    enables e.g. `Authorization` for sync receivers.
  - Optional Supabase mirror: store constructor takes `Option<Supabase>`; on every
    change the file is written AND full snapshot upserted to `turso_webhooks`
    (fire-and-forget); on cold start with no local file, hooks are loaded from
    Supabase. Local file remains source of truth.
- **`src/routes.rs`:** `POST /v1/sync/{id}` — applies webhook payload statements to
  the target db with auth/tenant checks. Deliberately does NOT re-dispatch webhooks
  (one-hop replication ⇒ no loops). Returns `{applied, rows_affected}`.
- **`src/models.rs`:** `headers` on `CreateWebhookRequest`; new `SyncResponse`.
- **`src/main.rs`:** `WebhookStore::new(path, supabase.clone())`.
- **`Cargo.toml`:** version `1.1.0 → 1.2.0`.
- **`scripts/webhook-receiver.js`:** zero-dependency Node reference receiver that
  verifies `X-Turso-Signature` (HMAC, `timingSafeEqual`), logs events, exposes
  `/last` + `/count`, and `BAD=N` mode to exercise sender retries.
- **`DEPLOY.md`:** API row for `/sync`, updated webhook docs (retries, `changes`
  payload, custom headers, Supabase mirror, push-sync how-to).
- **Git history:** rewrote all 54 commits with `git filter-branch` + `gc
  --prune=now`; the four leaked literals are gone from the entire object database
  (verified via `cat-file` scan). Rewritten main is `ahead 3 / behind 0` of the
  unchanged `origin/main` ancestor → plain push suffices (no force needed).

### Tests (22 total, all pass) — fmt/clippy/build clean
Added: statement classifier (inserts incl. quoted/comment/OR REPLACE, update incl.
OR <behavior>, delete, ddl incl. IF NOT EXISTS, CTE→null), headers validation,
retry-until-success and give-up-after-backoff (in-process axum flaky/always-fail
handlers + 10ms backoffs), custom-header + `changes` assertions in the loopback test.

### Manual smoke (localhost:3100 + node receiver)
- Webhook → Node receiver: signature verified OK, `insert`+`update` ops recovered
  from a 2-statement batch.
- Flaky receiver `BAD=2`: exactly 3 HTTP hits (2×500 then 200) → retry works live.
- **Sync e2e:** webhook on dbA (`url=http://127.0.0.1:3100/v1/sync/<dbB>`,
  `Authorization` header) → writing 2 rows to A replicated them to B; no loop.

### Notes / limitations
- `changes` classifier is best-effort (no full SQL parser): CTE-prefixed writes and
  exotic quoting ⇒ `op=null`. Row-level *values* (which rows changed) still not
  captured — that's a change-tracking/framing layer, not statement parsing.
- Retry queue is in-memory per-process; a restart between attempts drops pending
  deliveries. Durable queue is future work.
- Supabase mirror upserts the full snapshot; orphans (webhooks removed locally)
  aren't deleted from Supabase. Deleting a DB clears the local entry only.
- Sync applies statements sequentially; on error it aborts with `SYNC_ERROR` (400)
  so the sender retries — at-least-once, apply idempotency is the operator's concern.

### Files changed
`Cargo.toml`, `Cargo.lock`, `DEPLOY.md`, `src/webhooks.rs`, `src/routes.rs`,
`src/models.rs`, `src/main.rs`, new `scripts/webhook-receiver.js`.
Committed locally; push deferred to the boss (Render secrets).

---

## 2026-08-29 — Webhook/sync feature (v1.1.0)

**Model/session:** opencode (big-pickle)

### Objective
Start the webhook/sync feature: notify external endpoints when data changes in a
database, and provide the base for push-style sync between turso-service instances.

### What was built
- **New `src/webhooks.rs`:**
  - `Webhook { id, url, secret, events, created_at }`, persisted to `DATA_DIR/webhooks.json`
    (file-backed store, same pattern as the DB manifest; survives restarts).
  - `WebhookStore::add/list/remove/remove_all`, validation of URLs (http/https) and
    event tokens (`write`, `*`).
  - `dispatch()`: fire-and-forget `tokio::spawn` per matching hook (does NOT block the
    write response); 10s request timeout; headers `X-Turso-Event`, `X-Turso-Database`,
    and `X-Turso-Signature: sha256=<hex HMAC-SHA256 of raw body>` when a secret is set.
  - Payload shape: `{ event, schema_version:1, delivery_id, timestamp,
    database:{id,name}, owner, statements, rows_affected }`.
- **Write-path wiring (both paths trigger webhooks):**
  - `POST /v1/databases/{id}/execute` → `DatabaseManager::execute` now returns an
    `ExecuteReport { statements, rows_affected }` (in `src/db.rs`); route dispatches
    a `write` webhook for non-empty statements.
  - libsql pipeline (`POST /v1/libsql/{db}/v2/pipeline`) — accumulates executed
    write statements across `execute`/`batch` requests and dispatches one webhook
    per pipeline call.
  - `split_sql` made `pub(crate)`; new `sql_is_query()` classifier shared by
    `run_statement` and webhook plumbing (read-only statements never fire webhooks).
- **Routes:** `GET|POST /v1/databases/{id}/webhooks`, `DELETE /v1/databases/{id}/webhooks/{hook_id}`;
  owner-only (tenant isolation via `check_db_owner`). DB deletion also clears its webhooks.
- **Deps:** added `hmac 0.12`, `sha2 0.10`, `hex 0.4`; version bumped `1.0.0 → 1.1.0`.
- **Docs:** DEPLOY.md webhook section + API table rows.

### Tests (16 total, all pass)
- New: HMAC known-vector, signature format, event matching, URL/event validation,
  store file roundtrip, payload shape, and a live loopback delivery test that spins
  up an axum receiver and asserts signed headers + payload.
- `cargo fmt --check`, `cargo clippy --all-targets`, `cargo build`, `cargo test` — all clean.

### Manual smoke (localhost:3100)
Register webhook (201), list (1), invalid URL→400, invalid event→400, multi-statement
execute fired-and-returned in ~114ms (non-blocking), delete→204, re-delete→404,
db delete cleared `webhooks.json` to `{}`.

### Notes / limitations (recorded intent)
- MVP fires one `write` event per write batch (all statements listed in payload).
  Row-level insert/update/delete detection is a planned enhancement (needs SQL
  analysis or change capture BEFORE execute).
- No retry/backoff queue yet — delivery is best-effort fire-and-forget.
- Supabase-backed webhook persistence not implemented (file-only for now).

### Files changed
- New: `src/webhooks.rs`
- Modified: `Cargo.toml`, `Cargo.lock`, `src/main.rs`, `src/routes.rs`, `src/db.rs`,
  `src/libsql.rs`, `src/models.rs`, `DEPLOY.md`
- Committed locally (see below).

---

## 2026-08-28 — Build fix, security pass, real columns, warnings cleanup

**Model/session:** opencode (big-pickle)

### Objective
Resume the project: repair broken local build, then security pass, real `/query` columns, warning cleanup, memory structure, and prepare next steps.

### Environment fixes
- `start.ps1` referenced dead paths (`D:\turso-target`, `C:\msys64\mingw64\bin`). Rewrote to:
  - Prepend `D:\Backups\Ruby40-x64\msys64\ucrt64\bin` to PATH (MinGW-w64 UCRT64 for the `x86_64-pc-windows-gnu` target).
  - Use `$env:RUSTUP_HOME="D:\rustup"`, `$env:CARGO_HOME="D:\cargo"`, cargo at `C:\Users\Sirkelvin Kamami\.cargo\bin\cargo.exe`.
  - Auto `cargo build` then run `target\debug\turso-service.exe`.
- Root cause of broken builds: the **ReadOnly attribute was set on 1273 directories** under the workspace, which broke `autocfg` build scripts. Attribute cleared on all of them.

### Security pass
- **New `src/google.rs`:** verifies Google OAuth ID tokens: fetches JWKS from `https://www.googleapis.com/oauth2/v3/certs`, cached 3600s, `DecodingKey::from_rsa_components`, `/jsonwebtoken` RS256 `Validation` (issuer `accounts.google.com` / `https://accounts.google.com`, audience from `GOOGLE_CLIENT_ID`).
- **`src/routes.rs`:**
  - `POST /v1/auth/google-token` — verifies ID token, provisions a user record (random password) if new, returns app JWT with `sub`/`email`/`name`.
  - `GET /v1/auth/google-config` — public, returns `{ client_id, enabled }` so the frontend can initialize the button.
  - Admin login: missing `ADMIN_PASSWORD` now returns HTTP 500 `CONFIG_ERROR` instead of `panic!` (previously `expect`).
- **`static/dashboard.html`:** Google sign-in button was never initialized — added `initGoogleSignIn()`; `handleGoogleSignIn` now sends `{ id_token: response.credential }`.
- **Secret scrub (were committed in git history/working tree):**
  - `render.yaml` — `JWT_SECRET`, `ADMIN_PASSWORD`, `SEED_USERS` replaced with placeholders and `sync: false`.
  - `DEPLOY.md` — rewritten; removed non-existent `/v1/auth/register`, documented real endpoints.
  - `static/das-creatives-setup.js`, `static/turso-client.js`, `admin.ps1` — removed hardcoded credentials.
  - `admin.ps1` now reads admin creds from `.env`; also fixed `list`/`query`/`users` argument parsing.
  - `.env.example` — documented new env vars.
  - Old values (JWT secret, admin password, seed user password) removed from files — literals are NOT recorded here; they live only in git history / old `.env`. **Rotate the actual credentials.**
  - **Still rotated? NO.** The actual credentials in Render/.env must be rotated by the owner.

### Real column names in `/query`
- `src/db.rs`: added `value_to_string(...)` helper (values → `"NULL"`/number/text/`<blob N bytes>`) and `query_with_columns(...)` that uses `rows.columns()` for names. `query(...)` refactored onto the helper.
- `src/routes.rs`: `run_query` calls `query_with_columns`. Verified live: `columns: [id, first_name, age]`, rows `[1,Alice,30]`, `[2,Bob,25]`.

### Warning / lint cleanup
- `cargo check` was 0 warnings; `cargo fmt --check` was failing (repo was never formatted; CI masks this with `|| true`) and `clippy` had 7 warnings.
- Applied `cargo fmt` (whole tree), then fixed all clippy findings:
  - db.rs: `format!("recovered-{}", id)` (redundant borrow); 2× `collapsible_if` → `if let ... && let ...`;
  - google.rs: `collapsible_if`;
  - libsql.rs: `collapsible_if` (and fixed an unbalanced brace from that edit);
  - plans.rs: `impl Default` → `#[derive(Default)]` + `#[default]`;
  - routes.rs: `sort_by` → `sort_by_key(Reverse(...))`.
- **Fixed a latent bug found during smoke tests:** `delete_database` left orphaned `-wal`/`-shm` files on disk; it now removes `{id}.db`, `{id}.db-wal`, `{id}.db-shm`.

### Verification
- `cargo fmt --check` → exit 0
- `cargo clippy --all-targets` → 0 warnings, exit 0
- `cargo build` → exit 0
- `cargo test --all-targets` → 0 tests, pass
- Manual end-to-end smoke test on port 3100 (temporary `BIND_ADDRESS=0.0.0.0:3100`): health OK, google-config OK, admin login OK, create DB, create table, insert 2 rows, query returned real columns + rows, delete. Cleaned up test databases and orphan files afterwards.

### Files changed
- New: `src/google.rs`
- Modified: `.env.example`, `DEPLOY.md`, `admin.ps1`, `render.yaml`, `src/analytics.rs`, `src/auth.rs`, `src/config.rs`, `src/db.rs`, `src/libsql.rs`, `src/main.rs`, `src/models.rs`, `src/plans.rs`, `src/routes.rs`, `src/supabase.rs`, `src/users.rs`, `start.ps1`, `static/das-creatives-setup.js`, `static/dashboard.html`, `static/turso-client.js`
- New (memory): `MEMORY/projects/turso-db/STATUS.md`, this log, lessons, daily report

### Unresolved / next
1. **Port conflict:** port 3000 is used by node/PID 6068 (another project's Next.js dev server). `localhost` → the other server. Decide whether to change `BIND_ADDRESS` or have the user stop that server.
2. **Render secrets:** `render.yaml` secrets are `sync: false`; owner must re-enter secrets in the Render dashboard before/after the next deploy.
3. **Uncommitted changes:** ~1,119 insertions across 19 files + new google.rs. Needs a commit (owner decision) and push; push triggers CI → Render deploy.
4. **No tests:** `cargo test` runs 0 tests. Should add unit tests (e.g. `value_to_string`, JWT/plan logic).
5. **Multi-statement execute:** `conn.execute(sql, ())` runs only the first statement in a `;`-joined string (observed in smoke test). Consider splitting statements.
6. **Next feature:** owner hinted at webhook/sync work — not yet specified.
7. Deprecated entries in `MEMORY\README.md` (auth.rs/users.rs/ratelimit.rs/analytics.rs vs actual modules) — minor doc drift.