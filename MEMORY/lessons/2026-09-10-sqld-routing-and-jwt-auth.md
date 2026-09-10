# Lesson: sqld (libsql-server) routing + auth for the turso replica sync engine

**Date:** 2026-09-10
**Project:** turso-db

## Facts (verified in source)
- Modern `sqld` (libsql-server main) mounts every replication/Hrana endpoint at the
  server **root** (`/`, `/v2/pipeline`, tonic-web gRPC services, `/health`). Unknown
  prefixed paths fall through `.fallback(handle_fallback)` → 404.
- The turso sync engine builds requests as `{base_url}{path}` and the path comes from
  a bundled opaque C SDK — so don't put a database path segment in the remote URL.
  Remote URL for a self-hosted hub = the hub base only.
- sqld namespace selection: namespaces disabled by default → everything is the single
  `default` namespace. `--enable-namespaces` + per-namespace provisioning (admin API
  or JWT `id` claim) enables multi-DB. Namespace-per-DB on the hub requires
  accommodation that our service does not yet do.
- Auth: `SQLD_AUTH_JWT_KEY` (or FILE) = EdDSA decoding keys; `exp` claim optional;
  a JWT with empty claims grants full write access on the default namespace.
  `DecodingKey::from_ed_components` accepts raw 32-byte public key base64(URL).
- The turso client sends `Authorization: Bearer <token>` → **only JWT auth works**
  with it; sqld's legacy `SQLD_HTTP_AUTH` Basic scheme is Bearer-incompatible.
- `SQLD_HTTP_LISTEN_ADDR` defaults to `127.0.0.1:8080` — must be `0.0.0.0` when
  containerized/Render.

## How to mint a hub token without new deps
1. Tiny standalone Rust tool using cached crates `ring` + `base64`:
2. `Ed25519KeyPair::from_seed_unchecked(&seed)` (ring 0.17 has no `as_ref()->PKCS8`,
   so hand-roll the JWT: `b64u(header).b64u(payload).b64u(sign)`).
3. header `{"alg":"EdDSA","typ":"JWT"}`, payload `{"exp": <ts>}`.
4. `SQLD_AUTH_JWT_KEY` = base64url(public_key), `SYNC_HUB_TOKEN` = the JWT.
5. Verify with `jsonwebtoken::decode` using `DecodingKey::from_ed_components` +
   `Validation::new(Algorithm::EdDSA)` (with `exp` removed from required) — this is
   sqld's exact path. Keep the seed (`SEED_HEX`) to re-issue tokens.

## Gotchas
- Render docker services: only the Dockerfile `EXPOSE` port is routed; no second
  public port for the admin API — plan admin/namespace provisioning accordingly.
- Render free instances have no persistent disk — hub SQLite data is ephemeral;
  replicas re-push after hub wipe, but don't run prod sync on free.
- Node-20 GA runner deprecation is a warning only; CI still green.