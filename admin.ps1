# NEXUS SYNTHEX - Turso Database Admin
# Run this to manage your databases

param(
    [string]$Action = "help",
    [string]$DatabaseName = ""
)

$API_URL = "https://turso-db-8svn.onrender.com"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "NEXUS SYNTHEX - Database Admin" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# Login function
function Get-AdminToken {
    $login = Invoke-RestMethod -Uri "$API_URL/v1/auth/login" -Method Post -ContentType "application/json" -Body '{"username":"admin","password":"TJACuFbcLnBHN31Y"}'
    return $login.token
}

$token = Get-AdminToken

switch ($Action) {
    "list" {
        Write-Host "Your Databases:" -ForegroundColor Green
        $dbs = Invoke-RestMethod -Uri "$API_URL/v1/databases" -Method Get -Headers @{Authorization="Bearer $token"}
        if ($dbs.databases) {
            $dbs.databases | ForEach-Object {
                Write-Host "  - $($_.name)" -ForegroundColor White
                Write-Host "    ID: $($_.id)" -ForegroundColor Gray
                Write-Host "    Owner: $($_.owner)" -ForegroundColor Gray
                Write-Host ""
            }
        } else {
            Write-Host "  No databases found" -ForegroundColor Yellow
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
        Write-Host "  .\admin.ps1 -Action list                    List all databases" -ForegroundColor White
        Write-Host "  .\admin.ps1 -Action create -DatabaseName 'name'  Create new database" -ForegroundColor White
        Write-Host "  .\admin.ps1 -Action query -DatabaseName 'name'   Show tables in database" -ForegroundColor White
        Write-Host "  .\admin.ps1 -Action users                    Show users in das-hub" -ForegroundColor White
        Write-Host ""
        Write-Host "Credentials:" -ForegroundColor Yellow
        Write-Host "  Admin: admin / TJACuFbcLnBHN31Y" -ForegroundColor White
        Write-Host "  User: das-creatives / nw2a.-QVN-85L||8" -ForegroundColor White
    }
}
