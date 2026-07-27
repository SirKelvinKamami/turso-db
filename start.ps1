# Turso Service - Start Script
# Run this to start the production server

$ErrorActionPreference = "Stop"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Turso Service - Starting..." -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# Set environment
$env:RUSTUP_HOME = "D:\rustup"
$env:CARGO_HOME = "D:\cargo"
$env:CARGO_TARGET_DIR = "D:\turso-target"
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH

# Check if already running
$existing = Get-Process turso-service -ErrorAction SilentlyContinue
if ($existing) {
    Write-Host "[WARN] Server already running (PID: $($existing.Id))" -ForegroundColor Yellow
    Write-Host "       Run .\stop.ps1 to stop it first" -ForegroundColor Yellow
    exit 1
}

# Ensure data directory exists
$dataDir = if (Test-Path ".env") {
    (Get-Content ".env" | Where-Object { $_ -match "^DATA_DIR=" } | ForEach-Object { $_ -replace "DATA_DIR=", "" })
} else { ".\data" }
New-Item -ItemType Directory -Path $dataDir -Force | Out-Null

# Start server
$exePath = "D:\turso-target\debug\turso-service.exe"
if (-not (Test-Path $exePath)) {
    Write-Host "[ERROR] Binary not found at $exePath" -ForegroundColor Red
    Write-Host "        Run: cargo build --release" -ForegroundColor Yellow
    exit 1
}

$proc = Start-Process -FilePath $exePath -WorkingDirectory $PWD -PassThru
Start-Sleep -Seconds 2

# Health check
try {
    $health = Invoke-RestMethod -Uri "http://localhost:3000/v1/health" -TimeoutSec 5
    Write-Host ""
    Write-Host "[OK] Server is running!" -ForegroundColor Green
    Write-Host "     PID: $($proc.Id)" -ForegroundColor Gray
    Write-Host "     Health: http://localhost:3000/v1/health" -ForegroundColor Gray
    Write-Host "     Version: $($health.version)" -ForegroundColor Gray
    Write-Host ""
    Write-Host "Test it:" -ForegroundColor Cyan
    Write-Host '  $token = (Invoke-RestMethod -Uri "http://localhost:3000/v1/auth/login" -Method Post -ContentType "application/json" -Body ''{"username":"admin","password":"password"}'').token' -ForegroundColor Gray
    Write-Host ""
} catch {
    Write-Host "[WARN] Server started but health check failed" -ForegroundColor Yellow
    Write-Host "       Check if port 3000 is available" -ForegroundColor Yellow
}
