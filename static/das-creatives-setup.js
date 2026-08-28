// DAS Creatives Hub — One-shot setup
// Update credentials, then paste this in browser console on the dashboard page.
// Never commit real passwords.

const API = 'https://turso-db-8svn.onrender.com/v1';
const USER = 'das-creatives';
const PASS = 'REPLACE_WITH_USER_PASSWORD';

async function setup() {
  const login = await fetch(`${API}/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: USER, password: PASS }),
  });
  if (!login.ok) throw new Error('Login: ' + (await login.json()).error);
  const { token } = await login.json();

  const res = await fetch(`${API}/setup`, {
    method: 'POST',
    headers: { 'Authorization': `Bearer ${token}` },
  });
  if (!res.ok) throw new Error('Setup: ' + (await res.json()).error);
  const data = await res.json();

  console.log('Database ID:', data.database_id);
  console.log('Tables:', data.schema);
  return data;
}

// Run: setup().then(d => console.log('Ready:', d.database_id));

// --- Query after setup ---
// fetch(`${API}/databases/${DB_ID}/query`, {
//   method: 'POST',
//   headers: { 'Authorization': `Bearer ${TOKEN}`, 'Content-Type': 'application/json' },
//   body: JSON.stringify({ sql: 'SELECT * FROM projects' }),
// }).then(r => r.json()).then(console.log);
