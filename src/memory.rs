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
         );",
    )
    .context("creating tables")?;
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
            let _ = conn.execute(
                "INSERT INTO messages (ts, role, content) VALUES (?1, ?2, ?3)",
                params![now_secs(), role, content],
            );
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
    }
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
