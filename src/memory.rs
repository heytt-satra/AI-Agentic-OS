// ── src/memory.rs : memory as an ACTOR ──────────────────────────────────────
//
// One dedicated OS thread owns the SQLite Connection. Everyone else holds a
// `MemoryHandle` (a cloneable channel sender) and mails it commands. Reads
// carry a `oneshot` return-address so the caller can await the answer.
//
// Why: rusqlite::Connection is !Send (can't be shared across threads) and
// blocking (would stall the async runtime). The actor solves BOTH: the
// Connection lives and dies on its own thread; blocking happens off-runtime.
// This is the eng-review's single-writer design — now justified because the
// heartbeat task and the REPL both need memory concurrently.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use tokio::sync::{mpsc, oneshot};

// ── The messages we can send the actor ──────────────────────────────────────
enum MemCmd {
    Log { role: String, content: String },
    LogAudit { tool: String, args: String, decision: String, ok: bool },
    Count { reply: oneshot::Sender<i64> },
    AuditCount { reply: oneshot::Sender<i64> },
    RecentDialog { n: i64, reply: oneshot::Sender<Vec<(String, String)>> },
    Search { query: String, n: i64, reply: oneshot::Sender<Vec<(String, String)>> },
    RecentAudit { n: i64, reply: oneshot::Sender<Vec<(String, String, bool)>> },
}

// ── The handle other code holds. Clone is cheap (clones a channel sender). ──
#[derive(Clone)]
pub struct MemoryHandle {
    tx: mpsc::Sender<MemCmd>,
}

impl MemoryHandle {
    // Spawn the owner thread and return a handle to it.
    pub fn spawn(path: &str) -> Result<Self> {
        let (tx, mut rx) = mpsc::channel::<MemCmd>(64);
        let path = path.to_string();

        // A PLAIN OS thread (not a tokio task), so blocking SQLite is fine and
        // the !Send Connection is created here and never leaves.
        std::thread::Builder::new()
            .name("jarvis-memory".into())
            .spawn(move || {
                let conn = match open_db(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("memory thread failed to open db: {e}");
                        return;
                    }
                };
                // blocking_recv() blocks this thread until a command arrives.
                // Allowed because we are NOT on a tokio runtime thread.
                while let Some(cmd) = rx.blocking_recv() {
                    handle_cmd(&conn, cmd);
                }
            })
            .context("spawning memory thread")?;

