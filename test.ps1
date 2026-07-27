# Turso Service Test Script (PowerShell)

$BaseUrl = "http://localhost:3000/v1"

Write-Host "=== Turso Service Test ==="

# Login
Write-Host "1. Logging in..."
$LoginResponse = Invoke-RestMethod -Uri "$BaseUrl/auth/login" -Method Post -ContentType "application/json" -Body '{"username":"admin","password":"password"}'
$Token = $LoginResponse.token
Write-Host "Token: $Token"

# Create database
Write-Host "2. Creating database..."
$DbResponse = Invoke-RestMethod -Uri "$BaseUrl/databases" -Method Post -ContentType "application/json" -Body '{"name":"test-database"}' -Headers @{Authorization="Bearer $Token"}
$DbId = $DbResponse.id
Write-Host "Database ID: $DbId"

# Create table
Write-Host "3. Creating table..."
Invoke-RestMethod -Uri "$BaseUrl/databases/$DbId/execute" -Method Post -ContentType "application/json" -Body '{"sql":"CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT)"}' -Headers @{Authorization="Bearer $Token"}

# Insert data
Write-Host "4. Inserting data..."
Invoke-RestMethod -Uri "$BaseUrl/databases/$DbId/execute" -Method Post -ContentType "application/json" -Body '{"sql":"INSERT INTO users (name, email) VALUES (''Alice'', ''alice@example.com'')"}' -Headers @{Authorization="Bearer $Token"}

Invoke-RestMethod -Uri "$BaseUrl/databases/$DbId/execute" -Method Post -ContentType "application/json" -Body '{"sql":"INSERT INTO users (name, email) VALUES (''Bob'', ''bob@example.com'')"}' -Headers @{Authorization="Bearer $Token"}

# Query data
Write-Host "5. Querying data..."
$queryResult = Invoke-RestMethod -Uri "$BaseUrl/databases/$DbId/query" -Method Post -ContentType "application/json" -Body '{"sql":"SELECT * FROM users"}' -Headers @{Authorization="Bearer $Token"}
$queryResult | ConvertTo-Json -Depth 5

Write-Host "=== Test Complete ==="
