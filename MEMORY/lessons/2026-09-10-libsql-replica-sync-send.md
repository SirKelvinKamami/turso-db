# 2026-09-10 — Send-typed async work in axum handlers (replica sync)

## Context
Adding `turso::sync` replicas to `DatabaseManager` broke the axum `Handler` bound for
exactly the handlers that awaited the statement paths (`execute_query`, `run_query`,
`sync_database`, `setup_database`, `pipeline_handler`). The crate itself compiled; the
routing macro rejected the futures as non-`Send`.

## Root cause (two layers)
1. `turso::sync::Database` is `Send` but NOT `Sync`, and its `connect()` future is
   **not** `Send`: the body uses `&self` *after* an internal await (the connection's
   `extra_io` driver callback clones `self.io` post-result), so the async fn captures
   `&self` across the await. Awaited from a handler path → non-`Send` future.
2. A `Result<Connection, Box<dyn StdError>>` from `connect_to().await` used in
   `if let Ok(conn) = … { … .await … }` holds the whole `Result` (whose error arm is a
   non-`Send` `Box<dyn Error>`) across the inner await, because a match scrutinee's
   temporary drop scope spans the entire `if let`.

## Fixes that worked
- Run the non-`Send` future on a dedicated thread with its own current-thread runtime:
  ```rust
  tokio::task::spawn_blocking(move || {
      let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
      rt.block_on(open_replica_connection(&db)).map_err(|e| e.to_string())
  }).await??;
  ```
  `Runtime::block_on` accepts non-`Send` futures; the sync engine wakes via its own IO
  worker thread.
- Match on `Result` directly and early-return on `Err` so the non-`Send` arm drops
  before any later await (don't rely on `if let`/`.ok()`).
- Regression guard (compile-time, cheap):
  ```rust
  const fn _assert_send<T: Send>() {}
  const _: () = { _assert_send::<SyncDatabase>(); _assert_send::<turso::Connection>(); };
  ```
- Diagnose route futures fast: add `#[axum::debug_handler]` (needs `axum` feature
  `macros`) — it names the non-`Send` type and the exact line.

## Reusable rules
- `Box<dyn Error>` is NOT `Send`. Keep errors out of anything held across an await in a
  handler-reachable future (`Box<dyn Error + Send>`, `String`, or match/early-return).
- A borrowed handle (`&MutexGuard`, `&T`) or a method-`self` async fn keeps its borrow
  across the whole body — if the type isn't `Sync`, the future isn't `Send`.
- Matching temporaries live through the whole match/if-let statement; route non-`Send`
  arms out before any inner `.await`.
- Compile-time trait assertions (`const fn` + `const _`) are clippy-clean dead-code-free
  guards against dependency bumps flipping `Send`/`Sync` silently.