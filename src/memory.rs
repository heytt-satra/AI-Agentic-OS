// ── src/memory.rs : Jarvis's long-term memory (SQLite) ──────────────────────
//
// One SQLite file holds every message. This is the seed of the plan's memory
// system; later it grows facts + vector recall, but the shape starts here.
//
// NOTE for later (from the eng review): rusqlite's Connection is synchronous
// and !Send. For now we use ONE connection on the main task, which is fine for
// a single-user console. When we add the always-on daemon, writes move to a
// dedicated writer thread and reads use a pool. We are deliberately simple now.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

pub struct Memory {
    conn: Connection,
}

impl Memory {
    // Open (or create) the database file and ensure the table exists.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("opening db at {path}"))?;
        // WAL mode = better concurrency + crash safety (writes go to a -wal file).
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (
                id      INTEGER PRIMARY KEY,
                ts      INTEGER NOT NULL,   -- unix seconds
                role    TEXT    NOT NULL,   -- user | assistant | tool
                content TEXT    NOT NULL
            );",
        )
        .context("creating messages table")?;
        Ok(Memory { conn })
    }

    // Append one message. `?1, ?2, ?3` are bound parameters: NEVER format SQL
    // with string interpolation (that's how SQL injection happens).
    pub fn log(&self, role: &str, content: &str) -> Result<()> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn
            .execute(
                "INSERT INTO messages (ts, role, content) VALUES (?1, ?2, ?3)",
                params![ts, role, content],
            )
            .context("inserting message")?;
        Ok(())
    }

    // How many messages we've ever stored.
    pub fn count(&self) -> Result<i64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .context("counting messages")?;
        Ok(n)
    }

    // The last N messages, oldest-first (naive recall — last-N for now).
    // Not wired into the loop yet; v0.2 uses this to seed context from past
    // sessions. #[allow(dead_code)] silences "unused" until then.
    #[allow(dead_code)]
    pub fn recent(&self, n: i64) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT role, content FROM messages ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map(params![n], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        out.reverse(); // query was newest-first; flip to chronological
        Ok(out)
    }
}
