# Lessons

Shared, hard-won lessons from sessions. Newest first.

## 2026-08-28 (turso-db)

### Windows automated builds are fragile
- Toolchain lives at `D:\rustup` / `D:\cargo` (cargo binary at `C:\Users\Sirkelvin Kamami\.cargo\bin\cargo.exe`) and MinGW-w64 for the `windows-gnu` target is at `D:\Backups\Ruby40-x64\msys64\ucrt64\bin`. Cargo must be invoked by full path — it is **not** on PATH — and `RUSTUP_HOME`/`CARGO_HOME` must be set each shell. Save this as a reusable snippet.
- The **ReadOnly directory attribute breaks `autocfg`** (a cargo build-script dependency). If builds suddenly fail with autocfg/link errors, check `attrib` on the workspace tree. Clear with: iterate over `Get-ChildItem -Directory -Recurse` and set attributes to `Normal`.
- PowerShell reports cargo's stderr writes as `RemoteException` "errors" — ignore them; judge success by `$LASTEXITCODE`.

### Secret hygiene
- Credentials (JWT secret, admin password, seed passwords) were committed in `render.yaml`, `DEPLOY.md`, `admin.ps1`, `static/*.js`. Scrub files, but **scrubbing files does not rotate the secret** — the owner must rotate the actual credential and any third-party surface (Render) that used the old value.
- When a Render `render.yaml` secret value is removed, set `sync: false` so deploys don't overwrite dashboard values, and plan to re-enter secrets manually.

### SQLite / turso crate gotchas
- The `turso` crate's `conn.execute(sql, ())` executes only the **first** statement of a `;`-joined string. Document multi-statement behavior; run statements individually.
- Deleting a libSQL DB leaves orphaned `-wal`/`-shm` files unless the code removes them explicitly.
- Real column names require using the result-set `columns()`; turso's default API may return positional names.

### Process / workflow
- `cargo fmt` had never been applied (CI masked it with `cargo fmt --check || true`); a fresh `cargo fmt` produces large diffs. Apply it deliberately, then keep `--check` in the loop.
- Always smoke-test with a **non-conflicting port** via `BIND_ADDRESS` env override — port 3000 here is owned by an unrelated Next.js dev server.
- `query_tracker`/rate limiting fire on real requests; a smoke test that runs queries exercises those paths too.