        Ok(MemoryHandle { tx })
    }

    // ── async API: send a command, optionally await a reply ─────────────────
    pub async fn log(&self, role: &str, content: &str) {
        let _ = self
            .tx
            .send(MemCmd::Log { role: role.to_string(), content: content.to_string() })
            .await;
    }

    pub async fn log_audit(&self, tool: &str, args: &str, decision: &str, ok: bool) {
        let _ = self
            .tx
            .send(MemCmd::LogAudit {
                tool: tool.to_string(),
                args: args.to_string(),
                decision: decision.to_string(),
                ok,
            })
            .await;
    }

    pub async fn count(&self) -> i64 {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::Count { reply }).await.is_err() {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    pub async fn audit_count(&self) -> i64 {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::AuditCount { reply }).await.is_err() {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    pub async fn recent_dialog(&self, n: i64) -> Vec<(String, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::RecentDialog { n, reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    // Recent tool-call feedback rows (tool, decision, ok) for the digest.
    pub async fn recent_audit(&self, n: i64) -> Vec<(String, String, bool)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::RecentAudit { n, reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    // Relevance recall: the N past dialog messages most relevant to `query`.
    pub async fn search(&self, query: &str, n: i64) -> Vec<(String, String)> {
        let (reply, rx) = oneshot::channel();
        let cmd = MemCmd::Search { query: query.to_string(), n, reply };
        if self.tx.send(cmd).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
}

// ── Everything below runs ON the owner thread, with exclusive Connection access ─
fn open_db(path: &str) -> Result<Connection> {
    let conn = Connection::open(path).with_context(|| format!("opening db at {path}"))?;
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            role TEXT NOT NULL, content TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS audit (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            tool TEXT NOT NULL, args TEXT NOT NULL,
            decision TEXT NOT NULL, ok INTEGER NOT NULL
         );
         -- Full-text search index over message content. FTS5 ranks matches by
         -- relevance (bm25), so we can recall the MOST RELEVANT past messages
         -- for a query, not just the most recent. rowid mirrors messages.id.
         CREATE VIRTUAL TABLE IF NOT EXISTS mem_fts USING fts5(role UNINDEXED, content);",
    )
    .context("creating tables")?;

    // One-time backfill: if the FTS index is empty but we already have messages
    // (from before this feature existed), index them now.
    let fts_n: i64 = conn.query_row("SELECT count(*) FROM mem_fts", [], |r| r.get(0)).unwrap_or(0);
    let msg_n: i64 = conn.query_row("SELECT count(*) FROM messages", [], |r| r.get(0)).unwrap_or(0);
    if fts_n == 0 && msg_n > 0 {
        conn.execute(
            "INSERT INTO mem_fts(rowid, role, content) SELECT id, role, content FROM messages",
            [],
        )
        .context("backfilling FTS index")?;
    }
    Ok(conn)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn handle_cmd(conn: &Connection, cmd: MemCmd) {
    match cmd {
        MemCmd::Log { role, content } => {
            if conn
                .execute(
                    "INSERT INTO messages (ts, role, content) VALUES (?1, ?2, ?3)",
                    params![now_secs(), role, content],
                )
                .is_ok()
            {
                // Mirror into the FTS index using the same rowid as messages.id.
                let id = conn.last_insert_rowid();
                let _ = conn.execute(
                    "INSERT INTO mem_fts(rowid, role, content) VALUES (?1, ?2, ?3)",
                    params![id, role, content],
                );
            }
        }
        MemCmd::LogAudit { tool, args, decision, ok } => {
            let _ = conn.execute(
                "INSERT INTO audit (ts, tool, args, decision, ok) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![now_secs(), tool, args, decision, ok as i64],
            );
        }
        MemCmd::Count { reply } => {
            let n = conn
                .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
                .unwrap_or(0);
            let _ = reply.send(n);
        }
        MemCmd::AuditCount { reply } => {
            let n = conn
                .query_row("SELECT COUNT(*) FROM audit", [], |r| r.get(0))
                .unwrap_or(0);
            let _ = reply.send(n);
        }
        MemCmd::RecentDialog { n, reply } => {
            let rows = query_recent_dialog(conn, n).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::Search { query, n, reply } => {
            let rows = query_search(conn, &query, n).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::RecentAudit { n, reply } => {
            let rows = query_recent_audit(conn, n).unwrap_or_default();
            let _ = reply.send(rows);
        }
    }
}

fn query_recent_audit(conn: &Connection, n: i64) -> Result<Vec<(String, String, bool)>> {
    let mut stmt = conn
        .prepare("SELECT tool, decision, ok FROM audit ORDER BY id DESC LIMIT ?1")?;
    let rows = stmt.query_map(params![n], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? != 0,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    out.reverse();
    Ok(out)
}

// Turn arbitrary user text into a safe FTS5 MATCH expression. We extract word
// tokens (length >= 2) and join them with OR. This both sanitizes (user text
// can't inject FTS syntax) and broadens the match to "any of these words".
fn to_fts_query(text: &str) -> String {
    let toks: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 2)
        .map(|s| s.to_lowercase())
        .collect();
    toks.join(" OR ")
}

fn query_search(conn: &Connection, query: &str, n: i64) -> Result<Vec<(String, String)>> {
    let match_q = to_fts_query(query);
    if match_q.is_empty() {
        return Ok(Vec::new());
    }
    // ORDER BY rank = best (bm25) matches first. Restrict to dialog turns.
    let mut stmt = conn.prepare(
        "SELECT role, content FROM mem_fts
         WHERE mem_fts MATCH ?1 AND role IN ('user','assistant')
         ORDER BY rank LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![match_q, n], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn query_recent_dialog(conn: &Connection, n: i64) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT role, content FROM messages
         WHERE role IN ('user','assistant')
         ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![n], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    out.reverse();
    Ok(out)
}
