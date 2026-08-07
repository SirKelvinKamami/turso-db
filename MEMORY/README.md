# Turso Service - Project Memory

**Project:** turso-db (Turso Service)
**Type:** Self-hosted Database Service
**Tech Stack:** Rust, Axum, Turso, SQLite
**Status:** Production Ready

---

## Overview

A self-hosted, privacy-first database service powered by Turso. Provides SQLite-compatible database hosting with JWT authentication, multi-tenant isolation, and REST API.

---

## Architecture

```
turso-db/
├── src/
│   ├── main.rs          # Entry point
│   ├── config.rs        # Configuration
│   ├── db.rs            # Database manager
│   ├── auth.rs          # JWT authentication
│   ├── routes.rs        # API routes
│   ├── models.rs        # Data models
│   ├── users.rs         # User management
│   ├── ratelimit.rs     # Rate limiting
│   └── analytics.rs     # Query tracking
├── static/              # Web UI
├── MEMORY/              # Project memory
├── Cargo.toml           # Rust dependencies
├── Dockerfile           # Docker setup
├── docker-compose.yml   # Docker Compose
└── CLAUDE.md            # AI rules
```

---

## Tech Stack

| Component | Technology |
|-----------|------------|
| Language | Rust (2024 edition) |
| Web Framework | Axum |
| Database | Turso (libSQL/SQLite) |
| Auth | JWT (jsonwebtoken + argon2) |
| Rate Limiting | In-memory (dashmap) |
| Analytics | Query tracking |
| Deploy | Docker, Render, Cloudflare |

---

## API Reference

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/v1/health` | No | Health check |
| POST | `/v1/auth/login` | No | Get JWT token |
| POST | `/v1/databases` | Yes | Create database |
| GET | `/v1/databases` | Yes | List databases |
| POST | `/v1/databases/{id}/execute` | Yes | Run SQL |
| POST | `/v1/databases/{id}/query` | Yes | Query data |

---

## Key Features

- **SQLite-compatible** - Full SQL support
- **Multi-tenant** - Isolated databases per user/app
- **JWT Authentication** - Secure API access
- **REST API** - Simple HTTP interface
- **Privacy-first** - No external calls, fully offline
- **MIT Licensed** - Fork, modify, deploy anywhere

---

## Memory Locations

- **Project Memory:** `MEMORY/projects/turso-db/`
- **Lessons:** `MEMORY/lessons/`
- **Reports:** `MEMORY/reports/daily/`
- **Patterns:** `MEMORY/patterns/`

---

**Last Updated:** 2026-08-08
**Maintained By:** Super AI
