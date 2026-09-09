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
| GET | `/v1/databases/{id}/webhooks` | List webhooks for a database |
| POST | `/v1/databases/{id}/webhooks` | Register a webhook (`{url, secret?, events?}`) |
| DELETE | `/v1/databases/{id}/webhooks/{hook_id}` | Remove a webhook |
| POST | `/v1/sync/{id}` | Apply a webhook payload's statements (push-sync receiver) |
| POST | `/v1/setup` | One-click init: db + `projects`/`tasks` schema + seed |
| GET | `/v1/analytics` | Query volume, totals, per-client breakdown |

### LibSQL Compatible (Turso protocol via HTTP)
| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/v1/libsql/{db}/v2/pipeline` | Hrana v2 pipeline; auth via `Authorization: Bearer <api-key>` |

`{db}` matches a database by name or id. Client API keys are issued on account
creation and can be rotated via the `/v1/users/{username}/api-key` endpoint.

---

## Webhooks (change notifications / push sync)

Register a URL and the service POSTs a JSON payload to it whenever a write
(`INSERT`/`UPDATE`/`DELETE`/`CREATE`/…) succeeds on that database — through either
`/execute` or the libsql pipeline. This is also the mechanism for push-style sync:
point a webhook at another turso-service instance to forward writes.

```powershell
$headers = @{ Authorization = "Bearer $token" }

# Register (optional secret signs the payload; optional events filter; optional headers;
# optional custom retry policy)
$body = '{
  "url":"https://your-app.example.com/hooks/db-changed",
  "secret":"pick-a-long-random-string",
  "headers":{"X-Foo":"bar"},
  "retry":{"max_attempts":5,"backoff_ms":[1000,2000,4000,8000]}
}'
$wh = Invoke-RestMethod -Uri "http://localhost:3100/v1/databases/$($db.id)/webhooks" -Method Post `
  -ContentType "application/json" -Headers $headers -Body $body

# List / remove
Invoke-RestMethod -Uri "http://localhost:3100/v1/databases/$($db.id)/webhooks" -Headers $headers
Invoke-RestMethod -Uri "http://localhost:3100/v1/databases/$($db.id)/webhooks/$($wh.id)" -Method Delete -Headers $headers
```

Delivery details:

- **Timing:** fire-and-forget after the write commits; the HTTP response is not
  delayed by webhook delivery.
- **Retries:** a failed or non-2xx delivery is retried with backoff (`1s / 2s / 4s / 8s`,
  five attempts total by default) before giving up. Override per webhook with
  `retry: { "max_attempts": N, "backoff_ms": [...] }` at creation (validated: 1..=100
  attempts, up to 20 delays each 1..=600000ms). All attempts happen off the request path.
- **Durable queue:** if all retry attempts still fail, the delivery is persisted to
  `DATA_DIR/pending_deliveries/` and retried again on a background timer (every 60s),
  so it survives process restarts until the receiver comes back. Queued deliveries for a
  deleted webhook/database are purged automatically. Queues are additionally bounded:
  deliveries are dropped once they exceed `WEBHOOK_PENDING_MAX_ATTEMPTS` (default
  10080) or are older than `WEBHOOK_PENDING_TTL_SECS` (default 604800 = 7 days).
- **Event:** currently the `write` event (one delivery per write batch; every
  statement in the batch is listed in the payload).
- **Payload** (JSON, `Content-Type: application/json`):
  ```json
  {
    "event": "write",
    "schema_version": 1,
    "delivery_id": "<uuid>",
    "timestamp": "RFC3339",
    "database": { "id": "<uuid>", "name": "my-app" },
    "owner": "admin",
    "statements": ["INSERT INTO users ..."],
    "changes": [
      { "sql": "INSERT INTO users ...", "op": "insert", "table": "users",
        "values": { "columns": null, "rows": [["1", "x"]] } },
      { "sql": "UPDATE users SET status='paid' WHERE id=7", "op": "update",
        "table": "users", "values": { "set": { "status": "paid" }, "where": "id=7" } },
      { "sql": "DELETE FROM users WHERE id=7", "op": "delete",
        "table": "users", "values": { "where": "id=7" } }
    ],
    "rows_affected": 1
  }
  ```
  `changes` is a best-effort classifier: each entry carries `op` (`insert`/`update`/
  `delete`/`ddl`/`other` or `null` when unrecognized — e.g. a CTE — and
  `table` when it can be parsed). Entries may also carry `values` captured from the
  statement: inserts get `{ "columns": [...], "rows": [[...]] }` parsed from the
  `VALUES` clause; updates get `{ "set": {...}, "where": "<raw>" }` (WHERE omitted when
  absent); deletes get `{ "where": "<raw>" }` (or `null` for a full-table
  `DELETE FROM t`). `values` is `null` when a clause can't be parsed (e.g.
  `INSERT ... SELECT`).
- **Signature:** if `secret` is set, the request includes
  `X-Turso-Signature: sha256=<lowercase hex HMAC-SHA256 of the raw body>`.
  Verify on the receiver side for authenticity (see `scripts/webhook-receiver.js`).
- **Headers:** `X-Turso-Event: write`, `X-Turso-Database: <id>`, plus any custom
  `headers` you registered (e.g. `Authorization` for a sync receiver).
- Webhooks are stored in `DATA_DIR/webhooks.json` (and mirrored to Supabase when
  `SUPABASE_URL`/`SUPABASE_SERVICE_KEY` are set) and survive restarts. Deleting a
  database removes its webhooks. Owner-only access (tenant isolation preserved).

## Push-style sync between instances

Point a webhook at another turso-service instance's sync receiver to replicate
writes: register a webhook with `url = https://target/v1/sync/<target-db-id>` and a
`headers` entry `{ "Authorization": "Bearer <token-with-access-to-target-db>" }`.

The sync receiver applies the payload's `statements` to the target database and
deliberately does **not** re-dispatch webhooks, so replication is always one hop
from the authoritative source — this prevents notification loops. For N replicas,
register each replica's sync URL as a webhook on the source.

```powershell
Invoke-RestMethod -Uri "http://localhost:3100/v1/databases/$($a.id)/webhooks" -Method Post `
  -ContentType "application/json" -Headers $headers `
  -Body (@{ url = "http://localhost:3100/v1/sync/$($b.id)"; headers = @{ Authorization = "Bearer $token" } } | ConvertTo-Json -Depth 5)
```

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