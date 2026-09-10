use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use turso::Builder;
use turso::sync::{Builder as SyncBuilder, Database as SyncDatabase};
use uuid::Uuid;

use crate::config::SyncConfig;
use crate::supabase::Supabase;
use crate::webhooks::{RowFrame, StatementFrame, classify_write, where_text_for};

/// True when the statement is read-only (SELECT/WITH/PRAGMA/EXPLAIN/VALUES).
pub(crate) fn sql_is_query(sql: &str) -> bool {
    let head = sql.trim_start().to_ascii_lowercase();
    head.starts_with("select")
        || head.starts_with("with")
        || head.starts_with("pragma")
        || head.starts_with("explain")
        || head.starts_with("values")
}

/// Result of an executed write batch. `frames` holds best-effort before/after row
/// snapshots aligned one-to-one with `statements`.
#[derive(Debug, Clone)]
pub struct ExecuteReport {
    pub statements: Vec<String>,
    pub rows_affected: u64,
    pub frames: Vec<StatementFrame>,
}

/// Concrete, `Send`-friendly error used by change-framing so handler futures stay `Send`.
#[derive(Debug)]
struct FrameError(String);

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for FrameError {}

/// Cap on rows captured per change-frame, bounding webhook payload size for
/// full-table sweeps.
const FRAME_CAP_ROWS: usize = 100;

fn value_to_string(value: turso::Value) -> String {
    match value {
        turso::Value::Null => "NULL".to_string(),
        turso::Value::Integer(n) => n.to_string(),
        turso::Value::Real(f) => f.to_string(),
        turso::Value::Text(s) => s,
        turso::Value::Blob(b) => format!("<blob {} bytes>", b.len()),
    }
}

