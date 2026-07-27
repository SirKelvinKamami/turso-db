# Get BitLocker Recovery Key
# Run this as Administrator!

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  BitLocker Recovery Key for D: Drive" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# Check BitLocker status
$status = manage-bde -status D: 2>&1
Write-Host "Current Status:" -ForegroundColor Yellow
Write-Host $status
Write-Host ""

# Get recovery key
Write-Host "Recovery Key:" -ForegroundColor Yellow
manage-bde -protectors -get D: 2>&1
Write-Host ""

# Save to file
$keyFile = "D:\turso-service\RECOVERY_KEY_$((Get-Date).ToString('yyyyMMdd')).txt"
Write-Host "Saving recovery key to: $keyFile" -ForegroundColor Yellow
manage-bde -protectors -get D: > $keyFile 2>&1
Write-Host "Saved!" -ForegroundColor Green

Write-Host ""
Write-Host "IMPORTANT: Save this recovery key somewhere safe!" -ForegroundColor Red
Write-Host "You may need it if you lose access to your TPM." -ForegroundColor Red
