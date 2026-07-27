# BitLocker Setup for D: Drive
# Run this as Administrator!

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Enabling BitLocker on D: Drive" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# Check if BitLocker is available
$feature = Get-WindowsOptionalFeature -Online -FeatureName BitLocker -ErrorAction SilentlyContinue
if ($feature -and $feature.State -ne "Enabled") {
    Write-Host "[INFO] Enabling BitLocker feature..." -ForegroundColor Yellow
    Enable-WindowsOptionalFeature -Online -FeatureName BitLocker -All -NoRestart
}

# Check current status
$status = manage-bde -status D: 2>&1
if ($status -match "Protection Status: Protection On") {
    Write-Host "[OK] BitLocker is already enabled on D:" -ForegroundColor Green
    exit 0
}

Write-Host "[INFO] Starting BitLocker encryption on D:" -ForegroundColor Yellow
Write-Host "       This may take some time depending on drive size." -ForegroundColor Yellow
Write-Host ""

# Enable BitLocker with TPM
manage-bde -on D: -UsedSpaceOnly -TPMAndPIN
# Or without TPM (backup key to file):
# manage-bde -on D: -UsedSpaceOnly -RecoveryPassword

Write-Host ""
Write-Host "[OK] BitLocker encryption started!" -ForegroundColor Green
Write-Host "     SAVE YOUR RECOVERY KEY somewhere safe!" -ForegroundColor Yellow
Write-Host "     Check: manage-bde -protectors -get D:" -ForegroundColor Yellow