/// Splits SQL text into individual statements on top-level semicolons, keeping
/// semicolons inside string literals, quoted identifiers, and comments intact.
pub(crate) fn split_sql(sql: &str) -> Vec<String> {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Normal,
        Single,
        Double,
        Backtick,
        Bracket,
        LineComment,
        BlockComment,
    }

    let mut statements = Vec::new();
    let mut current = String::new();
    let mut chars = sql.chars().peekable();
    let mut mode = Mode::Normal;

    while let Some(c) = chars.next() {
        match mode {
            Mode::Normal => match c {
                '\'' => {
                    mode = Mode::Single;
                    current.push(c);
                }
                '"' => {
                    mode = Mode::Double;
                    current.push(c);
                }
                '`' => {
                    mode = Mode::Backtick;
                    current.push(c);
                }
                '[' => {
                    mode = Mode::Bracket;
                    current.push(c);
                }
                '-' if chars.peek() == Some(&'-') => {
                    mode = Mode::LineComment;
                    current.push(c);
                    current.push('-');
                    chars.next();
                }
                '/' if chars.peek() == Some(&'*') => {
                    mode = Mode::BlockComment;
                    current.push(c);
                    current.push('*');
                    chars.next();
                }
                ';' => {
                    let stmt = current.trim().to_string();
                    if !stmt.is_empty() {
                        statements.push(stmt);
                    }
                    current.clear();
                }
                _ => current.push(c),
            },
            Mode::Single => {
                current.push(c);
                if c == '\'' {
                    if chars.peek() == Some(&'\'') {
                        current.push('\'');
                        chars.next();
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::Double => {
                current.push(c);
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        current.push('"');
                        chars.next();
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::Backtick => {
                current.push(c);
                if c == '`' {
                    if chars.peek() == Some(&'`') {
                        current.push('`');
                        chars.next();
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::Bracket => {
                current.push(c);
                if c == ']' {
                    if chars.peek() == Some(&']') {
                        current.push(']');
                        chars.next();
                    } else {
                        mode = Mode::Normal;
                    }
                }
            }
            Mode::LineComment => {
                current.push(c);
                if c == '\n' {
                    mode = Mode::Normal;
                }
            }
            Mode::BlockComment => {
                current.push(c);
                if c == '*' && chars.peek() == Some(&'/') {
                    current.push('/');
                    chars.next();
                    mode = Mode::Normal;
                }
            }
        }
    }

    let stmt = current.trim().to_string();
    if !stmt.is_empty() {
        statements.push(stmt);
    }
    statements
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub id: String,
    pub owner: String,
    pub name: String,
    pub created_at: String,
}

#[derive(Clone)]
pub struct DatabaseEntry {
    pub owner: String,
    pub name: String,
    pub created_at: String,
}

/// Open database handle: a plain local engine, or (when sync is enabled) a synced
/// replica connected to the hub. Both expose the same `turso::Connection`, so every
/// statement path works identically on either. The sync engine's handle is `Send` but
/// NOT `Sync`, and its `connect()` future is not `Send` either — see `connect_to`.
#[derive(Clone)]
pub(crate) enum DbHandle {
    Local(turso::Database),
    Replica {
        db: SyncDatabase,
        remote_url: String,
    },
}

/// `turso::sync::Database::connect()` future is not `Send`: its body uses `&self`
/// (for the connection's `extra_io` driver callback) *after* an internal await, so the
/// async fn captures `&self` across the await. It must never run directly in a task
/// future that axum requires to be `Send`. `build()`/`push()`/`pull()` don't have this
/// shape, so opening and the supervisor loop are unproblematic.
async fn open_replica_connection(db: &SyncDatabase) -> Result<turso::Connection, turso::Error> {
    db.connect().await
}

// Compile-time shape check for the sync handle so a turso bump can't silently make
// handler futures non-`Send` again. Expected: sync Database is Send but NOT Sync (the
// latter is why `connect()` must run off the handler path — see `connect_to`).
const fn _assert_send<T: Send>() {}
const _: () = {
    _assert_send::<SyncDatabase>();
    _assert_send::<turso::Connection>();
};

/// Build the per-database remote URL as `{hub_base}/{db-name}` with a lean slug for the
/// db name so hub path segments stay safe (no slashes/quotes/query cruft).
fn sync_remote_url(hub_base: &str, db_name: &str) -> String {
    let base = hub_base.trim().trim_end_matches('/');
    let slug: String = db_name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{base}/{slug}")
}

#[derive(Clone)]
pub struct DatabaseManager {
    data_dir: String,
    manifest_path: String,
    databases: Arc<DashMap<String, (DbHandle, DatabaseEntry)>>,
    supabase: Option<Supabase>,
    sync: SyncConfig,
}

impl DatabaseManager {
    pub async fn new(
        data_dir: &str,
        supabase: Option<Supabase>,
        sync: SyncConfig,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(data_dir)?;
        let manager = Self {
            data_dir: data_dir.to_string(),
            manifest_path: format!("{}/databases.json", data_dir),
            databases: Arc::new(DashMap::new()),
            supabase,
            sync,
        };
        if manager.supabase.is_some() {
            manager.load_from_supabase().await?;
        } else {
            manager.load_manifest().await?;
        }
        Ok(manager)
    }

    /// Open a database file as either a plain local handle or (when sync is enabled and
    /// a hub is configured) a synced replica. Sync open failures degrade to local-only
    /// with a warning, so the instance keeps serving while the hub is unreachable.
    async fn open_handle(
        &self,
        path: &str,
        name: &str,
    ) -> Result<DbHandle, Box<dyn std::error::Error>> {
        if self.sync.enabled
            && let Some(base) = self.sync.hub_url.as_deref()
        {
            let remote = sync_remote_url(base, name);
            let mut builder = SyncBuilder::new_remote(path)
                .with_remote_url(&remote)
                .with_client_name("turso-service")
                .with_long_poll_timeout(Duration::from_millis(self.sync.poll_ms.max(250)));
            if let Some(token) = self.sync.hub_token.as_deref() {
                builder = builder.with_auth_token(token.to_string());
            }
            match builder.build().await {
                Ok(db) => {
                    tracing::info!("Opened synced replica for {name}: {remote}");
                    return Ok(DbHandle::Replica {
                        db,
                        remote_url: remote,
                    });
                }
                Err(e) => tracing::warn!(
                    "Sync open failed for {name} ({remote}); degraded to local-only: {e}"
                ),
            }
        }
        Ok(DbHandle::Local(Builder::new_local(path).build().await?))
    }

    /// Background supervisor that keeps replica databases converged with the hub: pull
    /// remote changes, push local changes, and periodically checkpoint the WAL. Safe
    /// no-op when sync is disabled; loop is single-task so pull/push never race.
    pub fn spawn_sync_loop(&self) {
        if !self.sync.enabled {
            return;
        }
        let dbs = self.databases.clone();
        let poll_ms = self.sync.poll_ms.max(250);
        tokio::spawn(async move {
            let mut tick: u64 = 0;
            loop {
                let replicas: Vec<(SyncDatabase, String)> = dbs
                    .iter()
                    .filter_map(|entry| match &entry.value().0 {
                        DbHandle::Replica { db, remote_url } => {
                            Some((db.clone(), remote_url.clone()))
                        }
                        DbHandle::Local(_) => None,
                    })
                    .collect();
                for (db, remote) in replicas {
                    if let Err(e) = db.pull().await {
                        tracing::debug!("sync pull failed ({remote}): {}", e);
                    }
                    if let Err(e) = db.push().await {
                        tracing::debug!("sync push failed ({remote}): {}", e);
                    }
                    tick += 1;
                    if tick.is_multiple_of(30)
                        && let Err(e) = db.checkpoint().await
                    {
                        tracing::debug!("sync checkpoint failed ({remote}): {}", e);
                    }
                }
                tokio::time::sleep(Duration::from_millis(poll_ms)).await;
            }
        });
    }

    async fn load_manifest(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut recovered = false;
        if Path::new(&self.manifest_path).exists() {
            let raw = std::fs::read_to_string(&self.manifest_path)?;
            if let Ok(entries) = serde_json::from_str::<Vec<ManifestEntry>>(&raw) {
                for entry in entries {
                    let path = format!("{}/{}.db", self.data_dir, entry.id);
                    if !Path::new(&path).exists() {
                        continue;
                    }
                    match self.open_handle(&path, &entry.name).await {
                        Ok(handle) => {
                            self.databases.insert(
                                entry.id.clone(),
                                (
                                    handle,
                                    DatabaseEntry {
                                        owner: entry.owner.clone(),
                                        name: entry.name.clone(),
                                        created_at: entry.created_at.clone(),
                                    },
                                ),
                            );
                            tracing::info!("Reloaded database: {} ({})", entry.name, entry.id);
                        }
                        Err(e) => tracing::warn!("Failed to reload database {}: {}", entry.id, e),
                    }
                }
            }
        }

        let known: Vec<String> = self
            .databases
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        for dir_entry in std::fs::read_dir(&self.data_dir)? {
            let path = dir_entry?.path();
            let Some(file_name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_string())
            else {
                continue;
            };
            if !file_name.ends_with(".db") || file_name == "auth.db" || file_name == "users.json" {
                continue;
            }
            let id = file_name.trim_end_matches(".db").to_string();
            if known.contains(&id) {
                continue;
            }
            let db_path = path.to_string_lossy().into_owned();
            let name = format!("recovered-{}", id);
            match self.open_handle(&db_path, &name).await {
                Ok(handle) => {
                    self.databases.insert(
                        id.clone(),
                        (
                            handle,
                            DatabaseEntry {
                                owner: "admin".to_string(),
                                name,
                                created_at: chrono::Utc::now().to_rfc3339(),
                            },
                        ),
                    );
                    tracing::warn!("Recovered orphan database file {} as owner admin", id);
                    recovered = true;
                }
                Err(e) => tracing::warn!("Failed to recover database file {}: {}", file_name, e),
            }
        }
        if recovered {
            self.save_manifest()?;
        }
        Ok(())
    }

    async fn load_from_supabase(&self) -> Result<(), Box<dyn std::error::Error>> {
        let sb = self.supabase.as_ref().unwrap();
        let rows = sb.rows("turso_databases", "").await?;
        let mut ids: Vec<String> = Vec::new();
        for row in rows {
            let id = row
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let owner = row
                .get("owner")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = row
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let created_at = row
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            ids.push(id.clone());
            let path = format!("{}/{}.db", self.data_dir, id);
            if !Path::new(&path).exists() {
                match sb.download_db(&owner, &id).await {
                    Ok(bytes) => {
                        tokio::fs::write(&path, bytes).await?;
                        tracing::info!("Restored database {} ({}) from storage", name, id);
                    }
                    Err(e) => {
                        tracing::warn!("No stored backup for {} ({}): {}", name, id, e);
                        self.open_handle(&path, &name).await?;
                    }
                }
            }
            match self.open_handle(&path, &name).await {
                Ok(handle) => {
                    self.databases.insert(
                        id.clone(),
                        (
                            handle,
                            DatabaseEntry {
                                owner,
                                name,
                                created_at,
                            },
                        ),
                    );
                    tracing::info!("Loaded database from registry: {}", id);
                }
                Err(e) => tracing::warn!("Failed to open database {}: {}", id, e),
            }
        }

        for dir_entry in std::fs::read_dir(&self.data_dir)? {
            let path = dir_entry?.path();
            let Some(file_name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.to_string())
            else {
                continue;
            };
            if !file_name.ends_with(".db") || file_name == "auth.db" {
                continue;
            }
            let id = file_name.trim_end_matches(".db").to_string();
            if !ids.contains(&id) {
                let _ = std::fs::remove_file(&path);
                tracing::warn!("Removed orphan database file not in registry: {}", id);
            }
        }
        Ok(())
    }

    fn save_manifest(&self) -> Result<(), Box<dyn std::error::Error>> {
        let entries: Vec<ManifestEntry> = self
            .databases
            .iter()
            .map(|entry| {
                let id = entry.key().clone();
                let meta = entry.value().1.clone();
                ManifestEntry {
                    id,
                    owner: meta.owner,
                    name: meta.name,
                    created_at: meta.created_at,
                }
            })
            .collect();
        let raw = serde_json::to_string_pretty(&entries)?;
        std::fs::write(&self.manifest_path, raw)?;
        Ok(())
    }

    pub async fn create_database(
        &self,
        name: &str,
        owner: &str,
    ) -> Result<(String, DatabaseEntry), Box<dyn std::error::Error>> {
        let id = Uuid::new_v4().to_string();
        let path = format!("{}/{}.db", self.data_dir, id);
        let handle = self.open_handle(&path, name).await?;
        let entry = DatabaseEntry {
            owner: owner.to_string(),
            name: name.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.databases.insert(id.clone(), (handle, entry.clone()));
        if let Some(sb) = &self.supabase {
            if let Err(e) = sb
                .insert(
                    "turso_databases",
                    serde_json::json!({
                        "id": id,
                        "name": entry.name,
                        "owner": entry.owner,
                        "created_at": entry.created_at,
                    }),
                )
                .await
            {
                tracing::error!("Failed to persist database registry row: {}", e);
            }
            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    if let Err(e) = sb.upload_db(&entry.owner, &id, bytes).await {
                        tracing::error!("Failed to upload new database file: {}", e);
                    }
                }
                Err(e) => tracing::error!("Failed to read new database file: {}", e),
            }
        } else {
            self.save_manifest()?;
        }
        tracing::info!("Created database: {} (owner: {}) at {}", name, owner, path);
        Ok((id, entry))
    }

    pub(crate) async fn get_database(
        &self,
        id: &str,
    ) -> Result<(DbHandle, DatabaseEntry), Box<dyn std::error::Error>> {
        self.databases
            .get(id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| format!("Database {} not found", id).into())
    }

    /// Open a `turso::Connection` for this database. Local engines connect in place;
    /// sync replicas run the non-`Send` `connect()` on a blocking thread via a
    /// `LocalSet` so every handler future stays `Send`. The first few calls after a
    /// replica is created may wait for the remote libsql hub to respond.
    async fn connect_to(
        &self,
        db_id: &str,
    ) -> Result<turso::Connection, Box<dyn std::error::Error>> {
        let (handle, _) = self.get_database(db_id).await?;
        match handle {
            DbHandle::Local(db) => db.connect().map_err(|e| e.to_string().into()),
            DbHandle::Replica { db, .. } => {
                // The sync engine's `connect()` future is not `Send`, so it cannot run
                // inline in these handler paths (axum requires `Send` futures). Run it
                // on a blocking thread with a dedicated current-thread runtime; the
                // engine drives completion through its own I/O worker thread.
                let conn = tokio::task::spawn_blocking(move || {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    rt.block_on(open_replica_connection(&db))
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(|e| e.to_string())??;
                Ok(conn)
            }
        }
    }

    async fn persist_db(&self, db_id: &str, owner: &str) -> Result<(), Box<dyn std::error::Error>> {
        let Some(sb) = &self.supabase else {
            return Ok(());
        };
        let path = format!("{}/{}.db", self.data_dir, db_id);
        // Match on the `Result` directly so its non-`Send` `Box<dyn Error>` arm is dropped
        // before the checkpoint await below (keeps the handler futures `Send`).
        let conn = match self.connect_to(db_id).await {
            Ok(conn) => conn,
            Err(_) => return Ok(()),
        };
        let _ = conn.execute("PRAGMA wal_checkpoint(TRUNCATE);", ()).await;
        let bytes = tokio::fs::read(&path).await?;
        if let Err(e) = sb.upload_db(owner, db_id, bytes).await {
            tracing::error!("Failed to persist database {} to storage: {}", db_id, e);
        }
        Ok(())
    }

    pub async fn execute(
        &self,
        db_id: &str,
        sql: &str,
    ) -> Result<ExecuteReport, Box<dyn std::error::Error>> {
        let (_, entry) = self.get_database(db_id).await?;
        let conn = self.connect_to(db_id).await?;
        let statements: Vec<String> = split_sql(sql)
            .into_iter()
            .filter(|s| !sql_is_query(s))
            .collect();
        if statements.is_empty() {
            return Ok(ExecuteReport {
                statements,
                rows_affected: 0,
                frames: Vec::new(),
            });
        }
        let (total, frames) = Self::frame_and_apply(&conn, &statements)
            .await
            .map_err(|e| -> Box<dyn std::error::Error> { e })?;
        let _ = self.persist_db(db_id, &entry.owner).await;
        Ok(ExecuteReport {
            statements,
            rows_affected: total,
            frames,
        })
    }

    /// True for transaction-control statements that must not be folded into the framing
    /// transaction wrapper (they would nest or conflict with `BEGIN IMMEDIATE`).
    fn is_control_statement(sql: &str) -> bool {
        let head = sql.trim_start().to_ascii_lowercase();
        ["begin", "commit", "rollback", "end", "savepoint", "release"]
            .iter()
            .any(|kw| head.starts_with(kw))
    }

    /// Read up to `FRAME_CAP_ROWS` rows as a stringified frame. `None` means the query
    /// succeeded but matched no rows. Errors are strings (Send) so frames never make a
    /// handler future non-`Send`.
    async fn read_frame(conn: &turso::Connection, sql: &str) -> Result<Option<RowFrame>, String> {
        let mut rows = conn.query(sql, ()).await.map_err(|e| e.to_string())?;
        let columns: Vec<String> = rows
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();
        let mut out = Vec::new();
        while let Some(row) = rows.next().await.map_err(|e| e.to_string())? {
            let mut r = Vec::with_capacity(row.column_count());
            for i in 0..row.column_count() {
                r.push(
                    row.get_value(i)
                        .map(value_to_string)
                        .unwrap_or_else(|_| "?".to_string()),
                );
            }
            out.push(r);
            if out.len() >= FRAME_CAP_ROWS {
                break;
            }
        }
        if out.is_empty() {
            return Ok(None);
        }
        Ok(Some(RowFrame { columns, rows: out }))
    }

    /// Executes a batch of write statements inside a single transaction, capturing true
    /// change-frames: matched rows before UPDATE/DELETE (via a rowid+columns snapshot)
    /// and `RETURNING rowid, *` rows after INSERT/UPDATE. Framing is best-effort and
    /// non-mutating when it fails: unsupported shapes (WITHOUT ROWID tables, RETURNING
    /// errors, control statements) degrade to plain execution without frames.
    async fn frame_and_apply(
        conn: &turso::Connection,
        statements: &[String],
    ) -> Result<(u64, Vec<StatementFrame>), Box<dyn std::error::Error + Send>> {
        fn qualified_table(name: &str) -> String {
            name.split('.')
                .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(".")
        }
        fn box_send(msg: String) -> Box<dyn std::error::Error + Send> {
            Box::new(FrameError(msg))
        }
        let wrap_tx = !statements.iter().any(|s| Self::is_control_statement(s));
        if wrap_tx {
            conn.execute("BEGIN IMMEDIATE", ())
                .await
                .map_err(|e| box_send(e.to_string()))?;
        }
        let mut total: u64 = 0;
        let mut frames = Vec::with_capacity(statements.len());
        let result: Result<(), String> = async {
            for stmt in statements {
                let (op, table) = classify_write(stmt);
                let mut frame = StatementFrame {
                    op: op.map(str::to_string),
                    table,
                    before: None,
                    after: None,
                };
                if matches!(op, Some("update") | Some("delete")) {
                    let snapshot = match (
                        frame.table.as_deref().map(qualified_table),
                        op.and_then(|o| where_text_for(stmt, o)),
                    ) {
                        (Some(t), Some(w)) => {
                            Self::read_frame(conn, &format!("SELECT rowid, * FROM {t} WHERE {w}"))
                                .await
                        }
                        _ => Ok(None),
                    };
                    match snapshot {
                        Ok(f) => frame.before = f,
                        Err(e) => tracing::debug!(
                            "Change-frame snapshot unavailable for {:?}; continuing: {}",
                            frame.op,
                            e
                        ),
                    }
                }
                let returned = if matches!(op, Some("insert") | Some("update") | Some("delete")) {
                    Some(Self::read_frame(conn, &format!("{stmt} RETURNING rowid, *")).await)
                } else {
                    None
                };
                match returned {
                    Some(Ok(after)) => {
                        total += after.as_ref().map_or(0, |f| f.rows.len()) as u64;
                        if matches!(op, Some("insert") | Some("update")) {
                            frame.after = after;
                        }
                    }
                    _ => {
                        total += conn.execute(stmt, ()).await.map_err(|e| e.to_string())?;
                    }
                }
                frames.push(frame);
            }
            Ok(())
        }
        .await;
        if wrap_tx {
            match result {
                Ok(()) => {
                    conn.execute("COMMIT", ())
                        .await
                        .map_err(|e| box_send(e.to_string()))?;
                }
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", ()).await;
                    return Err(box_send(e));
                }
            }
        } else if let Err(e) = result {
            return Err(box_send(e));
        }
        Ok((total, frames))
    }

    pub async fn query_with_columns(
        &self,
        db_id: &str,
        sql: &str,
    ) -> Result<(Vec<String>, Vec<Vec<String>>), Box<dyn std::error::Error>> {
        let (_, entry) = self.get_database(db_id).await?;
        let conn = self.connect_to(db_id).await?;
        let mut rows = conn.query(sql, ()).await?;
        let columns: Vec<String> = rows
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();

        let mut results = Vec::new();
        while let Some(row) = rows.next().await? {
            let mut row_data = Vec::new();
            for i in 0..row.column_count() {
                row_data.push(
                    row.get_value(i)
                        .map(value_to_string)
                        .unwrap_or_else(|_| "?".to_string()),
                );
            }
            results.push(row_data);
        }
        let _ = self.persist_db(db_id, &entry.owner).await;
        Ok((columns, results))
    }

    pub async fn query(
        &self,
        db_id: &str,
        sql: &str,
    ) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
        let (_, entry) = self.get_database(db_id).await?;
        let conn = self.connect_to(db_id).await?;
        let mut rows = conn.query(sql, ()).await?;
        let mut results = Vec::new();

        while let Some(row) = rows.next().await? {
            let mut row_data = Vec::new();
            for i in 0..row.column_count() {
                let value = row
                    .get_value(i)
                    .map(value_to_string)
                    .unwrap_or_else(|_| "?".to_string());
                row_data.push(value);
            }
            results.push(row_data);
        }
        let _ = self.persist_db(db_id, &entry.owner).await;
        Ok(results)
    }

    pub async fn run_statement(
        &self,
        db_id: &str,
        sql: &str,
        p: turso::params::Params,
    ) -> Result<
        (Vec<(String, Option<String>)>, Vec<Vec<turso::Value>>, u64),
        Box<dyn std::error::Error>,
    > {
        let (_, entry) = self.get_database(db_id).await?;
        let conn = self.connect_to(db_id).await?;

        let is_query = sql_is_query(sql);

        let mut cols: Vec<(String, Option<String>)> = Vec::new();
        let mut rows_out: Vec<Vec<turso::Value>> = Vec::new();
        let affected: u64;

        if is_query {
            let mut rows = conn.query(sql, p).await?;
            for c in rows.columns() {
                cols.push((c.name().to_string(), c.decl_type().map(|s| s.to_string())));
            }
            while let Some(row) = rows.next().await? {
                let mut r = Vec::with_capacity(row.column_count());
                for i in 0..row.column_count() {
                    r.push(row.get_value(i)?);
                }
                rows_out.push(r);
            }
            affected = 0;
        } else {
            affected = conn.execute(sql, p).await?;
            cols.push(("changes".to_string(), Some("integer".to_string())));
            rows_out.push(vec![turso::Value::Integer(affected as i64)]);
            self.persist_db(db_id, &entry.owner).await?;
        }

        Ok((cols, rows_out, affected))
    }

    pub async fn delete_database(&self, id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let owner = self.databases.get(id).map(|e| e.value().1.owner.clone());
        self.databases.remove(id);
        let base = format!("{}/{}", self.data_dir, id);
        for suffix in [".db", ".db-wal", ".db-shm"] {
            let f = format!("{}{}", base, suffix);
            if Path::new(&f).exists() {
                let _ = std::fs::remove_file(&f);
            }
        }
        if let Some(sb) = &self.supabase {
            if let Err(e) = sb.delete("turso_databases", &format!("id=eq.{}", id)).await {
                tracing::error!("Failed to delete registry row for {}: {}", id, e);
            }
            if let Some(owner) = owner
                && let Err(e) = sb.delete_db(&owner, id).await
            {
                tracing::error!("Failed to delete storage object for {}: {}", id, e);
            }
        } else {
            self.save_manifest()?;
        }
        tracing::info!("Deleted database: {}", id);
        Ok(())
    }

    pub fn list_databases(&self, owner: Option<&str>) -> Vec<(String, DatabaseEntry)> {
        self.databases
            .iter()
            .filter(|entry| {
                if let Some(owner) = owner {
                    entry.value().1.owner == owner
                } else {
                    true
                }
            })
            .map(|entry| (entry.key().clone(), entry.value().1.clone()))
            .collect()
    }

    pub fn get_db_owner(&self, db_id: &str) -> Option<String> {
        self.databases.get(db_id).map(|e| e.value().1.owner.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use turso::Builder;
    use uuid::Uuid;

    async fn local_conn() -> turso::Connection {
        let dir = std::env::temp_dir().join(format!("turso-frm-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.db").to_string_lossy().into_owned();
        let db = Builder::new_local(&path).build().await.unwrap();
        db.connect().unwrap()
    }

    #[test]
    fn sync_remote_url_slugs_db_names() {
        assert_eq!(
            sync_remote_url("https://hub.example.com", "my-app"),
            "https://hub.example.com/my-app"
        );
        assert_eq!(
            sync_remote_url("http://127.0.0.1:8080/", "a b/c"),
            "http://127.0.0.1:8080/a-b-c"
        );
        assert_eq!(
            sync_remote_url("libsql://hub.example.com", "orders"),
            "libsql://hub.example.com/orders"
        );
        assert_eq!(
            sync_remote_url("https://hub.example.com/", ""),
            "https://hub.example.com/"
        );
    }

    #[test]
    fn sync_config_defaults_to_disabled() {
        let c = SyncConfig::default();
        assert!(!c.enabled);
        assert!(c.hub_url.is_none());
        assert!(c.hub_token.is_none());
    }

    #[tokio::test]
    async fn open_handle_is_local_when_sync_disabled() {
        let dir = std::env::temp_dir().join(format!("turso-open-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mgr = DatabaseManager::new(dir.to_str().unwrap(), None, SyncConfig::default())
            .await
            .unwrap();
        let path = format!("{}/x.db", dir.to_str().unwrap());
        let handle = mgr.open_handle(&path, "demo").await.unwrap();
        assert!(matches!(handle, DbHandle::Local(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn splits_multiple_statements() {
        let stmts = split_sql("CREATE TABLE t (a TEXT); INSERT INTO t VALUES ('x');");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "CREATE TABLE t (a TEXT)");
        assert_eq!(stmts[1], "INSERT INTO t VALUES ('x')");
    }

    #[test]
    fn keeps_semicolons_inside_strings() {
        let stmts = split_sql("INSERT INTO t VALUES ('a;b'); SELECT 1;");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "INSERT INTO t VALUES ('a;b')");
    }

    #[test]
    fn ignores_semicolons_in_comments() {
        let stmts = split_sql("-- hello; world\nSELECT 1; /* cmt; */ SELECT 2");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "-- hello; world\nSELECT 1");
        assert_eq!(stmts[1], "/* cmt; */ SELECT 2");
    }

    #[test]
    fn handles_quoted_identifiers_and_escapes() {
        let stmts = split_sql("CREATE TABLE \"weird;name\" (a TEXT); SELECT 'it''s; ok' AS x");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "CREATE TABLE \"weird;name\" (a TEXT)");
        assert_eq!(stmts[1], "SELECT 'it''s; ok' AS x");
    }

    #[test]
    fn handles_backtick_and_bracket_identifiers() {
        let stmts = split_sql("CREATE TABLE `t;a` (b TEXT); SELECT 1 FROM [x;y]");
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "CREATE TABLE `t;a` (b TEXT)");
        assert_eq!(stmts[1], "SELECT 1 FROM [x;y]");
    }

    #[test]
    fn drops_empty_statements() {
        let stmts = split_sql(";; SELECT 1 ;;");
        assert_eq!(stmts, vec!["SELECT 1"]);
    }

    #[test]
    fn single_statement_without_terminator() {
        let stmts = split_sql("SELECT 1");
        assert_eq!(stmts, vec!["SELECT 1"]);
    }

    #[test]
    fn empty_input_yields_no_statements() {
        assert!(split_sql("   \n\t ").is_empty());
        assert!(split_sql("").is_empty());
    }

    #[tokio::test]
    async fn returning_clause_supported() {
        let dir = std::env::temp_dir().join(format!("turso-rtn-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe.db").to_string_lossy().into_owned();
        let db = Builder::new_local(&path).build().await.unwrap();
        let conn = db.connect().unwrap();

        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)", ())
            .await
            .unwrap();
        conn.execute("INSERT INTO t (name) VALUES ('a'), ('b'), ('c')", ())
            .await
            .unwrap();

        let mut rows = conn
            .query("INSERT INTO t (name) VALUES ('d') RETURNING rowid, *", ())
            .await
            .unwrap();
        let cols: Vec<String> = rows
            .columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect();
        let mut returned = 0usize;
        while let Some(row) = rows.next().await.unwrap() {
            returned += 1;
            assert_eq!(row.column_count(), cols.len());
        }
        assert_eq!(returned, 1, "INSERT ... RETURNING must return 1 row");

        let mut upd = conn
            .query(
                "UPDATE t SET name = upper(name) WHERE id <= 2 RETURNING rowid, name",
                (),
            )
            .await
            .unwrap();
        let mut n = 0usize;
        while let Some(_row) = upd.next().await.unwrap() {
            n += 1;
        }
        assert_eq!(n, 2, "UPDATE ... RETURNING must return 2 rows");

        let mut del = conn
            .query("DELETE FROM t WHERE id = 3 RETURNING rowid, name", ())
            .await
            .unwrap();
        let mut m = 0usize;
        while let Some(_row) = del.next().await.unwrap() {
            m += 1;
        }
        assert_eq!(m, 1, "DELETE ... RETURNING must return 1 row");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn frames_capture_before_and_after_rows() {
        let conn = local_conn().await;
        conn.execute(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO users (name, age) VALUES ('a', 1), ('b', 2)",
            (),
        )
        .await
        .unwrap();
        let stmts = vec![
            "INSERT INTO users (name, age) VALUES ('c', 3)".into(),
            "UPDATE users SET age = age + 1 WHERE id = 1".into(),
            "DELETE FROM users WHERE id = 2".into(),
        ];
        let (total, frames) = DatabaseManager::frame_and_apply(&conn, &stmts)
            .await
            .unwrap();
        assert_eq!(total, 3);

        let f0 = &frames[0];
        assert_eq!(f0.op.as_deref(), Some("insert"));
        assert!(f0.before.is_none());
        let after = f0.after.as_ref().unwrap();
        assert!(after.columns.iter().any(|c| c == "rowid"));
        assert_eq!(after.rows, vec![vec!["3", "3", "c", "3"]]);

        let f1 = &frames[1];
        assert_eq!(f1.op.as_deref(), Some("update"));
        assert_eq!(
            f1.before.as_ref().unwrap().rows,
            vec![vec!["1", "1", "a", "1"]]
        );
        assert_eq!(
            f1.after.as_ref().unwrap().rows,
            vec![vec!["1", "1", "a", "2"]]
        );

        let f2 = &frames[2];
        assert_eq!(f2.op.as_deref(), Some("delete"));
        assert_eq!(
            f2.before.as_ref().unwrap().rows,
            vec![vec!["2", "2", "b", "2"]]
        );
        assert!(f2.after.is_none());

        let mut q = conn
            .query("SELECT name, age FROM users ORDER BY id", ())
            .await
            .unwrap();
        let mut vals = Vec::new();
        while let Some(row) = q.next().await.unwrap() {
            vals.push(vec![
                row.get_value(0).map(value_to_string).unwrap(),
                row.get_value(1).map(value_to_string).unwrap(),
            ]);
        }
        assert_eq!(vals, vec![vec!["a", "2"], vec!["c", "3"]]);
    }

    #[tokio::test]
    async fn unframeable_tables_degrade_to_plain_execution() {
        let dir = std::env::temp_dir().join(format!("turso-frm-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wnr.db").to_string_lossy().into_owned();
        let db = Builder::new_local(&path)
            .experimental_without_rowid(true)
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        // WITHOUT ROWID tables have no implicit rowid, so both the snapshot
        // (`SELECT rowid, *`) and `RETURNING rowid, *` fail. Framing must degrade to
        // plain execution while still applying the write.
        conn.execute(
            "CREATE TABLE wnr (k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID",
            (),
        )
        .await
        .unwrap();
        conn.execute("INSERT INTO wnr VALUES ('x', '1')", ())
            .await
            .unwrap();
        // INSERT is applicable on WITHOUT ROWID tables, but `RETURNING rowid, *` fails
        // (there is no rowid), so framing must fall back to plain execution.
        let stmts = vec!["INSERT INTO wnr VALUES ('y', '2')".into()];
        let (total, frames) = DatabaseManager::frame_and_apply(&conn, &stmts)
            .await
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(frames[0].op.as_deref(), Some("insert"));
        assert!(frames[0].after.is_none());
        let mut q = conn
            .query("SELECT v FROM wnr WHERE k = 'x' OR k = 'y' ORDER BY k", ())
            .await
            .unwrap();
        let mut vals = Vec::new();
        while let Some(row) = q.next().await.unwrap() {
            vals.push(row.get_value(0).map(value_to_string).unwrap());
        }
        assert_eq!(vals, vec!["1", "2"]);
    }

    #[tokio::test]
    async fn batch_rolls_back_entirely_on_error() {
        let conn = local_conn().await;
        conn.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)", ())
            .await
            .unwrap();
        let stmts = vec![
            "INSERT INTO t (v) VALUES ('kept')".into(),
            "INSERT INTO t (id, v) VALUES (1, 'dupbreaks')".into(),
            "INSERT INTO t (id, v) VALUES (1, 'again')".into(),
        ];
        let err = DatabaseManager::frame_and_apply(&conn, &stmts)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("UNIQUE"), "got: {}", err);
        let mut q = conn.query("SELECT COUNT(*) AS n FROM t", ()).await.unwrap();
        let mut n = 0u64;
        while let Some(row) = q.next().await.unwrap() {
            n = row
                .get_value(0)
                .map(value_to_string)
                .unwrap()
                .parse()
                .unwrap();
        }
        assert_eq!(n, 0, "failed batch must be fully rolled back");
    }
}
