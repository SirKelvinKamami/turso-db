# Turso Service — Full libsql Replica Sync Plan

**Status:** PLANNING (no code written yet)
**Owner decision needed before implementation** (see [Open questions](#open-questions))
**Last updated:** 2026-09-10 (v1.6.0 baseline)

---

## 1. Goal

Replace the statement-forwarding push-sync (`POST /v1/sync/{id}` + webhooks) with a
real libsql embedded-replica protocol so multiple turso-service instances/sites can
stay converged on the same logical database with row-level change replication.

Current production sync is "one hop, apply statements, never re-dispatch" (see
`SESSION_LOG` 2026-08-31). It is idempotency-by-operator, statement-ordered, and
fragile when statements don't replay cleanly or conflict. This plan moves to Turso's
native sync engine at the storage layer.

## 2. What turso 0.7.2 already gives us (verified in `vendor` copy)

`cargo registry src/index.crates.io-*/turso-0.7.2/src/sync.rs` exposes a complete
synced-database API:

- `turso::sync::Builder::new_remote(path)` with:
  - `.with_remote_url(url)` (normalized via `normalize_base_url`, e.g. `libsql://host`
    or `https://host`), loaded from the sync metadata file when omitted
  - `.with_auth_token(token)` or `.with_auth_token_fn(...)` — token can rotate per
    request (callback runs before every HTTP request)
  - `.with_client_name(name)` (sync peer identification)
  - `.with_long_poll_timeout(Duration)` (waiting loop for remote changes)
  - `.bootstrap_if_empty(bool)` (download schema + initial data on first open)
  - experimental engine-feature mirrors (attach / without_rowid / custom_types / …)
  - option overrides: `with_remote_encryption(_key, cipher)`, partial sync
    (`with_partial_sync_opts_experimental`), logical-MVCC-pull toggle
- `turso::sync::Builder::build()` → `turso::sync::Database`:
  - `push()` — send local changes to the remote
  - `pull()` — long-poll wait + apply remote changes, returns `bool` (changed?)
  - `checkpoint()` — force WAL checkpoint of the main db (retries on sync-busy)
  - `stats()` — `DatabaseSyncStats`
  - `connect()` → the same `turso::Connection` type used by the local builder, so the
    entire existing `run_statement` / framing / value encoding stack works unchanged.
- Sync uses Turso's logical MVCC; concurrent writers converge at row level
  (lamport/LWW ordering; last-writer-wins per row unless the merge applies deltas).

**Key integration fact:** `turso::Database` (local-only) and `turso::sync::Database`
are **different types**. `DatabaseManager.databases` currently stores
`(turso::Database, DatabaseEntry)` (db.rs:202). The sync engine is a separate handle,
not a builder flag on the local one — so a replica either gets its own map/entry type
or we wrap both behind an enum/arc (recommended below).

## 3. Where it plugs into the current code

Current open path in `DatabaseManager` (db.rs): `Builder::new_local(&path).build()`
at manifest load, database create, and recover. Every `/execute` and the libsql
pipeline call `db.execute(...)` on that `turso::Database`.

Proposed integration points (unchanged public surface):
- `DatabaseEntry` gains per-db sync metadata (`replica: { kind, remote_url,
  auth_token_ref }`); persisted in `databases.json`/Supabase manifest.
- New `SyncBackend` owned by `DatabaseManager`: keeps `DashMap<id, turso::sync::Database>`
  for databases configured as replicas, and a supervisor task per replica that loops
  `pull()` → checkpoint on idle.
- Database create gains an optional `replica_url` / token body so `POST /v1/databases`
  can start replicas directly (see Open questions — token handling is a security
  decision first).
- Write path (both `/execute` and libsql pipeline): **unchanged** up to commit; after
  `COMMIT` we call `sync_db.push()` for replica-backed databases. Push is off the
  request path (spawn) like webhook dispatch, so response latency does not include
  network sync.
- Read path: served from the local replica file (fresh as of last `pull()`), with an
  optional `staleness` cap using `stats()` (see Conflicts/failures).

## 4. Proposed design

### 4.1 Config (opt-in, per database)

```bash
# instance-level defaults
SYNC_ENABLED=false            # global kill switch (feature flag)
SYNC_POLL_MS=5000             # supervisor long-poll cadence per replica
SYNC_MAX_REPLICAS_PER_INSTANCE=100
# per-database (create request or manifest): { "replica": { "url": "libsql://...",
#   "auth": "<token source>" } }
```

Auth token source is deliberately a **reference, not the literal**:
`env://SYNC_TOKEN_<DBID>` or a Supabase-backed secret lookup, so replica tokens never
land in `databases.json`/webhook payloads. (Matches the standing rule that the four
leaked literals never re-enter this repo; new sync tokens get equivalent
scrub/rotation guards.)

### 4.2 Replica lifecycle

1. `CreateDatabase(overrides)` → `sync::Builder::new_remote(path).with_remote_url(url)
   .with_auth_token_fn(stored-lookup).bootstrap_if_empty(true).build()`.
   Failure to reach the primary at create time = 400 with a clear error; no silent
   local-only fallback at create.
2. Supervisor: per replica, `loop { if sync_db.pull() { stats/checkpoint } sleep }`,
   spawned on `tokio`, exiting cleanly on db delete. Stops pulling while
   `SYNC_ENABLED=false`. Long-poll backend already handles incremental pulls — a busy
   loop is NOT needed.
3. Delete: `sync_db` handle dropped + file cleanup (reuse existing orphan cleanup for
   `-wal`/`-shm`).
4. Cold start: `load_manifest` opens replica databases via the sync builder instead of
   the local builder; if offline, open local-only (degraded reads) and let the
   supervisor re-establish.

### 4.3 Write-path ordering (the important bit)

Our change-framing already wraps write batches in `BEGIN IMMEDIATE … COMMIT`
(db.rs `frame_and_apply`, v1.6.0). For replica databases:

- Keep the transaction + framing exactly as is — it operates on the connection.
- After `COMMIT`, `push()` the local changes. Ordering guarantee: push happens after
  the local commit is durable, so a crash between commit and push is just "changes
  not yet sent" and the next `pull()`/`push()` cycle reconciles.
- `push()` failures are retried by the supervisor and logged as security-relevant
  events (per CLAUDE security rules), but never block the HTTP response.

### 4.4 Conflict semantics (be explicit)

- **Primary/secondary ("read replica") mode (default):** only the primary accepts
  writes; replicas are strongly eventual readers. No conflict surface. This is the
  recommended first release and is a drop-in upgrade path from today's webhook sync.
- **Bidirectional mode (later):** both sides push. Convergence is row-level
  last-writer-wins with Turso lamport ordering; whole-batch atomicity is NOT
  guaranteed across replicas (a two-row transaction can interleave with another
  writer's). Document this loudly before enabling; recommend LWW-safe schemas.
- **Framing + sync interplay:** webhook `frame` snapshots reflect the local replica's
  pre/post rows; downstream consumers see per-replica writes, not merged remote
  writes. Replicas DO NOT dispatch webhooks for remotely-applied writes (keeps the
  no-loop rule; `pull()` writes bypass `frame_and_apply` entirely).

## 5. Migration & coexistence

- Keep `POST /v1/sync/{id}` for the transition window — it still serves legacy peers.
- Replica databases hold their own copy; the webhook/push-sync config for that database
  can be dropped once all peers run a replica instance.
- One-way rule from v1.2.0 holds for replicas: the primary never registers a replica's
  sync URL as a webhook (loop-free).

## 6. Testing plan

- **Local two-instance e2e:** instance A (primary, `:3100`), instance B (replica,
  `:3101`) → create db on A with replica url pointing at B's turso URL; write on A;
  poll B's `execute`/query until the row appears (assert eventually, with timeout).
- **Reverse direction:** forbid/verify writes to a primary/secondary replica are
  rejected or pushed then re-converged (decision: reject by default in v1).
- **Degrade:** kill A; B keeps serving reads; A returns → supervisor re-establishes,
  states converge. Assert no duplicate the webhook/sync path would have produced.
- **Framing regression:** existing 75 tests still green — replica DBs do not change
  `/execute` semantics; add a unit test that `frame_and_apply` on a sync connection
  yields the same `StatementFrame`s as local.
- **Conflicts (bidirectional phase):** concurrent writers on A and B updating the same
  row → both push → final row deterministic per LWW and `changes[]` frames on each
  side reflect their own writes.
- **Token security test:** assert sync tokens never appear in `databases.json`,
  webhook payloads, or any public route response (grep-guard in tests like the CI
  leak scan).

## 7. Rollout gates (project rules)

1. Implement behind `SYNC_ENABLED=false` default; staging deploy first.
2. `cargo fmt`/`clippy -D warnings`/`cargo test --all-targets` green on every phase.
3. **Boss approval before production**; Render env gets `SYNC_ENABLED=true` only then.
4. Version bump per release; CHANGELOG + DEPLOY.md + MEMORY STATUS update in the same
   commit; push → CI → Render; verify health version + a replica smoke on Render.
5. Leak-literal scan before every commit; rotate any token that ever appeared in a
   log/payload.

## 8. Known risks / non-goals (v1)

- **Multi-process WAL + push interplay:** sync uses its own IO worker thread; the
  existing `experimental_multiprocess_wal` flag on replicas is unverified — keep
  replicas single-process in v1.
- **Encrypted remotes / at-rest encryption** on the replica path is not supported by
  the sync engine (local encryption intentionally omitted per crate docs). If
  required, it belongs to the cloud side.
- **Cross-region latency** sets pull cadence; no push batching beyond per-commit
  `push()` in v1.
- **Bidirectional atomicity** is explicitly out of scope for v1 (see conflicts).

## 9. Open questions (need boss)

1. Is the primary turso backend on Turso Cloud (so replica URLs/tokens exist to
   consume), or would the "primary" be another self-hosted turso-service instance
   (needs a libsql server / embedded replica server between)?
2. Where do replica auth tokens live (`env://`, Supabase secret, sealed value)?
3. Bidirectional sync: desired now or "read replica first"?
4. Confirm staging (Render preview) as the first replica deployment target.

---

*Author: opencode (big-pickle), 2026-09-10. Baseline: v1.6.0 (change-framing + PATCH).*