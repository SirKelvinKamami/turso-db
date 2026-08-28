# Turso Service - Deployment Guide

## Quick Start (Local)

### 1. Configure
```powershell
cd D:\turso-service
Copy-Item .env.example .env
notepad .env   # set JWT_SECRET, ADMIN_USERNAME, ADMIN_PASSWORD (see below)
```

Every secret (`JWT_SECRET`, `ADMIN_PASSWORD`, `SUPABASE_SERVICE_KEY`, ...) lives in
`.env` — never commit these to the repository.

### 2. Start the Server
```powershell
.\start.ps1
```
The script auto-builds the binary on first run and starts it on the port from
`BIND_ADDRESS` (default `0.0.0.0:3000`).

### 3. Test the API
```powershell
$adminPass = (Get-Content .env | Where-Object { $_ -match "^ADMIN_PASSWORD=" }) -replace "^ADMIN_PASSWORD=", ""

# Login and get token
$token = (Invoke-RestMethod -Uri "http://localhost:3000/v1/auth/login" -Method Post -ContentType "application/json" -Body "{`"username`":`"admin`",`"password`":`"$adminPass`"}").token

# Create a database
$db = Invoke-RestMethod -Uri "http://localhost:3000/v1/databases" -Method Post -ContentType "application/json" -Body '{"name":"my-app"}' -Headers @{Authorization="Bearer $token"}

# Create table
Invoke-RestMethod -Uri "http://localhost:3000/v1/databases/$($db.id)/execute" -Method Post -ContentType "application/json" -Body '{"sql":"CREATE TABLE users (id INTEGER, name TEXT, email TEXT)"}' -Headers @{Authorization="Bearer $token"}

# Insert data
Invoke-RestMethod -Uri "http://localhost:3000/v1/databases/$($db.id)/execute" -Method Post -ContentType "application/json" -Body '{"sql":"INSERT INTO users VALUES (1, ''Alice'', ''alice@test.com'')"}' -Headers @{Authorization="Bearer $token"}

# Query data (returns real column names)
Invoke-RestMethod -Uri "http://localhost:3000/v1/databases/$($db.id)/query" -Method Post -ContentType "application/json" -Body '{"sql":"SELECT * FROM users"}' -Headers @{Authorization="Bearer $token"}
```

### 4. Stop the Server
```powershell
.\stop.ps1
```

---

## API Reference

### Public Endpoints
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/v1/health` | Health check (no auth) |
| GET | `/v1/rate-limit` | Rate limit info |
| GET | `/v1/auth/google-config` | Google OAuth client config |
| POST | `/v1/auth/login` | Get JWT token (admin or client user) |
| POST | `/v1/auth/google-token` | Exchange verified Google ID token for a JWT |

### Protected Endpoints (require `Authorization: Bearer <token>`)
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/v1/users/me` | Current user + plan limits |
| GET | `/v1/users` | List client users (admin) |
| POST | `/v1/auth/signup` | Create a client user (admin) |
| DELETE | `/v1/users/{username}` | Delete client user + databases (admin) |
| PUT | `/v1/users/{username}/plan` | Set plan: free/starter/pro/enterprise (admin) |
| POST | `/v1/users/{username}/api-key` | Rotate client API key (admin) |
| GET | `/v1/databases` | List databases |
| POST | `/v1/databases` | Create new database |
| GET | `/v1/databases/{id}` | Get database info |
| DELETE | `/v1/databases/{id}` | Delete database |
| POST | `/v1/databases/{id}/execute` | Execute SQL (INSERT/UPDATE/DELETE/CREATE) |
| POST | `/v1/databases/{id}/query` | Query data (SELECT) with real column names |
| POST | `/v1/setup` | One-click init: db + `projects`/`tasks` schema + seed |
| GET | `/v1/analytics` | Query volume, totals, per-client breakdown |

### LibSQL Compatible (Turso protocol via HTTP)
| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/v1/libsql/{db}/v2/pipeline` | Hrana v2 pipeline; auth via `Authorization: Bearer <api-key>` |

`{db}` matches a database by name or id. Client API keys are issued on account
creation and can be rotated via the `/v1/users/{username}/api-key` endpoint.

