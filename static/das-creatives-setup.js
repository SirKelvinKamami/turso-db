// DAS Creatives Hub — One-shot setup script
// Run this after every Render deploy to re-create the database + schema.
// Uses the REST API (no @libsql/client needed).

const API = 'https://turso-db-8svn.onrender.com/v1';
const USER = 'das-creatives';
const PASS = 'nw2a.-QVN-85L||8';

async function setup() {
  // 1. Login
  const login = await fetch(`${API}/auth/login`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ username: USER, password: PASS }),
  });
  if (!login.ok) throw new Error('Login failed: ' + (await login.json()).error);
  const { token } = await login.json();
  console.log('Logged in');

  // 2. Create database
  const dbRes = await fetch(`${API}/databases`, {
    method: 'POST',
    headers: { 'Authorization': `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ name: 'das-creatives-hub' }),
  });
  if (!dbRes.ok && !dbRes.status.toString().startsWith('4')) throw new Error('DB create failed');
  const db = await dbRes.json();
  const dbId = db.id;
  console.log('Database:', dbId);

  // 3. Create tables
  const sqls = [
    `CREATE TABLE IF NOT EXISTS projects (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      name TEXT NOT NULL,
      description TEXT,
      status TEXT DEFAULT 'active',
      created_at TEXT DEFAULT (datetime('now'))
    )`,
    `CREATE TABLE IF NOT EXISTS tasks (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      project_id INTEGER,
      title TEXT NOT NULL,
      description TEXT,
      status TEXT DEFAULT 'pending',
      created_at TEXT DEFAULT (datetime('now'))
    )`,
    `INSERT INTO projects (name, description)
     VALUES ('DAS Creatives Hub', 'Creative platform for digital assets')`,
  ];

  for (const sql of sqls) {
    const r = await fetch(`${API}/databases/${dbId}/execute`, {
      method: 'POST',
      headers: { 'Authorization': `Bearer ${token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ sql }),
    });
    if (!r.ok) throw new Error('SQL failed: ' + (await r.json()).error);
  }

  console.log('Tables created, project seeded');
  console.log('DB ID:', dbId);
  return { dbId, token };
}

// Run it:
// setup().then(console.log).catch(console.error);

// After setup, use the returned token for queries:
// fetch(`${API}/databases/${dbId}/query`, {
//   method: 'POST',
//   headers: { 'Authorization': `Bearer ${token}`, 'Content-Type': 'application/json' },
//   body: JSON.stringify({ sql: 'SELECT * FROM projects' }),
// }).then(r => r.json()).then(console.log);
