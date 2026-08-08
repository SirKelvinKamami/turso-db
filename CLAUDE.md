# turso-db — Rules for ALL AI Sessions

## Super AI Integration

This project is managed by Super AI. All AI sessions must follow these rules.

---

## Session Start (MANDATORY)

1. Read `MEMORY/README.md` — project overview
2. Read `MEMORY/projects/turso-db/` — current status
3. Check `MEMORY/patterns/` — shared best practices
4. Read `C:\Users\sirke\OneDrive\Documents\Alufacade World ERP System\super-ai\SESSION_PROTOCOL.md`
5. Check `C:\Users\sirke\OneDrive\Documents\Alufacade World ERP System\super-ai\ALERTS.md` — any issues

---

## Session End (MANDATORY)

1. Update `MEMORY/projects/turso-db/SESSION_LOG.md`
2. Add lessons to `MEMORY/lessons/`
3. Update `super-ai/PROJECTS.md` if status changed
4. Record tests, files changed, and unresolved issues in the session log

---

## Code Rules

- Follow Rust best practices (rustfmt, clippy)
- Use `cargo fmt` and `cargo clippy` before committing
- Write tests for new functionality
- Document public APIs

---

## Database Rules (Turso)

- Never expose database credentials
- Use environment variables
- Validate all inputs
- Use parameterized queries

---

## Deployment

- All changes go to staging first
- Get approval before production
- Run `cargo test` before deploy
- Update version in Cargo.toml

---

## Security

- Never commit `.env` files
- Use strong JWT secrets
- Rate limit all endpoints
- Log security events

---

**Last Updated:** 2026-08-08
**Authority:** SirKelvin Kamami (Boss)
