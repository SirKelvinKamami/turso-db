# Turso Service — Current Status

**Last Updated:** 2026-09-10 (v1.7.0)
**Version:** 1.7.0 (libsql replica sync, bidirectional LWW)

---

## Where We Are

- Build environment healthy on the dev machine (Windows / windows-gnu).
- v1.6.0 live on Render (`https://turso-db-8svn.onrender.com`): change-framing,
  webhook PATCH. Health verified.
- **v1.7.0 (this batch):** **live on Render** (health: `version: 1.7.0`,
  `supabase://public.turso_users`, 3 users). First deploy failed with
  `GLIBC_2.38 not found` — newer `rust:1.97` builder links a newer glibc than the
  `debian:bookworm-slim` runtime; fixed by switching the runtime stage to
  `debian:trixie-slim` (glibc 2.41).
- **v1.7.0 (this batch):**
  - **LibSQL replica sync implemented** (per `SYNC_PLAN.md`, boss decisions: hub =
    self-hosted libsql-server, bidirectional LWW, token via `env://` = `SYNC_HUB_TOKEN`).
    - Opt-in, off by default: `SYNC_ENABLED=false`, `SYNC_HUB_URL`, `SYNC_HUB_TOKEN`,
      `SYNC_POLL_MS` (default 5000, min 250).
    - Each opened/created database becomes `DbHandle::Local | Replica{turso::sync::Database}`;
      remote URL = `{hub}/{name-slug}`; hub namespace bootstrapped on first connect.
    - Single-task supervisor: pull → push → checkpoint every 30 ticks (LWW in both
      directions), safe no-op when disabled.
    - Degraded mode: unreachable hub at open → warn + local-only (service keeps serving).
  - **Non-`Send` sync fix:** `turso::sync::Database` is `Send` but NOT `Sync`, and its
    `connect()` future is not `Send` (uses `&self` after an internal await). Handler
    futures are kept `Send` by running `connect()` on a blocking thread with a fresh
    current-thread runtime (`tokio::task::spawn_blocking` + `Runtime::block_on`);
    guarded by a compile-time `const fn _assert_send` check.
  - Test count 75 → **78**; build + release build + fmt + clippy(`-D warnings`) clean.
- **Credential rotation still outstanding** (manual, boss): old JWT/admin/seed/Google
  values; ensure Render dashboard passwords are genuinely new.
- Render auto-deploy on push via CI `RENDER_DEPLOY_HOOK_URL`.

## What Is Uncommitted

Nothing — this batch is committed and pushed to `origin/main` (CI green + Render
deploy fired). Leak-literal scan clean before commit.

## Blockers / Decisions Needed

- **Credential rotation** (manual, boss) — oldest open item.
- **Hub deployment for production sync** — v1.7.0 ships the client side; enabling it
  needs a libsql-server hub (Render env: `SYNC_ENABLED`, `SYNC_HUB_URL`, `SYNC_HUB_TOKEN`).
- **Where does data live in prod?** Supabase set on Render (users + db files + analytics
  + webhooks); 1GB Render disk is a caching layer.

## Next Feature Candidates

- Provision/run the self-hosted libsql hub (docker) and flip `SYNC_ENABLED=true` in
  staging first, then Render.
- Real sync smoke test against the hub e2e (create on A → LWW converge on B).
- Wire change-frames into the libsql pipeline path (currently passes empty frames).
- Per-DB replica stats surface (`stats()`/staleness) for operator visibility.

## Test Notes

- `cargo test --all-targets` → **78 tests**, all pass.
- New in v1.7.0: `sync_remote_url_slugs_db_names`, `sync_config_defaults_to_disabled`,
  `open_handle_is_local_when_sync_disabled` (sync open requires a live hub; local
  fallback asserted instead).
- Kept: full v1.6.0 framing/rollback/patch suites (75) + integration harness.