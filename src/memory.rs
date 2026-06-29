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

use crate::embeddings::{blob_to_vec, cosine, vec_to_blob, Embedder};
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
    CheckPerm { tool: String, key: String, reply: oneshot::Sender<Option<bool>> },
    RememberPerm { tool: String, key: String, allow: bool },
    LogActivity { kind: String, app: String, detail: String },
    ActivityRecent { n: i64, reply: oneshot::Sender<Vec<(i64, String, String, String)>> },
    ActivitySearch { query: String, n: i64, reply: oneshot::Sender<Vec<(i64, String, String, String)>> },
    // Everything tracked since a timestamp (for timeframe recall).
    ActivitySince { since: i64, query: Option<String>, reply: oneshot::Sender<Vec<(i64, String, String, String)>> },
    // Full-history dumps for the training-dataset exporter (Stage 1).
    AllMessages { reply: oneshot::Sender<Vec<(i64, String, String)>> },
    AllAudit { reply: oneshot::Sender<Vec<(i64, String, String, bool)>> },
    // Durable task list (Power 4).
    TaskAdd { title: String, reply: oneshot::Sender<i64> },
    TaskList { reply: oneshot::Sender<Vec<(i64, String, String)>> },
    TaskSetStatus { id: i64, status: String, reply: oneshot::Sender<bool> },
    // Memory consolidation (Pillar 3): summarize + prune activity older than before_ts.
    ConsolidateActivity { before_ts: i64, reply: oneshot::Sender<(usize, usize)> }, // (pruned, summaries)
    // Self-healing skills (Pillar 4).
    SkillCreate { name: String, description: String, command: String, reply: oneshot::Sender<()> },
    SkillGet { name: String, reply: oneshot::Sender<Option<(String, String)>> }, // (description, command)
    SkillList { reply: oneshot::Sender<Vec<(String, String)>> }, // (name, description)
    SkillRemove { name: String, reply: oneshot::Sender<bool> },
    // Capability tokens (security).
    GrantAdd { capability: String, expires_at: i64, reply: oneshot::Sender<()> },
    GrantActive { capability: String, reply: oneshot::Sender<bool> },
    GrantsList { reply: oneshot::Sender<Vec<(String, i64)>> }, // (capability, secs_remaining)
    // Token usage accounting (Pillar 8).
    AddUsage { model: String, tokens: i64 },
    UsageTotal { reply: oneshot::Sender<(i64, i64)> }, // (calls, tokens)
    // Scheduled agents (Phase 3).
    ScheduleAdd { agent: String, every_secs: i64, reply: oneshot::Sender<i64> },
    ScheduleList { reply: oneshot::Sender<Vec<(i64, String, i64)>> },
    ScheduleRemove { id: i64, reply: oneshot::Sender<bool> },
    ScheduleDue { now: i64, reply: oneshot::Sender<Vec<(i64, String, i64)>> },
    ScheduleMarkRun { id: i64, next_run: i64 },
    // User-defined agents (gap 4).
    AgentCreate { name: String, instructions: String, reply: oneshot::Sender<bool> },
    AgentList { reply: oneshot::Sender<Vec<(String, String)>> },
    AgentGet { name: String, reply: oneshot::Sender<Option<String>> },
    AgentDelete { name: String, reply: oneshot::Sender<bool> },
    // Document RAG (gap 3).
    DocIngest { source: String, chunks: Vec<String>, reply: oneshot::Sender<usize> },
    DocSearch { query: String, k: i64, reply: oneshot::Sender<Vec<(String, String, f32)>> },
    // Leads / outreach engine.
    LeadAdd { lead: Lead, reply: oneshot::Sender<i64> },
    LeadList { reply: oneshot::Sender<Vec<(i64, Lead)>> },
    LeadSetStatus { id: i64, status: String, reply: oneshot::Sender<bool> },
}

// One lead/contact row (without the id/ts the DB assigns).
#[derive(Clone, Default)]
pub struct Lead {
    pub name: String,
    pub org: String,
    pub email: String,
    pub phone: String,
    pub url: String,
    pub note: String,
    pub status: String,
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

                // Try to load the local embedding model. If it fails (offline,
                // download error), we fall back to keyword (FTS) recall.
                let embedder = match Embedder::load() {
                    Ok(e) => {
                        eprintln!("[memory] semantic embeddings ready");
                        Some(e)
                    }
                    Err(e) => {
                        eprintln!("[memory] embeddings unavailable ({e}); using keyword recall");
                        None
                    }
                };
                if let Some(emb) = &embedder {
                    backfill_embeddings(&conn, emb);
                }

                // ANN index cache for large-corpus semantic search (rebuilt lazily).
                let mut ann = crate::ann::AnnCache::default();

