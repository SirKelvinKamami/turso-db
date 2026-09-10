# 2026-09-10 — Docker builder/runtime glibc mismatch on Render (v1.7.0 deploy)

## Symptom
- Build succeeded (all docker steps cached/green, CI green, deploy hook fired), but at
  startup the service exited with status 1:
  ```
  ./turso-service: /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.38' not found
  (required by ./turso-service)
  ```
- Render kept serving the last good deploy (1.6.0) and reported a failed deploy.

## Cause
The multi-stage Dockerfile built with `rust:1.97` but ran the binary on
`debian:bookworm-slim` (glibc 2.36). Newer `rust` image variants link against a newer
glibc (≥ 2.38), so the produced binary carries symbol version requirements the old
runtime can't satisfy. The binary is dynamically linked — the runtime image's libc must
be ≥ the builder's.

## Fix
Match (or exceed) the runtime glibc: `FROM debian:trixie-slim` (glibc 2.41).
Bookworm-slim is a frequent default, but the builder base decides the symbols.

## Reusable rules
- The deploy-time failure is invisible to CI (Rust builds have no glibc dependency
  guess) — CI cannot catch glibc-vs-runtime mismatches.
- When a new dependency or a base-image bump suddenly adds a `GLIBC_x.x not found`
  startup error, first check the builder image's base glibc vs the runtime stage.
- Safer long-term options: fully static musl build
  (`x86_64-unknown-linux-musl` + `musl-tools`) or pin both stages to the same distro
  release. Static linking avoids symbol version drift entirely.