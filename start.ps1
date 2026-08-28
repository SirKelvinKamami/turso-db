# Turso Service - Start Script
# Run this to start the production server

$ErrorActionPreference = "Stop"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Turso Service - Starting..." -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

# Set Rust toolchain locations
$env:RUSTUP_HOME = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "D:\rustup" }
$env:CARGO_HOME = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { "D:\cargo" }

# Locate cargo and a MinGW-w64 bin (needed to build the gnu target on Windows)
$cargoBin = "$env:USERPROFILE\.cargo\bin"
$mingwBins = @(
    "D:\Backups\Ruby40-x64\msys64\ucrt64\bin",
    "C:\msys64\mingw64\bin",
    "C:\msys64\ucrt64\bin"
) | Where-Object { Test-Path $_ }
$env:PATH = @($cargoBin, $mingwBins) + $env:PATH -join ";"

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

# Build if the binary is missing (first run)
$exePath = Join-Path $PWD "target\debug\turso-service.exe"
if (-not (Test-Path $exePath)) {
    Write-Host "[BUILD] Binary not found - building debug binary..." -ForegroundColor Yellow
    cargo build
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $exePath)) {
        Write-Host "[ERROR] Build failed. Check that a MinGW-w64 toolchain is available." -ForegroundColor Red
        exit 1
    }
    Write-Host "[BUILD] Build complete" -ForegroundColor Green
}

# Start server
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
} catch {
    Write-Host "[WARN] Server started but health check failed" -ForegroundColor Yellow
    Write-Host "       Check if port 3000 is available" -ForegroundColor Yellow
}