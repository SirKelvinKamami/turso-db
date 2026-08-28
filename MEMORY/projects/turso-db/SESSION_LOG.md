# Turso Service — Session Log

Log of AI working sessions. Newest first.

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