---

## Configuration (.env)

| Variable | Default | Description |
|----------|---------|-------------|
| `BIND_ADDRESS` | `0.0.0.0:3000` | Server bind address |
| `DATA_DIR` | `./data` | Database files location |
| `JWT_SECRET` | change-me | JWT signing secret (generate: `openssl rand -base64 32`) |
| `JWT_EXPIRY_HOURS` | `24` | Token expiration hours |
| `ADMIN_USERNAME` | `admin` | Admin login username |
| `ADMIN_PASSWORD` | *required* | Admin login password |
| `MAX_DATABASES` | `100` | Instance-wide database cap |
| `MAX_QUERIES_PER_MINUTE` | `60` | Global IP rate limit |
| `SEED_USERS` | *(empty)* | Comma-separated bootstrap users `user:password` |
| `GOOGLE_CLIENT_ID` | *(empty)* | Enables verified Google sign-in |
| `ANALYTICS_RETENTION_HOURS` | `168` | Analytics history window |
| `SUPABASE_URL` / `SUPABASE_SERVICE_KEY` | *(empty)* | Optional persistence backend |
| `RUST_LOG` | `info` | Log level |

### Persistence Backends

- **Local only** (no Supabase vars): data lives in `DATA_DIR` (manifest + `.db` files),
  users in memory.
- **Supabase** (`SUPABASE_URL` + `SUPABASE_SERVICE_KEY` set): users/plans registry,
  database registry + backup files (`turso-dbs` bucket), and analytics are persisted
  to Supabase and restored on startup.

---

## Data Privacy Features

| Feature | Implementation |
|---------|----------------|
| **Data Locality** | All data stored in `DATA_DIR` - never leaves your machine |
| **No Telemetry** | Zero external calls - fully offline capable |
| **JWT Auth** | Tokens expire after configurable hours |
| **Verified Google Login** | ID tokens are signature+audience verified against Google JWKS |
| **Filesystem Encryption** | BitLocker on data drives recommended |
| **MIT License** | Fork, modify, deploy anywhere |

---

## Production Hardening Checklist

- [x] Set strong `JWT_SECRET` in `.env` (never commit it)
- [x] Change default admin password
- [ ] Enable OS-level disk encryption (BitLocker on D:)
- [ ] Configure firewall to restrict port 3000
- [ ] Set up automated backups of `DATA_DIR`
- [ ] Configure log rotation
- [ ] Set up monitoring on `/v1/health`
- [ ] Prefer the Supabase persistence backend for durable storage

---

## Deployment (Render)

`render.yaml` provides a free-tier web service. Secrets (`JWT_SECRET`,
`ADMIN_PASSWORD`, `SEED_USERS`, `SUPABASE_URL`, `SUPABASE_SERVICE_KEY`) are marked
`sync: false` and must be entered in the Render dashboard, where they stay out of
the repo.

GitHub Actions CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, `cargo
check`, and `cargo build`, then curls the Render deploy hook on a green push to
`main`. Wire `RENDER_DEPLOY_HOOK_URL` as a repo secret.

---

## Backup Strategy

```powershell
# Backup all databases
$timestamp = Get-Date -Format "yyyyMMdd_HHmmss"
New-Item -ItemType Directory -Path ".\backups" -Force | Out-Null
Compress-Archive -Path ".\data\*" -DestinationPath ".\backups\data_$timestamp.zip"

# Or backup specific database
Copy-Item ".\data\$DB_ID.db" ".\backups\$DB_ID_$timestamp.db"
```

---

## Troubleshooting

| Issue | Solution |
|-------|----------|
| Port already in use | Change `BIND_ADDRESS` in `.env` (e.g. `0.0.0.0:3100`) or stop the other process |
| Database not found | Check `DATA_DIR` path in `.env` |
| Token expired | Re-login to get new token |
| Build fails | Run `.\start.ps1` which auto-builds; needs a MinGW-w64 toolchain on PATH for Windows |
| Admin login fails | Confirm `ADMIN_PASSWORD` is set in `.env` |
| Google button missing | Set `GOOGLE_CLIENT_ID` in `.env` and restart |