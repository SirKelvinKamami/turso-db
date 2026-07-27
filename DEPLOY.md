# Turso Service - Deployment Guide

## Quick Start (User Testing)

### 1. First Time Setup
```powershell
cd D:\turso-service

# Configuration is ready (.env already configured)
# JWT_SECRET and admin credentials are set
```

### 2. Start the Server
```powershell
.\start.ps1
```

### 3. Test the API
```powershell
# Login and get token
$token = (Invoke-RestMethod -Uri "http://localhost:3000/v1/auth/login" -Method Post -ContentType "application/json" -Body '{"username":"admin","password":"TJACuFbcLnBHN31Y"}').token

# Create a database
$db = Invoke-RestMethod -Uri "http://localhost:3000/v1/databases" -Method Post -ContentType "application/json" -Body '{"name":"my-app"}' -Headers @{Authorization="Bearer $token"}

# Create table
Invoke-RestMethod -Uri "http://localhost:3000/v1/databases/$($db.id)/execute" -Method Post -ContentType "application/json" -Body '{"sql":"CREATE TABLE users (id INTEGER, name TEXT, email TEXT)"}' -Headers @{Authorization="Bearer $token"}

# Insert data
Invoke-RestMethod -Uri "http://localhost:3000/v1/databases/$($db.id)/execute" -Method Post -ContentType "application/json" -Body '{"sql":"INSERT INTO users VALUES (1, ''Alice'', ''alice@test.com'')"}' -Headers @{Authorization="Bearer $token"}

# Query data
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
| POST | `/v1/auth/login` | Get JWT token |
| POST | `/v1/auth/register` | Register user |

### Protected Endpoints (require Bearer token)
| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/v1/databases` | List all databases |
| POST | `/v1/databases` | Create new database |
| GET | `/v1/databases/{id}` | Get database info |
| DELETE | `/v1/databases/{id}` | Delete database |
| POST | `/v1/databases/{id}/execute` | Execute SQL (INSERT/UPDATE/DELETE) |
| POST | `/v1/databases/{id}/query` | Query data (SELECT) |

---

## Configuration (.env)

| Variable | Value | Description |
|----------|-------|-------------|
| `BIND_ADDRESS` | `0.0.0.0:3000` | Server bind address |
| `DATA_DIR` | `D:/turso-service/data` | Database files location |
| `JWT_SECRET` | `KJCahIT4SIkilqH6MRSZ1jlQp2cVphBOaiaEGjk1JUA=` | JWT signing secret |
| `JWT_EXPIRY_HOURS` | `24` | Token expiration time |
| `ADMIN_USERNAME` | `admin` | Admin login username |
| `ADMIN_PASSWORD` | `TJACuFbcLnBHN31Y` | Admin login password |
| `MAX_DATABASES` | `100` | Max databases per instance |
| `RUST_LOG` | `info` | Log level |

---

## Data Privacy Features

| Feature | Implementation |
|---------|----------------|
| **Data Locality** | All data stored in `DATA_DIR` - never leaves your machine |
| **No Telemetry** | Zero external calls - fully offline capable |
| **JWT Auth** | Tokens expire after configurable hours |
| **Filesystem Encryption** | BitLocker enabled on D: drive |
| **MIT License** | Fork, modify, deploy anywhere |

---

## Production Hardening Checklist

- [x] Set strong `JWT_SECRET` in `.env`
- [x] Change default admin password
- [x] Enable OS-level disk encryption (BitLocker on D:)
- [ ] Configure firewall to restrict port 3000
- [ ] Set up automated backups of `DATA_DIR`
- [ ] Configure log rotation
- [ ] Set up monitoring on `/v1/health`

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
| Port already in use | Change `BIND_ADDRESS` in `.env` or kill existing process |
| Database not found | Check `DATA_DIR` path in `.env` |
| Token expired | Re-login to get new token |
| Build fails | Run `.\start.ps1` which handles environment setup |
| Forgot admin password | Check `.env` file for `ADMIN_PASSWORD` |
