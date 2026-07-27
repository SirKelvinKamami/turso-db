#!/bin/bash

BASE_URL="http://localhost:3000/v1"

echo "=== Turso Service Test ==="

# Login
echo "1. Logging in..."
TOKEN=$(curl -s -X POST "$BASE_URL/auth/login" \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"password"}' | jq -r '.token')

echo "Token: $TOKEN"

# Create database
echo "2. Creating database..."
DB_ID=$(curl -s -X POST "$BASE_URL/databases" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name":"test-database"}' | jq -r '.id')

echo "Database ID: $DB_ID"

# Create table
echo "3. Creating table..."
curl -s -X POST "$BASE_URL/databases/$DB_ID/execute" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"sql":"CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT)"}'

# Insert data
echo "4. Inserting data..."
curl -s -X POST "$BASE_URL/databases/$DB_ID/execute" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"sql":"INSERT INTO users (name, email) VALUES ('"'"'Alice'"'"', '"'"'alice@example.com'"'"')"}'

curl -s -X POST "$BASE_URL/databases/$DB_ID/execute" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"sql":"INSERT INTO users (name, email) VALUES ('"'"'Bob'"'"', '"'"'bob@example.com'"'"')"}'

# Query data
echo "5. Querying data..."
curl -s -X POST "$BASE_URL/databases/$DB_ID/query" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"sql":"SELECT * FROM users"}' | jq .

echo "=== Test Complete ==="