                // blocking_recv() blocks this thread until a command arrives.
                // Allowed because we are NOT on a tokio runtime thread.
                while let Some(cmd) = rx.blocking_recv() {
                    handle_cmd(&conn, embedder.as_ref(), &mut ann, cmd);
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

    // Remembered approval: Some(true)=always allow, Some(false)=always deny, None=ask.
    pub async fn check_permission(&self, tool: &str, key: &str) -> Option<bool> {
        let (reply, rx) = oneshot::channel();
        let cmd = MemCmd::CheckPerm { tool: tool.to_string(), key: key.to_string(), reply };
        if self.tx.send(cmd).await.is_err() {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    pub async fn remember_permission(&self, tool: &str, key: &str, allow: bool) {
        let _ = self
            .tx
            .send(MemCmd::RememberPerm { tool: tool.to_string(), key: key.to_string(), allow })
            .await;
    }

    // ── second-brain activity log ───────────────────────────────────────────
    pub async fn log_activity(&self, kind: &str, app: &str, detail: &str) {
        let _ = self
            .tx
            .send(MemCmd::LogActivity { kind: kind.to_string(), app: app.to_string(), detail: detail.to_string() })
            .await;
    }

    pub async fn activity_recent(&self, n: i64) -> Vec<(i64, String, String, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ActivityRecent { n, reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    pub async fn activity_search(&self, query: &str, n: i64) -> Vec<(i64, String, String, String)> {
        let (reply, rx) = oneshot::channel();
        let cmd = MemCmd::ActivitySearch { query: query.to_string(), n, reply };
        if self.tx.send(cmd).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    // All activity since a unix timestamp (optionally keyword-filtered),
    // chronological - the basis for "what did I do between X and Y".
    pub async fn activity_since(&self, since: i64, query: Option<&str>) -> Vec<(i64, String, String, String)> {
        let (reply, rx) = oneshot::channel();
        let cmd = MemCmd::ActivitySince { since, query: query.map(|s| s.to_string()), reply };
        if self.tx.send(cmd).await.is_err() {
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

    // Whole message history (ts, role, content) in chronological order, for the
    // training-dataset exporter.
    pub async fn all_messages(&self) -> Vec<(i64, String, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::AllMessages { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    // Whole audit history (ts, tool, args, ok) in chronological order.
    pub async fn all_audit(&self) -> Vec<(i64, String, String, bool)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::AllAudit { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    // ── durable task list (Power 4) ─────────────────────────────────────────
    pub async fn task_add(&self, title: &str) -> i64 {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::TaskAdd { title: title.to_string(), reply }).await.is_err() {
            return -1;
        }
        rx.await.unwrap_or(-1)
    }

    pub async fn task_list(&self) -> Vec<(i64, String, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::TaskList { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    pub async fn task_set_status(&self, id: i64, status: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::TaskSetStatus { id, status: status.to_string(), reply }).await.is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    // ── memory consolidation (Pillar 3) ──────────────────────────────────────
    pub async fn consolidate_activity(&self, before_ts: i64) -> (usize, usize) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ConsolidateActivity { before_ts, reply }).await.is_err() { return (0, 0); }
        rx.await.unwrap_or((0, 0))
    }

    // ── self-healing skills (Pillar 4) ───────────────────────────────────────
    pub async fn skill_create(&self, name: &str, description: &str, command: &str) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::SkillCreate { name: name.to_string(), description: description.to_string(), command: command.to_string(), reply }).await.is_err() { return; }
        let _ = rx.await;
    }
    pub async fn skill_get(&self, name: &str) -> Option<(String, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::SkillGet { name: name.to_string(), reply }).await.is_err() { return None; }
        rx.await.unwrap_or(None)
    }
    pub async fn skill_list(&self) -> Vec<(String, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::SkillList { reply }).await.is_err() { return Vec::new(); }
        rx.await.unwrap_or_default()
    }
    pub async fn skill_remove(&self, name: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::SkillRemove { name: name.to_string(), reply }).await.is_err() { return false; }
        rx.await.unwrap_or(false)
    }

    // ── capability tokens (security) ─────────────────────────────────────────
    pub async fn grant_add(&self, capability: &str, minutes: i64) {
        let expires_at = now_secs() + minutes.max(1) * 60;
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::GrantAdd { capability: capability.to_string(), expires_at, reply }).await.is_err() { return; }
        let _ = rx.await; // wait for the write so a CLI caller doesn't exit first
    }
    pub async fn grant_active(&self, capability: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::GrantActive { capability: capability.to_string(), reply }).await.is_err() { return false; }
        rx.await.unwrap_or(false)
    }
    pub async fn grants_list(&self) -> Vec<(String, i64)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::GrantsList { reply }).await.is_err() { return Vec::new(); }
        rx.await.unwrap_or_default()
    }

    // ── token usage accounting (Pillar 8) ───────────────────────────────────
    pub async fn add_usage(&self, model: &str, tokens: u64) {
        if tokens == 0 { return; }
        let _ = self.tx.send(MemCmd::AddUsage { model: model.to_string(), tokens: tokens as i64 }).await;
    }
    pub async fn usage_total(&self) -> (i64, i64) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::UsageTotal { reply }).await.is_err() { return (0, 0); }
        rx.await.unwrap_or((0, 0))
    }

    // ── scheduled agents (Phase 3) ──────────────────────────────────────────
    pub async fn schedule_add(&self, agent: &str, every_secs: i64) -> i64 {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ScheduleAdd { agent: agent.to_string(), every_secs, reply }).await.is_err() { return -1; }
        rx.await.unwrap_or(-1)
    }
    pub async fn schedule_list(&self) -> Vec<(i64, String, i64)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ScheduleList { reply }).await.is_err() { return Vec::new(); }
        rx.await.unwrap_or_default()
    }
    pub async fn schedule_remove(&self, id: i64) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ScheduleRemove { id, reply }).await.is_err() { return false; }
        rx.await.unwrap_or(false)
    }
    pub async fn schedule_due(&self, now: i64) -> Vec<(i64, String, i64)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ScheduleDue { now, reply }).await.is_err() { return Vec::new(); }
        rx.await.unwrap_or_default()
    }
    pub async fn schedule_mark_run(&self, id: i64, next_run: i64) {
        let _ = self.tx.send(MemCmd::ScheduleMarkRun { id, next_run }).await;
    }

    // ── user-defined agents (gap 4) ─────────────────────────────────────────
    pub async fn agent_create(&self, name: &str, instructions: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::AgentCreate { name: name.to_string(), instructions: instructions.to_string(), reply }).await.is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    pub async fn agent_list(&self) -> Vec<(String, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::AgentList { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    pub async fn agent_get(&self, name: &str) -> Option<String> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::AgentGet { name: name.to_string(), reply }).await.is_err() {
            return None;
        }
        rx.await.unwrap_or(None)
    }

    pub async fn agent_delete(&self, name: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::AgentDelete { name: name.to_string(), reply }).await.is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }

    // ── document RAG (gap 3) ────────────────────────────────────────────────
    pub async fn doc_ingest(&self, source: &str, chunks: Vec<String>) -> usize {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::DocIngest { source: source.to_string(), chunks, reply }).await.is_err() {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    pub async fn doc_search(&self, query: &str, k: i64) -> Vec<(String, String, f32)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::DocSearch { query: query.to_string(), k, reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    // ── leads / outreach engine ─────────────────────────────────────────────
    pub async fn lead_add(&self, lead: Lead) -> i64 {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::LeadAdd { lead, reply }).await.is_err() {
            return -1;
        }
        rx.await.unwrap_or(-1)
    }

    pub async fn lead_list(&self) -> Vec<(i64, Lead)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::LeadList { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    pub async fn lead_set_status(&self, id: i64, status: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::LeadSetStatus { id, status: status.to_string(), reply }).await.is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
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
         -- Full-text search index over message content (keyword fallback).
         CREATE VIRTUAL TABLE IF NOT EXISTS mem_fts USING fts5(role UNINDEXED, content);
         -- Semantic vectors: one embedding per message, keyed by messages.id.
         CREATE TABLE IF NOT EXISTS embeddings (rowid INTEGER PRIMARY KEY, vec BLOB NOT NULL);
         -- Remembered approval decisions ('always allow/deny this exact action').
         CREATE TABLE IF NOT EXISTS permissions (
            tool TEXT NOT NULL, key TEXT NOT NULL, allow INTEGER NOT NULL,
            PRIMARY KEY (tool, key)
         );
         -- The 'second brain': a log of what you were doing over time.
         -- kind = window | clipboard | screenshot. app + detail describe it.
         CREATE TABLE IF NOT EXISTS activity (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            kind TEXT NOT NULL, app TEXT NOT NULL, detail TEXT NOT NULL
         );
         -- Durable task list (Power 4): multi-step goals survive restarts.
         -- status = open | done | cancelled.
         CREATE TABLE IF NOT EXISTS tasks (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            title TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'open'
         );
         -- User-defined agents (gap 4): named, reusable instructions the user
         -- creates in plain language and runs on demand.
         CREATE TABLE IF NOT EXISTS agents (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            name TEXT NOT NULL UNIQUE, instructions TEXT NOT NULL
         );
         -- Consolidated activity (Pillar 3): per-day-per-app counts kept after the
         -- raw rows are pruned, so the activity log stays bounded.
         CREATE TABLE IF NOT EXISTS activity_summary (
            day TEXT NOT NULL, app TEXT NOT NULL, count INTEGER NOT NULL, PRIMARY KEY(day, app)
         );
         -- Self-healing skills: agent-authored shell-command tools (Pillar 4).
         CREATE TABLE IF NOT EXISTS skills (
            name TEXT PRIMARY KEY, ts INTEGER NOT NULL, description TEXT NOT NULL, command TEXT NOT NULL
         );
         -- Capability tokens (security): time-boxed, user-authorized grants that
         -- auto-approve an otherwise-gated tool/category until they expire.
         CREATE TABLE IF NOT EXISTS grants (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, capability TEXT NOT NULL, expires_at INTEGER NOT NULL
         );
         -- Token usage accounting (Pillar 8): one row per LLM call that reported usage.
         CREATE TABLE IF NOT EXISTS usage (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, model TEXT NOT NULL, tokens INTEGER NOT NULL
         );
         -- Scheduled agents (Phase 3): run a saved agent every N seconds.
         CREATE TABLE IF NOT EXISTS schedules (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            agent TEXT NOT NULL, every_secs INTEGER NOT NULL, next_run INTEGER NOT NULL
         );
         -- Document RAG: chunks of the user's ingested files + their embeddings.
         CREATE TABLE IF NOT EXISTS documents (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            source TEXT NOT NULL, chunk TEXT NOT NULL, vec BLOB NOT NULL
         );
         -- Leads / contacts for the research+outreach engine.
         -- status = new | contacted | replied | dropped.
         CREATE TABLE IF NOT EXISTS leads (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            name TEXT NOT NULL DEFAULT '', org TEXT NOT NULL DEFAULT '',
            email TEXT NOT NULL DEFAULT '', phone TEXT NOT NULL DEFAULT '',
            url TEXT NOT NULL DEFAULT '', note TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'new'
         );",
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

fn handle_cmd(conn: &Connection, embedder: Option<&Embedder>, ann: &mut crate::ann::AnnCache, cmd: MemCmd) {
    match cmd {
        MemCmd::Log { role, content } => {
            if conn
                .execute(
                    "INSERT INTO messages (ts, role, content) VALUES (?1, ?2, ?3)",
                    params![now_secs(), role, content],
                )
                .is_ok()
            {
                let id = conn.last_insert_rowid();
                // Mirror into the FTS index (keyword fallback).
                let _ = conn.execute(
                    "INSERT INTO mem_fts(rowid, role, content) VALUES (?1, ?2, ?3)",
                    params![id, role, content],
                );
                // Store a semantic vector for dialog turns (skip tool spam).
                if let Some(emb) = embedder {
                    if role == "user" || role == "assistant" {
                        if let Ok(v) = emb.embed(&content) {
                            let _ = conn.execute(
                                "INSERT OR REPLACE INTO embeddings(rowid, vec) VALUES (?1, ?2)",
                                params![id, vec_to_blob(&v)],
                            );
                        }
                    }
                }
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
            // Semantic search if we have embeddings; otherwise keyword (FTS).
            let rows = match embedder {
                Some(emb) => semantic_search(conn, emb, &query, n).unwrap_or_default(),
                None => query_search(conn, &query, n).unwrap_or_default(),
            };
            let _ = reply.send(rows);
        }
        MemCmd::RecentAudit { n, reply } => {
            let rows = query_recent_audit(conn, n).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::CheckPerm { tool, key, reply } => {
            let allow: Option<bool> = conn
                .query_row(
                    "SELECT allow FROM permissions WHERE tool=?1 AND key=?2",
                    params![tool, key],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
                .map(|n| n != 0);
            let _ = reply.send(allow);
        }
        MemCmd::RememberPerm { tool, key, allow } => {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO permissions (tool, key, allow) VALUES (?1, ?2, ?3)",
                params![tool, key, allow as i64],
            );
        }
        MemCmd::LogActivity { kind, app, detail } => {
            // Encrypt the sensitive part (window titles + clipboard) at rest.
            let enc = crate::crypto::encrypt(&detail);
            let _ = conn.execute(
                "INSERT INTO activity (ts, kind, app, detail) VALUES (?1, ?2, ?3, ?4)",
                params![now_secs(), kind, app, enc],
            );
        }
        MemCmd::ActivityRecent { n, reply } => {
            let rows = query_activity(conn, None, n).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::ActivitySearch { query, n, reply } => {
            let rows = query_activity(conn, Some(&query), n).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::ActivitySince { since, query, reply } => {
            let rows = query_activity_since(conn, since, query.as_deref()).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::AllMessages { reply } => {
            let rows = query_all_messages(conn).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::AllAudit { reply } => {
            let rows = query_all_audit(conn).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::TaskAdd { title, reply } => {
            let _ = conn.execute(
                "INSERT INTO tasks (ts, title, status) VALUES (?1, ?2, 'open')",
                params![now_secs(), title],
            );
            let _ = reply.send(conn.last_insert_rowid());
        }
        MemCmd::TaskList { reply } => {
            let rows = query_tasks(conn).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::TaskSetStatus { id, status, reply } => {
            let n = conn
                .execute("UPDATE tasks SET status=?2 WHERE id=?1", params![id, status])
                .unwrap_or(0);
            let _ = reply.send(n > 0);
        }
        MemCmd::ConsolidateActivity { before_ts, reply } => {
            // Load the old rows, summarize to (day, app, count), accumulate into
            // activity_summary, then prune the raw rows. Bounds the log's growth.
            let rows = (|| -> Result<Vec<(i64, String, String, String)>> {
                let mut stmt = conn.prepare("SELECT ts, kind, app FROM activity WHERE ts < ?1")?;
                let r = stmt.query_map(params![before_ts], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, String::new()))
                })?.filter_map(|x| x.ok()).collect();
                Ok(r)
            })().unwrap_or_default();
            let pruned = rows.len();
            let summaries = crate::proactivity::summarize_days(&rows);
            for (day, app, count) in &summaries {
                let _ = conn.execute(
                    "INSERT INTO activity_summary (day, app, count) VALUES (?1, ?2, ?3)
                     ON CONFLICT(day, app) DO UPDATE SET count = count + ?3",
                    params![day, app, *count as i64],
                );
            }
            if pruned > 0 {
                let _ = conn.execute("DELETE FROM activity WHERE ts < ?1", params![before_ts]);
            }
            let _ = reply.send((pruned, summaries.len()));
        }
        MemCmd::SkillCreate { name, description, command, reply } => {
            let _ = conn.execute(
                "INSERT INTO skills (name, ts, description, command) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(name) DO UPDATE SET description=?3, command=?4, ts=?2",
                params![name, now_secs(), description, command],
            );
            let _ = reply.send(());
        }
        MemCmd::SkillGet { name, reply } => {
            let row = conn.query_row(
                "SELECT description, command FROM skills WHERE name=?1",
                params![name],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            ).ok();
            let _ = reply.send(row);
        }
        MemCmd::SkillList { reply } => {
            let rows = (|| -> Result<Vec<(String, String)>> {
                let mut stmt = conn.prepare("SELECT name, description FROM skills ORDER BY name ASC")?;
                let r = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.filter_map(|x| x.ok()).collect();
                Ok(r)
            })().unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::SkillRemove { name, reply } => {
            let n = conn.execute("DELETE FROM skills WHERE name=?1", params![name]).unwrap_or(0);
            let _ = reply.send(n > 0);
        }
        MemCmd::GrantAdd { capability, expires_at, reply } => {
            let _ = conn.execute(
                "INSERT INTO grants (ts, capability, expires_at) VALUES (?1, ?2, ?3)",
                params![now_secs(), capability, expires_at],
            );
            let _ = reply.send(());
        }
        MemCmd::GrantActive { capability, reply } => {
            let now = now_secs();
            let active: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM grants WHERE capability=?1 AND expires_at > ?2",
                    params![capability, now],
                    |r| r.get::<_, i64>(0),
                )
                .map(|n| n > 0)
                .unwrap_or(false);
            let _ = reply.send(active);
        }
        MemCmd::GrantsList { reply } => {
            let now = now_secs();
            let rows = (|| -> Result<Vec<(String, i64)>> {
                let mut stmt = conn.prepare("SELECT capability, expires_at FROM grants WHERE expires_at > ?1 ORDER BY expires_at DESC")?;
                let r = stmt.query_map(params![now], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? - now)))?.filter_map(|x| x.ok()).collect();
                Ok(r)
            })().unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::AddUsage { model, tokens } => {
            let _ = conn.execute(
                "INSERT INTO usage (ts, model, tokens) VALUES (?1, ?2, ?3)",
                params![now_secs(), model, tokens],
            );
        }
        MemCmd::UsageTotal { reply } => {
            let row = conn
                .query_row("SELECT COUNT(*), COALESCE(SUM(tokens),0) FROM usage", [], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                })
                .unwrap_or((0, 0));
            let _ = reply.send(row);
        }
        MemCmd::ScheduleAdd { agent, every_secs, reply } => {
            let next = now_secs() + every_secs;
            let _ = conn.execute(
                "INSERT INTO schedules (ts, agent, every_secs, next_run) VALUES (?1, ?2, ?3, ?4)",
                params![now_secs(), agent, every_secs, next],
            );
            let _ = reply.send(conn.last_insert_rowid());
        }
        MemCmd::ScheduleList { reply } => {
            let rows = (|| -> Result<Vec<(i64, String, i64)>> {
                let mut stmt = conn.prepare("SELECT id, agent, every_secs FROM schedules ORDER BY id ASC")?;
                let r = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.filter_map(|x| x.ok()).collect();
                Ok(r)
            })().unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::ScheduleRemove { id, reply } => {
            let n = conn.execute("DELETE FROM schedules WHERE id=?1", params![id]).unwrap_or(0);
            let _ = reply.send(n > 0);
        }
        MemCmd::ScheduleDue { now, reply } => {
            let rows = (|| -> Result<Vec<(i64, String, i64)>> {
                let mut stmt = conn.prepare("SELECT id, agent, every_secs FROM schedules WHERE next_run <= ?1")?;
                let r = stmt.query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.filter_map(|x| x.ok()).collect();
                Ok(r)
            })().unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::ScheduleMarkRun { id, next_run } => {
            let _ = conn.execute("UPDATE schedules SET next_run=?2 WHERE id=?1", params![id, next_run]);
        }
        MemCmd::AgentCreate { name, instructions, reply } => {
            let ok = conn.execute(
                "INSERT OR REPLACE INTO agents (ts, name, instructions) VALUES (?1, ?2, ?3)",
                params![now_secs(), name, instructions],
            ).is_ok();
            let _ = reply.send(ok);
        }
        MemCmd::AgentList { reply } => {
            let rows = (|| -> Result<Vec<(String, String)>> {
                let mut stmt = conn.prepare("SELECT name, instructions FROM agents ORDER BY name ASC")?;
                let r = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|x| x.ok()).collect();
                Ok(r)
            })().unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::AgentGet { name, reply } => {
            let v = conn.query_row("SELECT instructions FROM agents WHERE name=?1", params![name], |r| r.get::<_, String>(0)).ok();
            let _ = reply.send(v);
        }
        MemCmd::AgentDelete { name, reply } => {
            let n = conn.execute("DELETE FROM agents WHERE name=?1", params![name]).unwrap_or(0);
            let _ = reply.send(n > 0);
        }
        MemCmd::DocIngest { source, chunks, reply } => {
            let mut n = 0usize;
            if let Some(emb) = embedder {
                for ch in &chunks {
                    if let Ok(v) = emb.embed(ch) {
                        if conn.execute(
                            "INSERT INTO documents (ts, source, chunk, vec) VALUES (?1, ?2, ?3, ?4)",
                            params![now_secs(), source, ch, vec_to_blob(&v)],
                        ).is_ok() {
                            n += 1;
                        }
                    }
                }
            }
            let _ = reply.send(n);
        }
        MemCmd::DocSearch { query, k, reply } => {
            // Brute-force cosine is sub-ms at small scale; above this many chunks
            // we switch to the cached HNSW index (Pillar 3) to stay fast.
            const ANN_THRESHOLD: i64 = 2000;
            let mut hits: Vec<(String, String, f32)> = Vec::new();
            if let Some(emb) = embedder {
                if let Ok(qv) = emb.embed(&query) {
                    let count: i64 = conn.query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0)).unwrap_or(0);
                    if count >= ANN_THRESHOLD {
                        // Rebuild the ANN index only when the corpus changed.
                        if ann.built_for != count as usize || ann.index.is_none() {
                            let mut vecs = Vec::new();
                            let mut meta = Vec::new();
                            if let Ok(mut stmt) = conn.prepare("SELECT source, chunk, vec FROM documents") {
                                if let Ok(rows) = stmt.query_map([], |r| {
                                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))
                                }) {
                                    for (s, c, b) in rows.filter_map(|x| x.ok()) {
                                        vecs.push(blob_to_vec(&b));
                                        meta.push((s, c));
                                    }
                                }
                            }
                            ann.rebuild(count as usize, vecs, meta);
                        }
                        if let Some(idx) = &ann.index {
                            for (i, score) in idx.search(&qv, k.max(1) as usize) {
                                if let Some((s, c)) = ann.meta.get(i) {
                                    hits.push((s.clone(), c.clone(), score));
                                }
                            }
                        }
                    } else if let Ok(mut stmt) = conn.prepare("SELECT source, chunk, vec FROM documents") {
                        if let Ok(rows) = stmt.query_map([], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))
                        }) {
                            let mut scored: Vec<(f32, String, String)> = rows
                                .filter_map(|x| x.ok())
                                .map(|(s, c, b)| (cosine(&qv, &blob_to_vec(&b)), s, c))
                                .collect();
                            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                            for (score, s, c) in scored.into_iter().take(k.max(1) as usize) {
                                hits.push((s, c, score));
                            }
                        }
                    }
                }
            }
            let _ = reply.send(hits);
        }
        MemCmd::LeadAdd { lead, reply } => {
            // Dedupe: if a lead with the same email or phone already exists,
            // return its id instead of inserting a duplicate.
            let dup: Option<i64> = if !lead.email.is_empty() {
                conn.query_row("SELECT id FROM leads WHERE lower(email)=lower(?1)", params![lead.email], |r| r.get(0)).ok()
            } else {
                None
            }
            .or_else(|| {
                if !lead.phone.is_empty() {
                    conn.query_row("SELECT id FROM leads WHERE phone=?1", params![lead.phone], |r| r.get(0)).ok()
                } else {
                    None
                }
            });
            if let Some(id) = dup {
                let _ = reply.send(id);
            } else {
                let status = if lead.status.is_empty() { "new".to_string() } else { lead.status.clone() };
                let _ = conn.execute(
                    "INSERT INTO leads (ts, name, org, email, phone, url, note, status) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![now_secs(), lead.name, lead.org, lead.email, lead.phone, lead.url, lead.note, status],
                );
                let _ = reply.send(conn.last_insert_rowid());
            }
        }
        MemCmd::LeadList { reply } => {
            let rows = query_leads(conn).unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::LeadSetStatus { id, status, reply } => {
            let n = conn
                .execute("UPDATE leads SET status=?2 WHERE id=?1", params![id, status])
                .unwrap_or(0);
            let _ = reply.send(n > 0);
        }
    }
}

// All leads not dropped, oldest first.
fn query_leads(conn: &Connection) -> Result<Vec<(i64, Lead)>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, org, email, phone, url, note, status FROM leads \
         WHERE status != 'dropped' ORDER BY id ASC LIMIT 500",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Lead {
                    name: r.get(1)?, org: r.get(2)?, email: r.get(3)?, phone: r.get(4)?,
                    url: r.get(5)?, note: r.get(6)?, status: r.get(7)?,
                },
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

