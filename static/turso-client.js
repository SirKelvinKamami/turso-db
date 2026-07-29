class TursoClient {
  constructor(baseURL) {
    this.baseURL = baseURL || 'https://turso-db-8svn.onrender.com/v1';
    this.token = null;
  }

  async login(username, password) {
    const res = await fetch(`${this.baseURL}/auth/login`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password }),
    });
    if (!res.ok) {
      const err = await res.json();
      throw new Error(err.error || 'Login failed');
    }
    const data = await res.json();
    this.token = data.token;
    return data.user;
  }

  headers() {
    return {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${this.token}`,
    };
  }

  async listDatabases() {
    const res = await fetch(`${this.baseURL}/databases`, { headers: this.headers() });
    if (!res.ok) throw new Error((await res.json()).error);
    return res.json();
  }

  async createDatabase(name) {
    const res = await fetch(`${this.baseURL}/databases`, {
      method: 'POST',
      headers: this.headers(),
      body: JSON.stringify({ name }),
    });
    if (!res.ok) throw new Error((await res.json()).error);
    return res.json();
  }

  async execute(dbId, sql) {
    const res = await fetch(`${this.baseURL}/databases/${dbId}/execute`, {
      method: 'POST',
      headers: this.headers(),
      body: JSON.stringify({ sql }),
    });
    if (!res.ok) throw new Error((await res.json()).error);
    return res.json();
  }

  async query(dbId, sql) {
    const res = await fetch(`${this.baseURL}/databases/${dbId}/query`, {
      method: 'POST',
      headers: this.headers(),
      body: JSON.stringify({ sql }),
    });
    if (!res.ok) throw new Error((await res.json()).error);
    return res.json();
  }

  async deleteDatabase(dbId) {
    const res = await fetch(`${this.baseURL}/databases/${dbId}`, {
      method: 'DELETE',
      headers: this.headers(),
    });
    if (!res.ok) throw new Error((await res.json()).error);
    return true;
  }
}

// -- Usage example --
/*
const client = new TursoClient();

// Login
await client.login('das-creatives', 'nw2a.-QVN-85L||8');

// List databases
const dbs = await client.listDatabases();
console.log('Databases:', dbs);

// Create a new database
const db = await client.createDatabase('my-app-db');
console.log('Created:', db);

// Execute SQL (CREATE TABLE, INSERT, UPDATE, DELETE)
await client.execute(db.id, `
  CREATE TABLE IF NOT EXISTS users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    email TEXT UNIQUE,
    created_at TEXT DEFAULT (datetime('now'))
  )
`);

// Insert data
await client.execute(db.id,
  "INSERT INTO users (name, email) VALUES ('Alice', 'alice@example.com')"
);

// Query data
const result = await client.query(db.id, 'SELECT * FROM users');
console.log('Columns:', result.columns);
console.log('Rows:', result.rows);
*/
