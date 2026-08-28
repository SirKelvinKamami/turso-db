# Turso Database Admin
# Run this to manage your databases
# Reads credentials from .env — never hardcode passwords.

param(
    [string]$Action = "help",
    [string]$DatabaseName = ""
)

$API_URL = "https://turso-db-8svn.onrender.com"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Turso Database Admin" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# Load admin credentials from .env
function Get-EnvValue {
    param([string]$Key)
    if (-not (Test-Path ".env")) { return $null }
    (Get-Content ".env" | Where-Object { $_ -match "^$Key=" } | ForEach-Object {
        $_ -replace "^$Key=", "" -replace '^"|"$', ''
    }) | Select-Object -First 1
}

$AdminUser = Get-EnvValue "ADMIN_USERNAME"
if (-not $AdminUser) { $AdminUser = "admin" }
$AdminPass = Get-EnvValue "ADMIN_PASSWORD"
if (-not $AdminPass) {
    Write-Host "[ERROR] ADMIN_PASSWORD not found in .env" -ForegroundColor Red
    exit 1
}

# Login function
function Get-AdminToken {
    $body = @{ username = $AdminUser; password = $AdminPass } | ConvertTo-Json
    $login = Invoke-RestMethod -Uri "$API_URL/v1/auth/login" -Method Post -ContentType "application/json" -Body $body
    return $login.token
}

$token = Get-AdminToken

switch ($Action) {
    "list" {
        Write-Host "Your Databases:" -ForegroundColor Green
        $dbs = Invoke-RestMethod -Uri "$API_URL/v1/databases" -Method Get -Headers @{Authorization="Bearer $token"}
        $dbs | ForEach-Object {
            Write-Host "  - $($_.name)" -ForegroundColor White
            Write-Host "    ID: $($_.id)" -ForegroundColor Gray
            Write-Host "    Owner: $($_.owner)" -ForegroundColor Gray
            Write-Host ""
        }
    }

    "create" {
        if (-not $DatabaseName) {
            Write-Host "Usage: .\admin.ps1 -Action create -DatabaseName 'my-db'" -ForegroundColor Red
            exit 1
        }
        Write-Host "Creating database: $DatabaseName" -ForegroundColor Yellow
        $newDb = Invoke-RestMethod -Uri "$API_URL/v1/databases" -Method Post -ContentType "application/json" -Body "{`"name`":`"$DatabaseName`"}" -Headers @{Authorization="Bearer $token"}
        Write-Host "Database created!" -ForegroundColor Green
        Write-Host "  ID: $($newDb.id)" -ForegroundColor White
        Write-Host "  Name: $($newDb.name)" -ForegroundColor White
    }

    "query" {
        if (-not $DatabaseName) {
            Write-Host "Usage: .\admin.ps1 -Action query -DatabaseName 'das-hub'" -ForegroundColor Red
            exit 1
        }
        # Get database ID
        $dbs = Invoke-RestMethod -Uri "$API_URL/v1/databases" -Method Get -Headers @{Authorization="Bearer $token"}
        $db = $dbs | Where-Object { $_.name -eq $DatabaseName }
        if (-not $db) {
            Write-Host "Database not found: $DatabaseName" -ForegroundColor Red
            exit 1
        }
        Write-Host "Querying database: $DatabaseName" -ForegroundColor Yellow
        $result = Invoke-RestMethod -Uri "$API_URL/v1/databases/$($db.id)/query" -Method Post -ContentType "application/json" -Body '{"sql":"SELECT name FROM sqlite_master WHERE type=''table''"}' -Headers @{Authorization="Bearer $token"}
        Write-Host "Tables:" -ForegroundColor Green
        $result.rows | ForEach-Object { Write-Host "  - $($_[0])" -ForegroundColor White }
    }

    "users" {
        Write-Host "Registered Users:" -ForegroundColor Green
        $dbs = Invoke-RestMethod -Uri "$API_URL/v1/databases" -Method Get -Headers @{Authorization="Bearer $token"}
        $db = $dbs | Where-Object { $_.name -eq "das-hub" }
        if ($db) {
            $result = Invoke-RestMethod -Uri "$API_URL/v1/databases/$($db.id)/query" -Method Post -ContentType "application/json" -Body '{"sql":"SELECT id, username, email, created_at FROM users"}' -Headers @{Authorization="Bearer $token"}
            if ($result.rows) {
                $result.rows | ForEach-Object {
                    Write-Host "  - $($_[1]) ($($_[2]))" -ForegroundColor White
                }
            } else {
                Write-Host "  No users found" -ForegroundColor Yellow
            }
        }
    }

    "help" {
        Write-Host "Commands:" -ForegroundColor Yellow
        Write-Host "  .\admin.ps1 -Action list                              List all databases" -ForegroundColor White
        Write-Host "  .\admin.ps1 -Action create -DatabaseName 'name'       Create new database" -ForegroundColor White
        Write-Host "  .\admin.ps1 -Action query -DatabaseName 'name'        Show tables in database" -ForegroundColor White
        Write-Host "  .\admin.ps1 -Action users                             Show users in das-hub" -ForegroundColor White
        Write-Host ""
        Write-Host "Credentials are read from .env" -ForegroundColor Yellow
    }
}