// Tasks that still matter (not cancelled), oldest first: (id, title, status).
fn query_tasks(conn: &Connection) -> Result<Vec<(i64, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, title, status FROM tasks WHERE status != 'cancelled' ORDER BY id ASC LIMIT 100",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

// Full message history in chronological order (ts, role, content).
fn query_all_messages(conn: &Connection) -> Result<Vec<(i64, String, String)>> {
    let mut stmt = conn.prepare("SELECT ts, role, content FROM messages ORDER BY id ASC")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

// Full audit history in chronological order (ts, tool, args, ok).
fn query_all_audit(conn: &Connection) -> Result<Vec<(i64, String, String, bool)>> {
    let mut stmt = conn.prepare("SELECT ts, tool, args, ok FROM audit ORDER BY id ASC")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)? != 0))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

// All activity at or after `since` (unix secs), oldest first, optional keyword.
fn query_activity_since(conn: &Connection, since: i64, query: Option<&str>) -> Result<Vec<(i64, String, String, String)>> {
    let mut rows = Vec::new();
    if let Some(q) = query.filter(|q| !q.trim().is_empty()) {
        let like = format!("%{q}%");
        // NOTE: encrypted detail won't match a LIKE filter; we widen and filter
        // after decrypt below.
        let mut stmt = conn.prepare(
            "SELECT ts, kind, app, detail FROM activity WHERE ts >= ?1 ORDER BY id ASC LIMIT 2000",
        )?;
        let mapped = stmt.query_map(params![since], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        let needle = like.trim_matches('%').to_lowercase();
        for r in mapped {
            let (ts, kind, app, detail) = r?;
            let detail = crate::crypto::decrypt(&detail);
            if app.to_lowercase().contains(&needle) || detail.to_lowercase().contains(&needle) {
                rows.push((ts, kind, app, detail));
            }
        }
    } else {
        let mut stmt = conn.prepare(
            "SELECT ts, kind, app, detail FROM activity WHERE ts >= ?1 ORDER BY id ASC LIMIT 2000",
        )?;
        let mapped = stmt.query_map(params![since], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })?;
        for r in mapped {
            let (ts, kind, app, detail) = r?;
            rows.push((ts, kind, app, crate::crypto::decrypt(&detail)));
        }
    }
    Ok(rows)
}

