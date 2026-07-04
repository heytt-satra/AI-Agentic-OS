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
    // One-off reminders.
    ReminderAdd { due_ts: i64, text: String, reply: oneshot::Sender<i64> },
    ReminderList { reply: oneshot::Sender<Vec<(i64, i64, String)>> }, // (id, due_ts, text) pending
    ReminderCancel { id: i64, reply: oneshot::Sender<bool> },
    ReminderDue { now: i64, reply: oneshot::Sender<Vec<(i64, String)>> }, // fires + marks them
    // Encrypted secrets vault (value stored as ciphertext).
    SecretSet { name: String, value: String, reply: oneshot::Sender<bool> },
    SecretGet { name: String, reply: oneshot::Sender<Option<String>> },
    SecretList { reply: oneshot::Sender<Vec<String>> },
    SecretRemove { name: String, reply: oneshot::Sender<bool> },
    // User-defined agents (gap 4).
    AgentCreate { name: String, instructions: String, reply: oneshot::Sender<bool> },
    AgentList { reply: oneshot::Sender<Vec<(String, String)>> },
    AgentGet { name: String, reply: oneshot::Sender<Option<String>> },
    AgentDelete { name: String, reply: oneshot::Sender<bool> },
    // Document RAG (gap 3).
    DocIngest { source: String, chunks: Vec<String>, reply: oneshot::Sender<usize> },
    DocSearch { query: String, k: i64, reply: oneshot::Sender<Vec<(String, String, f32)>> },

    // Continuous-learning spine.
    LearnAdd { text: String, kind: String, source: String, reply: oneshot::Sender<String> },
    LearnRecall { query: String, k: i64, reply: oneshot::Sender<Vec<(String, String, f32)>> }, // (kind, text, confidence)
    LearnTop { k: i64, reply: oneshot::Sender<Vec<(String, String, f32)>> }, // highest-confidence profile
    LearnList { reply: oneshot::Sender<Vec<(i64, String, String, f32, i64)>> }, // (id, kind, text, confidence, reinforced)
    LearnDecay { idle_secs: i64, floor: f64, reply: oneshot::Sender<usize> }, // fade stale beliefs, prune below floor

    // Proactive nudges.
    NudgeAdd { text: String, reply: oneshot::Sender<bool> }, // false if a duplicate unshown nudge exists
    NudgeTake { reply: oneshot::Sender<Option<String>> },    // newest unshown nudge, marked shown
    NudgeList { reply: oneshot::Sender<Vec<(i64, String, bool)>> },
    NudgePending { reply: oneshot::Sender<Vec<(i64, String)>> }, // (id, text) not yet reacted to
    NudgeReact { id: i64, reaction: i64, reply: oneshot::Sender<bool> }, // record act(1)/dismiss(-1)
    NudgeReactionStats { reply: oneshot::Sender<(i64, i64)> }, // (acted, dismissed)

    // Causal world model (interventional log).
    CausalLog { tool: String, args: String, context: String, outcome: String, success: bool }, // fire-and-forget
    CausalForTool { tool: String, k: i64, reply: oneshot::Sender<Vec<(String, String, bool)>> }, // (args, outcome, success)
    CausalStats { reply: oneshot::Sender<Vec<(String, i64, i64)>> }, // (tool, total, successes)
    CausalRecent { n: i64, reply: oneshot::Sender<Vec<(String, String, String, bool)>> }, // (tool, args, outcome, success)
    CausalCalibration { reply: oneshot::Sender<(f64, i64)> }, // (calibration 0..1, scored count)

    // Self-direction: hypotheses + goals Jarvis forms and pursues on its own.
    GoalAdd { kind: String, text: String, reply: oneshot::Sender<bool> }, // false if duplicate open goal
    GoalOpen { k: i64, reply: oneshot::Sender<Vec<(i64, String, String)>> }, // (id, kind, text), oldest open first
    GoalSetStatus { id: i64, status: String, note: String, reply: oneshot::Sender<bool> },
    GoalList { reply: oneshot::Sender<Vec<(i64, String, String, String)>> }, // (id, kind, text, status)
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

    // ── one-off reminders ───────────────────────────────────────────────────
    pub async fn reminder_add(&self, due_ts: i64, text: &str) -> i64 {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ReminderAdd { due_ts, text: text.to_string(), reply }).await.is_err() { return -1; }
        rx.await.unwrap_or(-1)
    }
    pub async fn reminders_list(&self) -> Vec<(i64, i64, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ReminderList { reply }).await.is_err() { return Vec::new(); }
        rx.await.unwrap_or_default()
    }
    pub async fn reminder_cancel(&self, id: i64) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ReminderCancel { id, reply }).await.is_err() { return false; }
        rx.await.unwrap_or(false)
    }
    /// Return reminders now due (id, text) and atomically mark them fired.
    pub async fn reminders_due(&self, now: i64) -> Vec<(i64, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::ReminderDue { now, reply }).await.is_err() { return Vec::new(); }
        rx.await.unwrap_or_default()
    }

    // ── encrypted secrets vault ─────────────────────────────────────────────
    /// Store (or overwrite) a secret. `value` MUST already be ciphertext.
    pub async fn secret_set(&self, name: &str, value: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::SecretSet { name: name.to_string(), value: value.to_string(), reply }).await.is_err() { return false; }
        rx.await.unwrap_or(false)
    }
    /// Fetch a secret's stored (encrypted) value by name.
    pub async fn secret_get(&self, name: &str) -> Option<String> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::SecretGet { name: name.to_string(), reply }).await.is_err() { return None; }
        rx.await.ok().flatten()
    }
    /// List secret NAMES only (never values).
    pub async fn secret_list(&self) -> Vec<String> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::SecretList { reply }).await.is_err() { return Vec::new(); }
        rx.await.unwrap_or_default()
    }
    pub async fn secret_remove(&self, name: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::SecretRemove { name: name.to_string(), reply }).await.is_err() { return false; }
        rx.await.unwrap_or(false)
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

    // ── continuous-learning spine ───────────────────────────────────────────
    /// Record a durable learning (deduped/reinforced). Returns "added" or
    /// "reinforced (confidence X)".
    pub async fn learn(&self, text: &str, kind: &str, source: &str) -> String {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::LearnAdd { text: text.to_string(), kind: kind.to_string(), source: source.to_string(), reply }).await.is_err() {
            return "error".to_string();
        }
        rx.await.unwrap_or_else(|_| "error".to_string())
    }
    /// Learnings relevant to a query (kind, text, confidence), ranked by relevance*confidence.
    pub async fn recall_learnings(&self, query: &str, k: i64) -> Vec<(String, String, f32)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::LearnRecall { query: query.to_string(), k, reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
    /// The highest-confidence learnings (the stable "profile"), independent of query.
    pub async fn top_learnings(&self, k: i64) -> Vec<(String, String, f32)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::LearnTop { k, reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
    /// All learnings (id, kind, text, confidence, reinforced) for `jarvis learnings`.
    pub async fn learnings_list(&self) -> Vec<(i64, String, String, f32, i64)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::LearnList { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
    /// Fade learnings not seen in `idle_secs`, prune below `floor`. Returns pruned count.
    pub async fn decay_learnings(&self, idle_secs: i64, floor: f64) -> usize {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::LearnDecay { idle_secs, floor, reply }).await.is_err() {
            return 0;
        }
        rx.await.unwrap_or(0)
    }

    // ── proactive nudges ────────────────────────────────────────────────────
    /// Queue a proactive nudge. Returns false if an identical unshown one exists.
    pub async fn nudge_add(&self, text: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::NudgeAdd { text: text.to_string(), reply }).await.is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }
    /// Take the newest un-surfaced nudge (marks it shown). None if the queue is empty.
    pub async fn nudge_take(&self) -> Option<String> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::NudgeTake { reply }).await.is_err() {
            return None;
        }
        rx.await.ok().flatten()
    }
    /// Recent nudges (id, text, shown) for `jarvis nudges`.
    pub async fn nudges_list(&self) -> Vec<(i64, String, bool)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::NudgeList { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
    /// Nudges (id, text) the user hasn't acted on or dismissed yet - the queue the
    /// HUD mind panel offers Act/Dismiss buttons for (roadmap 5.2).
    pub async fn nudges_pending(&self) -> Vec<(i64, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::NudgePending { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
    /// Record the user's reaction to a nudge: 1 = acted on, -1 = dismissed.
    pub async fn nudge_react(&self, id: i64, reaction: i64) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::NudgeReact { id, reaction, reply }).await.is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }
    /// (acted, dismissed) tallies over all reacted nudges - the signal that
    /// auto-tunes how often Jarvis nudges.
    pub async fn nudge_reaction_stats(&self) -> (i64, i64) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::NudgeReactionStats { reply }).await.is_err() {
            return (0, 0);
        }
        rx.await.unwrap_or((0, 0))
    }

    // ── causal world model (interventions) ──────────────────────────────────
    /// Record one intervention (a consequential tool call and its real outcome).
    /// Fire-and-forget so it never slows the tool path.
    pub async fn causal_log(&self, tool: &str, args: &str, context: &str, outcome: &str, success: bool) {
        let _ = self.tx.send(MemCmd::CausalLog {
            tool: tool.to_string(),
            args: args.chars().take(300).collect(),
            context: context.chars().take(200).collect(),
            outcome: outcome.chars().take(300).collect(),
            success,
        }).await;
    }
    /// Past outcomes of a given action (args, outcome, success), newest first.
    pub async fn causal_for_tool(&self, tool: &str, k: i64) -> Vec<(String, String, bool)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::CausalForTool { tool: tool.to_string(), k, reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
    /// Per-tool (total, successes) tally for `jarvis causal`.
    pub async fn causal_stats(&self) -> Vec<(String, i64, i64)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::CausalStats { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
    /// Recent interventions (tool, args, outcome, success).
    pub async fn causal_recent(&self, n: i64) -> Vec<(String, String, String, bool)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::CausalRecent { n, reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
    /// How well Jarvis's causal predictions have matched reality (roadmap 5.2).
    /// Returns (calibration 0..1, number of scored predictions). Calibration is
    /// 1 - prequential Brier score: for each intervention we take the tool's
    /// success rate BEFORE it as the prediction and the real outcome as truth, so
    /// a perfectly-calibrated forecaster scores 1.0. (0.0, 0) means not enough
    /// history to score yet.
    pub async fn causal_calibration(&self) -> (f64, i64) {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::CausalCalibration { reply }).await.is_err() {
            return (0.0, 0);
        }
        rx.await.unwrap_or((0.0, 0))
    }

    // ── self-direction: hypotheses + goals ──────────────────────────────────
    /// Add a hypothesis or goal Jarvis set for itself. False if an open duplicate exists.
    pub async fn goal_add(&self, kind: &str, text: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::GoalAdd { kind: kind.to_string(), text: text.to_string(), reply }).await.is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }
    /// Oldest open goals/hypotheses (id, kind, text) - the queue Jarvis pursues.
    pub async fn goals_open(&self, k: i64) -> Vec<(i64, String, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::GoalOpen { k, reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }
    /// Update a goal's status (open|testing|confirmed|done|dropped) + a note.
    pub async fn goal_set_status(&self, id: i64, status: &str, note: &str) -> bool {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::GoalSetStatus { id, status: status.to_string(), note: note.to_string(), reply }).await.is_err() {
            return false;
        }
        rx.await.unwrap_or(false)
    }
    /// All goals (id, kind, text, status) for `jarvis goals`.
    pub async fn goals_list(&self) -> Vec<(i64, String, String, String)> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(MemCmd::GoalList { reply }).await.is_err() {
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
         -- One-off reminders: fire once at due_ts. fired=1 after it has been raised.
         CREATE TABLE IF NOT EXISTS reminders (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            due_ts INTEGER NOT NULL, text TEXT NOT NULL, fired INTEGER NOT NULL DEFAULT 0
         );
         -- Encrypted secrets vault: value is AES-256-GCM ciphertext ('enc:...'),
         -- so a stolen DB file never reveals a secret. Keyed by a friendly name.
         CREATE TABLE IF NOT EXISTS secrets (
            name TEXT PRIMARY KEY, value TEXT NOT NULL, ts INTEGER NOT NULL
         );
         -- Document RAG: chunks of the user's ingested files + their embeddings.
         CREATE TABLE IF NOT EXISTS documents (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL,
            source TEXT NOT NULL, chunk TEXT NOT NULL, vec BLOB NOT NULL
         );
         -- Continuous-learning spine: durable things Jarvis has LEARNED about the
         -- user/their work (preferences, facts, heuristics), recalled into future
         -- sessions. confidence grows on reinforcement, source records origin.
         CREATE TABLE IF NOT EXISTS learnings (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, last_seen INTEGER NOT NULL,
            kind TEXT NOT NULL DEFAULT 'fact', text TEXT NOT NULL,
            source TEXT NOT NULL DEFAULT '', confidence REAL NOT NULL DEFAULT 0.6,
            reinforced INTEGER NOT NULL DEFAULT 0, vec BLOB NOT NULL
         );
         -- Proactive nudges: things the background sensing loop noticed and wants
         -- to raise with the user. shown=0 until surfaced in a session.
         CREATE TABLE IF NOT EXISTS nudges (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, text TEXT NOT NULL, shown INTEGER NOT NULL DEFAULT 0
         );
         -- Self-direction: hypotheses Jarvis forms about the user (curiosity) and
         -- goals it sets for itself, which it then tests/pursues on its own.
         -- kind = hypothesis | goal ; status = open | testing | confirmed | done | dropped.
         CREATE TABLE IF NOT EXISTS goals (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, kind TEXT NOT NULL DEFAULT 'goal',
            text TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'open', note TEXT NOT NULL DEFAULT ''
         );
         -- Causal world model: every consequential tool call is a do() intervention
         -- on the real system. We record action -> observed outcome -> success so
         -- Jarvis can learn what actually causes what on THIS machine and predict
         -- before acting. context = a short signature of the situation at the time.
         CREATE TABLE IF NOT EXISTS causal_events (
            id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, tool TEXT NOT NULL,
            args TEXT NOT NULL DEFAULT '', context TEXT NOT NULL DEFAULT '',
            outcome TEXT NOT NULL DEFAULT '', success INTEGER NOT NULL DEFAULT 1
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

    // Idempotent migration: add the nudge reaction column to older DBs that predate
    // it (roadmap 5.2). reaction: 0 = pending, 1 = acted on, -1 = dismissed. SQLite
    // errors if the column already exists, so we ignore the result.
    let _ = conn.execute("ALTER TABLE nudges ADD COLUMN reaction INTEGER NOT NULL DEFAULT 0", []);

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
        MemCmd::ReminderAdd { due_ts, text, reply } => {
            let _ = conn.execute(
                "INSERT INTO reminders (ts, due_ts, text, fired) VALUES (?1, ?2, ?3, 0)",
                params![now_secs(), due_ts, text],
            );
            let _ = reply.send(conn.last_insert_rowid());
        }
        MemCmd::ReminderList { reply } => {
            let rows = (|| -> Result<Vec<(i64, i64, String)>> {
                let mut stmt = conn.prepare("SELECT id, due_ts, text FROM reminders WHERE fired=0 ORDER BY due_ts ASC")?;
                let r = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.filter_map(|x| x.ok()).collect();
                Ok(r)
            })().unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::ReminderCancel { id, reply } => {
            let n = conn.execute("DELETE FROM reminders WHERE id=?1 AND fired=0", params![id]).unwrap_or(0);
            let _ = reply.send(n > 0);
        }
        MemCmd::ReminderDue { now, reply } => {
            // Read the due, un-fired ones, then mark them fired so they raise once.
            let rows = (|| -> Result<Vec<(i64, String)>> {
                let mut stmt = conn.prepare("SELECT id, text FROM reminders WHERE fired=0 AND due_ts <= ?1")?;
                let r = stmt.query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|x| x.ok()).collect();
                Ok(r)
            })().unwrap_or_default();
            for (id, _) in &rows {
                let _ = conn.execute("UPDATE reminders SET fired=1 WHERE id=?1", params![id]);
            }
            let _ = reply.send(rows);
        }
        MemCmd::SecretSet { name, value, reply } => {
            let ok = conn.execute(
                "INSERT OR REPLACE INTO secrets (name, value, ts) VALUES (?1, ?2, ?3)",
                params![name, value, now_secs()],
            ).is_ok();
            let _ = reply.send(ok);
        }
        MemCmd::SecretGet { name, reply } => {
            let v = conn.query_row("SELECT value FROM secrets WHERE name=?1", params![name], |r| r.get::<_, String>(0)).ok();
            let _ = reply.send(v);
        }
        MemCmd::SecretList { reply } => {
            let rows = (|| -> Result<Vec<String>> {
                let mut stmt = conn.prepare("SELECT name FROM secrets ORDER BY name ASC")?;
                let r = stmt.query_map([], |r| r.get::<_, String>(0))?.filter_map(|x| x.ok()).collect();
                Ok(r)
            })().unwrap_or_default();
            let _ = reply.send(rows);
        }
        MemCmd::SecretRemove { name, reply } => {
            let n = conn.execute("DELETE FROM secrets WHERE name=?1", params![name]).unwrap_or(0);
            let _ = reply.send(n > 0);
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
        MemCmd::LearnAdd { text, kind, source, reply } => {
            // Learn, but DON'T pile up near-duplicates: if a very similar learning
            // already exists, REINFORCE it (raise confidence, bump the counter,
            // refresh the wording) instead of inserting a new row. This is how a
            // belief gets stronger the more often it's confirmed.
            let mut result = "skipped (no embedder)".to_string();
            if let Some(emb) = embedder {
                if let Ok(v) = emb.embed(&text) {
                    let mut best: Option<(i64, f32, f64, i64)> = None; // (id, sim, confidence, reinforced)
                    if let Ok(mut stmt) = conn.prepare("SELECT id, vec, confidence, reinforced FROM learnings") {
                        if let Ok(rows) = stmt.query_map([], |r| {
                            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?, r.get::<_, f64>(2)?, r.get::<_, i64>(3)?))
                        }) {
                            for (id, b, conf, rc) in rows.filter_map(|x| x.ok()) {
                                let sim = cosine(&v, &blob_to_vec(&b));
                                if best.as_ref().map_or(true, |(_, s, _, _)| sim > *s) {
                                    best = Some((id, sim, conf, rc));
                                }
                            }
                        }
                    }
                    match best {
                        Some((id, sim, conf, rc)) if sim >= 0.90 => {
                            let newconf = (conf + 0.1).min(0.99);
                            let _ = conn.execute(
                                "UPDATE learnings SET confidence=?1, reinforced=?2, last_seen=?3, text=?4, vec=?5 WHERE id=?6",
                                params![newconf, rc + 1, now_secs(), text, vec_to_blob(&v), id],
                            );
                            result = format!("reinforced (confidence {newconf:.2})");
                        }
                        _ => {
                            if conn.execute(
                                "INSERT INTO learnings (ts, last_seen, kind, text, source, confidence, reinforced, vec) VALUES (?1, ?1, ?2, ?3, ?4, ?5, 0, ?6)",
                                params![now_secs(), kind, text, source, 0.6_f64, vec_to_blob(&v)],
                            ).is_ok() {
                                result = "added".to_string();
                            } else {
                                result = "error".to_string();
                            }
                        }
                    }
                }
            }
            let _ = reply.send(result);
        }
        MemCmd::LearnRecall { query, k, reply } => {
            // Rank by relevance * confidence, so a strongly-held, on-topic belief
            // surfaces above a weakly-held tangential one.
            let mut hits: Vec<(String, String, f32)> = Vec::new();
            if let Some(emb) = embedder {
                if let Ok(qv) = emb.embed(&query) {
                    if let Ok(mut stmt) = conn.prepare("SELECT kind, text, confidence, vec FROM learnings") {
                        if let Ok(rows) = stmt.query_map([], |r| {
                            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?, r.get::<_, Vec<u8>>(3)?))
                        }) {
                            let mut scored: Vec<(f64, String, String, f32)> = rows
                                .filter_map(|x| x.ok())
                                .map(|(kind, text, conf, b)| {
                                    let sim = cosine(&qv, &blob_to_vec(&b));
                                    (sim as f64 * conf, kind, text, conf as f32)
                                })
                                .filter(|(rank, _, _, _)| *rank > 0.12)
                                .collect();
                            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
                            for (_, kind, text, conf) in scored.into_iter().take(k.max(1) as usize) {
                                hits.push((kind, text, conf));
                            }
                        }
                    }
                }
            }
            let _ = reply.send(hits);
        }
        MemCmd::LearnTop { k, reply } => {
            let mut out: Vec<(String, String, f32)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare(
                "SELECT kind, text, confidence FROM learnings ORDER BY confidence DESC, reinforced DESC, last_seen DESC LIMIT ?1",
            ) {
                if let Ok(rows) = stmt.query_map(params![k.max(1)], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, f64>(2)?))
                }) {
                    for (kind, text, conf) in rows.filter_map(|x| x.ok()) {
                        out.push((kind, text, conf as f32));
                    }
                }
            }
            let _ = reply.send(out);
        }
        MemCmd::LearnList { reply } => {
            let mut out: Vec<(i64, String, String, f32, i64)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare(
                "SELECT id, kind, text, confidence, reinforced FROM learnings ORDER BY confidence DESC, last_seen DESC",
            ) {
                if let Ok(rows) = stmt.query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, f64>(3)?, r.get::<_, i64>(4)?))
                }) {
                    for (id, kind, text, conf, rc) in rows.filter_map(|x| x.ok()) {
                        out.push((id, kind, text, conf as f32, rc));
                    }
                }
            }
            let _ = reply.send(out);
        }
        MemCmd::LearnDecay { idle_secs, floor, reply } => {
            // Beliefs fade if never reconfirmed: drop the confidence of learnings
            // not seen recently, then prune any that fell below the floor.
            let cutoff = now_secs() - idle_secs;
            let _ = conn.execute(
                "UPDATE learnings SET confidence = confidence - 0.05 WHERE last_seen < ?1",
                params![cutoff],
            );
            let pruned = conn
                .execute("DELETE FROM learnings WHERE confidence < ?1", params![floor])
                .unwrap_or(0);
            let _ = reply.send(pruned);
        }
        MemCmd::NudgeAdd { text, reply } => {
            // Don't queue the same nudge twice while it's still unshown.
            let dup: i64 = conn
                .query_row("SELECT COUNT(*) FROM nudges WHERE shown=0 AND text=?1", params![text], |r| r.get(0))
                .unwrap_or(0);
            let added = if dup == 0 {
                conn.execute("INSERT INTO nudges (ts, text, shown) VALUES (?1, ?2, 0)", params![now_secs(), text]).is_ok()
            } else {
                false
            };
            let _ = reply.send(added);
        }
        MemCmd::NudgeTake { reply } => {
            let row: Option<(i64, String)> = conn
                .query_row("SELECT id, text FROM nudges WHERE shown=0 ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
                .ok();
            let text = row.map(|(id, t)| {
                let _ = conn.execute("UPDATE nudges SET shown=1 WHERE id=?1", params![id]);
                t
            });
            let _ = reply.send(text);
        }
        MemCmd::NudgeList { reply } => {
            let mut out: Vec<(i64, String, bool)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare("SELECT id, text, shown FROM nudges ORDER BY id DESC LIMIT 30") {
                if let Ok(rows) = stmt.query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0))
                }) {
                    for row in rows.filter_map(|x| x.ok()) {
                        out.push(row);
                    }
                }
            }
            let _ = reply.send(out);
        }
        MemCmd::NudgePending { reply } => {
            let mut out: Vec<(i64, String)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare("SELECT id, text FROM nudges WHERE reaction=0 ORDER BY id DESC LIMIT 10") {
                if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))) {
                    out = rows.filter_map(|x| x.ok()).collect();
                }
            }
            let _ = reply.send(out);
        }
        MemCmd::NudgeReact { id, reaction, reply } => {
            // Clamp to the known reactions and mark it surfaced too.
            let r = if reaction > 0 { 1 } else { -1 };
            let ok = conn
                .execute("UPDATE nudges SET reaction=?1, shown=1 WHERE id=?2", params![r, id])
                .map(|n| n > 0)
                .unwrap_or(false);
            let _ = reply.send(ok);
        }
        MemCmd::NudgeReactionStats { reply } => {
            let acted: i64 = conn.query_row("SELECT COUNT(*) FROM nudges WHERE reaction=1", [], |r| r.get(0)).unwrap_or(0);
            let dismissed: i64 = conn.query_row("SELECT COUNT(*) FROM nudges WHERE reaction=-1", [], |r| r.get(0)).unwrap_or(0);
            let _ = reply.send((acted, dismissed));
        }
        MemCmd::CausalLog { tool, args, context, outcome, success } => {
            let _ = conn.execute(
                "INSERT INTO causal_events (ts, tool, args, context, outcome, success) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![now_secs(), tool, args, context, outcome, success as i64],
            );
        }
        MemCmd::CausalForTool { tool, k, reply } => {
            let mut out: Vec<(String, String, bool)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare("SELECT args, outcome, success FROM causal_events WHERE tool=?1 ORDER BY id DESC LIMIT ?2") {
                if let Ok(rows) = stmt.query_map(params![tool, k.max(1)], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0))
                }) {
                    for row in rows.filter_map(|x| x.ok()) {
                        out.push(row);
                    }
                }
            }
            let _ = reply.send(out);
        }
        MemCmd::CausalStats { reply } => {
            let mut out: Vec<(String, i64, i64)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare(
                "SELECT tool, COUNT(*), SUM(success) FROM causal_events GROUP BY tool ORDER BY COUNT(*) DESC",
            ) {
                if let Ok(rows) = stmt.query_map([], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
                }) {
                    for row in rows.filter_map(|x| x.ok()) {
                        out.push(row);
                    }
                }
            }
            let _ = reply.send(out);
        }
        MemCmd::CausalRecent { n, reply } => {
            let mut out: Vec<(String, String, String, bool)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare("SELECT tool, args, outcome, success FROM causal_events ORDER BY id DESC LIMIT ?1") {
                if let Ok(rows) = stmt.query_map(params![n.max(1)], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)? != 0))
                }) {
                    for row in rows.filter_map(|x| x.ok()) {
                        out.push(row);
                    }
                }
            }
            let _ = reply.send(out);
        }
        MemCmd::CausalCalibration { reply } => {
            let mut events: Vec<(String, bool)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare("SELECT tool, success FROM causal_events ORDER BY id ASC") {
                if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))) {
                    events = rows.filter_map(|x| x.ok()).collect();
                }
            }
            let _ = reply.send(calibration_from(&events));
        }
        MemCmd::GoalAdd { kind, text, reply } => {
            let dup: i64 = conn
                .query_row("SELECT COUNT(*) FROM goals WHERE status='open' AND text=?1", params![text], |r| r.get(0))
                .unwrap_or(0);
            let added = if dup == 0 {
                conn.execute(
                    "INSERT INTO goals (ts, kind, text, status, note) VALUES (?1, ?2, ?3, 'open', '')",
                    params![now_secs(), kind, text],
                ).is_ok()
            } else {
                false
            };
            let _ = reply.send(added);
        }
        MemCmd::GoalOpen { k, reply } => {
            let mut out: Vec<(i64, String, String)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare("SELECT id, kind, text FROM goals WHERE status='open' ORDER BY id ASC LIMIT ?1") {
                if let Ok(rows) = stmt.query_map(params![k.max(1)], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
                }) {
                    for row in rows.filter_map(|x| x.ok()) {
                        out.push(row);
                    }
                }
            }
            let _ = reply.send(out);
        }
        MemCmd::GoalSetStatus { id, status, note, reply } => {
            let ok = conn
                .execute("UPDATE goals SET status=?1, note=?2 WHERE id=?3", params![status, note, id])
                .map(|n| n > 0)
                .unwrap_or(false);
            let _ = reply.send(ok);
        }
        MemCmd::GoalList { reply } => {
            let mut out: Vec<(i64, String, String, String)> = Vec::new();
            if let Ok(mut stmt) = conn.prepare("SELECT id, kind, text, status FROM goals ORDER BY id DESC LIMIT 40") {
                if let Ok(rows) = stmt.query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
                }) {
                    for row in rows.filter_map(|x| x.ok()) {
                        out.push(row);
                    }
                }
            }
            let _ = reply.send(out);
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

