# Patterns — Shared Best Practices (turso-db)

Reusable patterns for this project. Newest first.

## Local Rust build on this Windows machine (2026-08-28)

Every cargo invocation needs these environment pieces (toolchain is not on PATH):

```powershell
$env:PATH = "D:\Backups\Ruby40-x64\msys64\ucrt64\bin;" + $env:PATH   # MinGW-w64 UCRT64 (windows-gnu target)
$env:RUSTUP_HOME = "D:\rustup"
$env:CARGO_HOME = "D:\cargo"
& "C:\Users\Sirkelvin Kamami\.cargo\bin\cargo.exe" check   # build / fmt / clippy / test
```

- Do **not** rely on `cargo` being on PATH; call the full path.
- Ignore PowerShell "RemoteException" noise from cargo; use `$LASTEXITCODE`.
- Build output: `target\debug\turso-service.exe`. `start.ps1` already does build+run.
- Local run with non-default port (when 3000 is taken):

```powershell
$env:BIND_ADDRESS = "0.0.0.0:3100"; Start-Process "D:\turso-service\target\debug\turso-service.exe" -WorkingDirectory "D:\turso-service"
```

## Verify gate before commit (CLAUDE.md rules)

Run in order; all must exit 0:

```powershell
$env:PATH = "D:\Backups\Ruby40-x64\msys64\ucrt64\bin;" + $env:PATH
$env:RUSTUP_HOME = "D:\rustup"; $env:CARGO_HOME = "D:\cargo"
& "C:\Users\Sirkelvin Kamami\.cargo\bin\cargo.exe" fmt --check
& "C:\Users\Sirkelvin Kamami\.cargo\bin\cargo.exe" clippy --all-targets
& "C:\Users\Sirkelvin Kamami\.cargo\bin\cargo.exe" build
& "C:\Users\Sirkelvin Kamami\.cargo\bin\cargo.exe" test --all-targets
```

## Secrets

- Never commit `.env` or real credentials. `.env` is untracked (verify with `git status`); `.env.example` documents vars only.
- `ADMIN_PASSWORD` is read from `.env` only; missing → HTTP 500 `CONFIG_ERROR`.
- Removed a secret from a repo file? Rotate the actual credential too.
- Render secrets in `render.yaml` are now `sync: false` — change them in the Render dashboard.

## SQL / turso crate

- `conn.execute(sql, ())` runs only the first statement in a `;`-joined string — run statements individually.
- Prefer `query_with_columns` (returns real column names) over `query` (values only).
- `delete_database` removes `.db`, `.db-wal`, `.db-shm`.
- Local (non-Supabase) persistence: files in `DATA_DIR` + `databases.json` manifest; orphan `*.db` files are recovered at startup as owner `admin`.

## Request flow (auth)

- JWTs signed with `JWT_SECRET`; `authenticate(&headers)` extracts claims; `check_db_owner` enforces tenant isolation; `check_user_rate_limit` enforces plan quotas. Written-new Google path: `/v1/auth/google-token` (app JWT issued after JWKS-verified Google ID token), `/v1/auth/google-config` (public client_id).