fn query_activity(conn: &Connection, query: Option<&str>, n: i64) -> Result<Vec<(i64, String, String, String)>> {
    // detail is encrypted at rest, so we fetch recent rows, decrypt, then filter.
    let mut stmt = conn.prepare("SELECT ts, kind, app, detail FROM activity ORDER BY id DESC LIMIT ?1")?;
    let mapped = stmt.query_map(params![n.max(1)], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
    })?;
    let needle = query.map(|q| q.to_lowercase());
    let mut rows = Vec::new();
    for r in mapped {
        let (ts, kind, app, detail) = r?;
        let detail = crate::crypto::decrypt(&detail);
        let keep = match &needle {
            Some(q) => app.to_lowercase().contains(q) || detail.to_lowercase().contains(q),
            None => true,
        };
        if keep {
            rows.push((ts, kind, app, detail));
        }
    }
    rows.reverse(); // chronological
    Ok(rows)
}

// Embed any dialog messages that don't yet have a vector (e.g. messages saved
// before embeddings existed). One-time cost on first run with the model.
fn backfill_embeddings(conn: &Connection, emb: &Embedder) {
    let rows: Vec<(i64, String)> = {
        let mut stmt = match conn.prepare(
            "SELECT id, content FROM messages
             WHERE role IN ('user','assistant')
               AND id NOT IN (SELECT rowid FROM embeddings)",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mapped = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map(|it| it.filter_map(|x| x.ok()).collect::<Vec<_>>())
            .unwrap_or_default();
        mapped
    };
    if rows.is_empty() {
        return;
    }
    eprintln!("[memory] backfilling {} embeddings (one time)...", rows.len());
    for (id, content) in rows {
        if let Ok(v) = emb.embed(&content) {
            let _ = conn.execute(
                "INSERT OR REPLACE INTO embeddings(rowid, vec) VALUES (?1, ?2)",
                params![id, vec_to_blob(&v)],
            );
        }
    }
}

// Semantic recall: embed the query, score every stored dialog vector by cosine
// similarity, return the top-N most MEANINGFULLY similar messages. Brute force
// is fine at personal scale (thousands of rows).
fn semantic_search(conn: &Connection, emb: &Embedder, query: &str, n: i64) -> Result<Vec<(String, String)>> {
    let qv = emb.embed(query)?;
    let mut stmt = conn.prepare(
        "SELECT m.role, m.content, e.vec FROM embeddings e
         JOIN messages m ON m.id = e.rowid
         WHERE m.role IN ('user','assistant')",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Vec<u8>>(2)?,
        ))
    })?;

    let mut scored: Vec<(f32, String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let (role, content, blob) = row?;
        if !seen.insert(content.clone()) {
            continue; // dedupe identical content
        }
        let score = cosine(&qv, &blob_to_vec(&blob));
        scored.push((score, role, content));
    }
    // Highest similarity first.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    Ok(scored
        .into_iter()
        .take(n as usize)
        .map(|(_, role, content)| (role, content))
        .collect())
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

