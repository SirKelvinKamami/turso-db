# Lessons — Change-framing, PATCH null semantics, sync-planning (2026-09-10)

## A plain `Box<dyn Error>` across an await silently kills your axum handlers
Change-framing needed an error type that survives an `.await`. The first draft returned
`Box<dyn std::error::Error>` from a helper held across the `COMMIT` await → the futures
became non-`Send`, and axum's `Handler` impl requires `Send` futures. The symptom is
misleading: "the trait bound `Handler<…>` is not satisfied" for EVERY handler awaiting
that function, none of which obviously depend on your new code. `cargo check` does not
flag it as a Send issue. Fix: keep the error `+ Send` where it must cross awaits
(`FrameError(String)` that is `Send`-by-construction), and only coerce to the classic
`Box<dyn Error>` signature at the routing boundary — the callback-based
`map_err(|e| -> Box<dyn std::error::Error> { e })` coercion keeps public API unchanged.

## This serde (1.0.229) moved the visitor error to a method generic
The classic `deserialize_option` double-option visitor was written against
`type Error` + `visit_none()/visit_some()` returning `Result<_, Self::Error>`. This
serde generates `fn visit_none<E>(self) -> Result<Self::Value, E>` — the error is a
per-method type parameter, so `Self::Error` fails with "no associated type" (or a
`E0308` mismatch). When vendored serde source lives under
`src/core/de/mod.rs` (not `src/de/mod.rs`), your crate is on the new trait shape.
Write `fn visit_none<E>(self) -> Result<Self::Value, E>` and
`fn visit_some<D2>(self, d: D2) -> Result<Self::Value, D2::Error> where D2: Deserializer<'de>`
and return `T::deserialize(d).map(|v| Some(Some(v)))`.

## "Explicit-null clears" needs a real visitor, not `Option<Option<T>>` nesting alone
`#[serde(default)] Option<Option<T>>` unwraps JSON `null` as `None` (absent), which is
exactly what you do NOT want for PATCH clear semantics — both "absent" and "null" would
mean "leave unchanged". Only a custom `Visitor` distinguishes them: deserialize the
outer option normally, but make the inner deserialization explicit via `visit_none` →
`Some(None)` (clear) while delegating values to `T::deserialize`. Document the three-way
contract on the API (omit / null / value), or consumers will trust `Option<Option<T>>`
intuition and burn you in code review.

## RETURNING rowid, * is your only engine-honest before/after, and it has holes
`INSERT/UPDATE … RETURNING rowid, *` gives real post-write rows and the pre-write
`SELECT rowid, * … WHERE <original where>` gives real matched rows — but `WITHOUT ROWID`
tables have no implicit `rowid`, so both queries fail and the whole frame becomes
impossible. Design frames to degrade honestly (skip the `frame` key) rather than
forcing `*`/implicit-rowid guesses; keep the statement-parsed `values` as the
independent, always-there fallback. Cap rows per frame — a no-WHERE `UPDATE`/
`DELETE` sweep would otherwise serialize the whole table into every payload.

## Batch transactionality is a feature, so make it a test
Wrapping the write batch in BEGIN IMMEDIATE…COMMIT (for framing) turns partial-apply
into all-or-nothing — a behavior change users will rely on. Assert it
(`batch_rolls_back_entirely_on_error`): roll back a multi-statement batch on a late
failure and confirm earlier statements took no effect. Also probe the engine feature
you depend on (`returning_clause_supported`) as a permanent regression test so a turso
dependency bump that drops RETURNING support is caught at test time, not in prod.

## Replica planning before code: verify the crate's sync surface first
The project's SYNCPLAN assumed Turso's embedded replication might not exist yet in
turso 0.7.2 — it ships a full `turso::sync` engine (`Builder::new_remote`,
`push`/`pull`/`checkpoint`/`stats`/`connect`, auth-token callbacks, long-poll),
so the "biggest remaining feature" is mostly wiring plus product decisions. Reading
the vendored crate (`cargo registry/.../turso-0.7.2/src/sync.rs`) before writing the
plan paid for itself: the plan's architecture, the type split
(`turso::Database` vs `turso::sync::Database`) that forces a `DatabaseManager` change,
and the write-path `commit → push()` ordering are all grounded in real API instead of
guesswork. Planning docs should end with discrete decisions for the owner, not a
build-it-all toast.