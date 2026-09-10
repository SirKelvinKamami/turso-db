# Turso Service — Current Status

**Last Updated:** 2026-09-10 (v1.7.1)
**Version:** 1.7.1 (libsql replica sync → hub base remote + hub service on Render)

---

## Where We Are

- v1.7.0+ **live on Render** (`https://turso-db-8svn.onrender.com`): change-framing,
  webhook PATCH, replica sync client. Health: `version: 1.7.0`, `supabase://public.turso_users`,
  3 users. Earlier deploy failures fixed (glibc mismatch → `debian:trixie-slim` runtime).
- **v1.7.1 (this batch):**
  - **Sync remote = hub base** (no `/{db-slug}`). Current sqld routes every
    replication/Hrana endpoint at the server root and resolves the target DB from
    auth/namespace negotiation; a path segment hit the 404 fallback.
    `sync_remote_url(hub_base)` now returns the trimmed base; test reworded.
  - **libsql hub service added to `render.yaml`** (`turso-db-hub`, docker,
    `ghcr.io/tursodatabase/libsql-server`, JWT auth via `SQLD_AUTH_JWT_KEY`,
    `/health` probe, listener 0.0.0.0:8080, DB `data.sqld`). Free plan → hub DB
    data is **ephemeral**; upgrade to paid + disk before flipping prod
    `SYNC_ENABLED=true`.
  - **Hub auth verified end-to-end at protocol level:** the turso sync engine sends
    `Authorization: Bearer <token>`; sqld accepts **EdDSA JWT** (`SQLD_AUTH_JWT_KEY`
    = base64url Ed25519 public key; `exp` optional; empty claims = full write on the
    default namespace). Legacy `SQLD_HTTP_AUTH` Basic is rejected (Bearer scheme).
    Minted a 5-year EdDSA JWT + public key (helper in temp; SEED_HEX backed up for
    renewal); token round-trip VERIFY=OK against sqld's exact decode path.
- **Multi-DB sync is NOT yet safe for prod:** with namespaces disabled, every sync DB
  converges into one `default` namespace. Staging proof uses a single test DB. Per-DB
  namespaces (`--enable-namespaces` + provisioning) is a follow-up decision before any
  prod flip touches real tenant DBs.
- **Credential rotation still outstanding** (manual, boss): old JWT/admin/seed/Google
  values; ensure Render dashboard passwords are genuinely new.

## What Is Uncommitted

Nothing — v1.7.1 committed + pushed (`af92ddf`), CI green. Render auto-deploy fired
(hub service provisioning in flight).

## Blockers / Decisions Needed

- **RENDER_API_KEY** (boss) — to set `SYNC_HUB_TOKEN` + flip `SYNC_ENABLED` on the
  prod service via API (or do it in the dashboard).
- **Set `SQLD_AUTH_JWT_KEY` on the Render hub** (dashboard/API) so replicas can
  authenticate; public key value is in the session's temp handoff file.
- **Multi-DB hub namespaces** design/decision before any prod `SYNC_ENABLED=true`
  with multiple real databases.
- **Credential rotation** (manual, boss) — oldest open item.

## Next Steps

1. Confirm `turso-db-hub` on Render is healthy (dashboard).
2. Set `SQLD_AUTH_JWT_KEY` (hub) → get hub public URL.
3. Local staging: two turso-service instances with `SYNC_ENABLED=true` + hub URL/JWT,
   single test DB each → prove bidirectional LWW convergence.
4. Post-gate: set prod `SYNC_HUB_URL`/`SYNC_HUB_TOKEN`, then enable sync only after
  the multi-DB namespace plan is decided.

## Test Notes

- `cargo test --all-targets` → **78 tests**, all pass; fmt + clippy (`-D warnings`) clean.
- v1.7.1: `sync_remote_url_is_normalized_hub_base` rewords the old slug test.