// Common filler words. Matching on these makes "what is my company?" rank the
// repeated QUESTION above the ANSWER, because every question shares them. We
// drop them so the query is the meaningful words only (e.g. just "company").
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "to", "of", "in", "on",
    "for", "and", "or", "my", "me", "i", "you", "your", "it", "its", "this",
    "that", "what", "who", "where", "when", "why", "how", "do", "does", "did",
    "can", "could", "would", "should", "with", "about", "tell", "give", "get",
];

// Turn arbitrary user text into a safe FTS5 MATCH expression: meaningful word
// tokens (>= 2 chars, not stopwords) joined with OR. Sanitizes too (user text
// can't inject FTS syntax). If everything was a stopword, fall back to all
// tokens so we still match something.
fn to_fts_query(text: &str) -> String {
    let all: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| s.len() >= 2)
        .map(|s| s.to_lowercase())
        .collect();
    let meaningful: Vec<String> = all
        .iter()
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .cloned()
        .collect();
    let chosen = if meaningful.is_empty() { all } else { meaningful };
    chosen.join(" OR ")
}

fn query_search(conn: &Connection, query: &str, n: i64) -> Result<Vec<(String, String)>> {
    let match_q = to_fts_query(query);
    if match_q.is_empty() {
        return Ok(Vec::new());
    }
    // Fetch MORE than we need, then dedupe identical content in Rust. Without
    // this, repeated identical messages (e.g. the same question asked twice)
    // crowd out the top-N and starve the actual answer. (Keyword search is
    // lexical; semantic embeddings — coming next — fix this properly.)
    let fetch = (n * 4).max(12);
    let mut stmt = conn.prepare(
        "SELECT role, content FROM mem_fts
         WHERE mem_fts MATCH ?1 AND role IN ('user','assistant')
         ORDER BY rank LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![match_q, fetch], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for r in rows {
        let (role, content) = r?;
        if seen.insert(content.clone()) {
            out.push((role, content));
            if out.len() as i64 >= n {
                break;
            }
        }
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
