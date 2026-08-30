# Turso Service — Session Log

Log of AI working sessions. Newest first.

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