// Prequential calibration (roadmap 5.2): given interventions in time order as
// (tool, success), score how well each tool's running success rate predicted the
// NEXT outcome. For each event past a tool's first two, the prior rate is the
// prediction p and the outcome is o (1/0); we accumulate the Brier score (p-o)^2.
// Returns (1 - mean_brier clamped to 0..1, number of scored events). Pure so it
// can be unit-tested without a database.
fn calibration_from(events: &[(String, bool)]) -> (f64, i64) {
    let mut priors: std::collections::HashMap<&str, (f64, f64)> = std::collections::HashMap::new(); // tool -> (total, succ)
    let mut brier_sum = 0.0f64;
    let mut scored = 0i64;
    for (tool, success) in events {
        let e = priors.entry(tool.as_str()).or_insert((0.0, 0.0));
        if e.0 >= 2.0 {
            let p = e.1 / e.0; // predicted success probability from history
            let o = if *success { 1.0 } else { 0.0 };
            brier_sum += (p - o) * (p - o);
            scored += 1;
        }
        e.0 += 1.0;
        if *success {
            e.1 += 1.0;
        }
    }
    let calib = if scored > 0 { (1.0 - brier_sum / scored as f64).clamp(0.0, 1.0) } else { 0.0 };
    (calib, scored)
}

