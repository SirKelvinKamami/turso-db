# Turso Service

A self-hosted, privacy-first database service powered by [Turso](https://github.com/tursodatabase/turso). Your data stays on your machine.

## Features

- **SQLite-compatible** - Full SQL support
- **Multi-tenant** - Isolated databases per user/app
- **JWT Authentication** - Secure API access
- **REST API** - Simple HTTP interface
- **Privacy-first** - No external calls, fully offline
- **MIT Licensed** - Fork, modify, deploy anywhere

## Quick Start

```powershell
# 1. Configure
Copy-Item .env.example .env
notepad .env  # Set JWT_SECRET!

# 2. Start
.\start.ps1

# 3. Use
$token = (Invoke-RestMethod -Uri "http://localhost:3000/v1/auth/login" -Method Post -ContentType "application/json" -Body '{"username":"admin","password":"password"}').token
```

## API Endpoints

| Method | Endpoint | Auth | Description |
|--------|----------|------|-------------|
| GET | `/v1/health` | No | Health check |
| POST | `/v1/auth/login` | No | Get JWT token |
| POST | `/v1/databases` | Yes | Create database |
| GET | `/v1/databases` | Yes | List databases |
| POST | `/v1/databases/{id}/execute` | Yes | Run SQL |
| POST | `/v1/databases/{id}/query` | Yes | Query data |

## Documentation

- [Deployment Guide](DEPLOY.md) - Full setup instructions
- [API Reference](DEPLOY.md#api-reference) - Complete API docs
- [Privacy Features](DEPLOY.md#data-privacy-features) - Security details

## License

MIT
