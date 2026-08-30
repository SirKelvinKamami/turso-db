# Lessons — Retries, classifiers, and loop-free sync (2026-08-31)

## Backoff loop shape (off-by-one)
A retry loop that sleeps at the END of each iteration yields `len(backoffs)` attempts,
not `len+1`. Shape it as "try first, then for each delay { sleep; try }" so N backoffs
really mean N+1 attempts. The bot's first version made a flaky-receiver test never reach
success; the corrected implementation (try → for delay { sleep → try }) matched reality.

## Best-effort SQL classification is fine, if you say so
Extracting `op`/`table` without a real parser is doable with keyword scanning, but the
edge cases multiply fast: `UPDATE OR IGNORE`, `CREATE TABLE IF NOT EXISTS`, quoted
identifiers, comments, schema-qualified names, CTE-prefixed INSERTs. Be explicit that
CTEs and exotic shapes yield `op=null` rather than pretending precision. Document the
limitation; charge full change capture to a change-tracking layer instead.

## Loop-free replication: one hop, done
If the sync receiver re-fires webhooks, A→B→A→B spins forever. The clean rule: the sync
endpoint applies statements but does NOT re-dispatch. Replication is always one hop from
the authoritative source; for N replicas you register every replica's sync URL as a
webhook on the source (star fan-out) rather than chaining. Simple, deterministic, and it
survives as the resilience story until a real replica protocol exists.

## Verified live beats verified in-loop
The in-process axum loopback test proves signing/headers/payload, but the two-machine
shape (real sender → real Node receiver) caught nothing extra — it converted the
documented behavior into confidence. Worth the 20 lines of PowerShell + a stdlib Node
script (`BAD=N` mode doubles as a retry test harness).

## Custom headers change the security surface
Sending arbitrary per-webhook headers (to support `Authorization: Bearer <token>`) means
a credential can sit inside `webhooks.json`. That's stored next to the DB manifest, not
in the clear except on disk where the DBs already live — acceptable here, but call it out
and validate header names/values at creation so injection-shaped names are rejected.

## History rewrite without filter-repo
No Python on the machine, but Git-for-Windows ships `sed`/`bash`: `git filter-branch
--tree-filter 'sed -i ...'` over `--all` + dropping `refs/original` + `reflog expire` +
`gc --prune=now` scrubbed the literals portably. Verification matters: scan the WHOLE
object DB (`git cat-file --batch-all-objects`), not just reachable commits, because the
leaked literals lived in a dangling amended commit, not in branch history.

## Git history is NOT where the leak lived
After the rewrite, branch history turned out to contain no plaintext secrets — the
exposure was a dangling object from an amended commit plus the untracked `.env`. The
amended-away commit is exactly why "I scrubbed the file before committing" is insufficient:
the previous version stays in the object DB until `gc --prune=now`.