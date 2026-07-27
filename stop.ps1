# Turso Service - Stop Script
# Run this to stop the server gracefully

Write-Host "Stopping Turso Service..." -ForegroundColor Cyan

$proc = Get-Process turso-service -ErrorAction SilentlyContinue
if ($proc) {
    Stop-Process -Name turso-service -Force
    Start-Sleep -Seconds 1
    Write-Host "[OK] Server stopped" -ForegroundColor Green
} else {
    Write-Host "[INFO] Server is not running" -ForegroundColor Yellow
}