#[cfg(test)]
mod tests {
    use super::calibration_from;

    fn ev(pairs: &[(&str, bool)]) -> Vec<(String, bool)> {
        pairs.iter().map(|(t, s)| (t.to_string(), *s)).collect()
    }

    #[test]
    fn perfectly_predictable_tool_scores_high() {
        // a tool that always succeeds: after the 2-event warmup, every prediction
        // is p=1.0 and the outcome is 1 -> Brier 0 -> calibration 1.0
        let (c, n) = calibration_from(&ev(&[("run_shell", true); 6]));
        assert_eq!(n, 4); // 6 events, first 2 warm up the prior
        assert!((c - 1.0).abs() < 1e-9, "calib was {c}");
    }

    #[test]
    fn a_surprise_lowers_calibration() {
        // three successes then a failure: at the 4th event p=1.0 but o=0 -> a
        // Brier hit, so calibration drops below 1.
        let (c, n) = calibration_from(&ev(&[("t", true), ("t", true), ("t", true), ("t", false)]));
        assert_eq!(n, 2);
        assert!(c < 1.0 && c >= 0.0, "calib was {c}");
    }

    #[test]
    fn too_little_history_scores_nothing() {
        let (c, n) = calibration_from(&ev(&[("a", true), ("b", false)]));
        assert_eq!((c, n), (0.0, 0));
    }
}
