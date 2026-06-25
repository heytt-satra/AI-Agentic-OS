// ── src/tools.rs : Jarvis's hands ───────────────────────────────────────────
//
// Each tool is (1) a DEFINITION we advertise to the model (name + JSON-schema
// args) and (2) an IMPLEMENTATION in Rust that we run when the model asks.
//
// Safety model from the plan:
//   - file ops are SANDBOXED to ./workspace  (model can't read C:\ or escape)
//   - run_shell is Tier-2: it asks YOU to approve before running (HITL gate)
//
// `args_json` arrives as a JSON string the model wrote, e.g. {"path":"a.txt"}.
// We parse it into a small typed struct per tool.

use crate::provider::{FunctionDef, Tool};
use serde::Deserialize;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const WORKSPACE: &str = "workspace"; // all file ops are confined here

// ── Definitions advertised to the model ─────────────────────────────────────
pub fn definitions() -> Vec<Tool> {
    let f = |name: &str, description: &str, params: serde_json::Value| Tool {
        kind: "function".to_string(),
        function: FunctionDef {
            name: name.to_string(),
            description: description.to_string(),
            parameters: params,
        },
    };
    vec![
        f(
            "read_file",
            "Read a UTF-8 text file from the workspace.",
            serde_json::json!({
                "type":"object",
                "properties": { "path": { "type":"string", "description":"relative path inside the workspace" } },
                "required": ["path"]
            }),
        ),
        f(
            "write_file",
            "Create or overwrite a UTF-8 text file in the workspace.",
            serde_json::json!({
                "type":"object",
                "properties": {
                    "path": { "type":"string" },
                    "content": { "type":"string" }
                },
                "required": ["path","content"]
            }),
        ),
        f(
            "fetch_url",
            "HTTP GET a URL and return the response body text (truncated).",
            serde_json::json!({
                "type":"object",
                "properties": { "url": { "type":"string" } },
                "required": ["url"]
            }),
        ),
        f(
            "news_search",
            "Search recent tech, startup, and finance news/discussions (via Hacker News). Use ONCE for current-events questions, then answer from the results.",
            serde_json::json!({
                "type":"object",
                "properties": { "query": { "type":"string", "description":"topic, e.g. 'AI' or 'interest rates'" } },
                "required": ["query"]
            }),
        ),
        f(
            "run_shell",
            "Run a shell command on the user's machine. Requires the user's approval.",
            serde_json::json!({
                "type":"object",
                "properties": { "command": { "type":"string" } },
                "required": ["command"]
            }),
        ),
    ]
}

// What a tool call produced + the feedback signal we record.
pub struct ToolOutcome {
    pub result: String,   // text fed back to the model
    pub decision: String, // auto | approved | denied
    pub ok: bool,         // did it succeed?
}

// Wrap a plain result string as an "auto" (no-approval) outcome, inferring ok
// from whether the result is an error/refusal.
fn auto(result: String) -> ToolOutcome {
    let bad = result.starts_with("ERROR") || result.starts_with("refused") || result.starts_with("DENIED");
    ToolOutcome { result, decision: "auto".to_string(), ok: !bad }
}

// ── Dispatch: the model picked a tool name; run the matching implementation ──
// async because fetch_url awaits the network. The others are sync but fit fine.
pub async fn execute(name: &str, args_json: &str) -> ToolOutcome {
    match name {
        "read_file" => auto(read_file(args_json)),
        "write_file" => auto(write_file(args_json)),
        "fetch_url" => auto(fetch_url(args_json).await),
        "news_search" => auto(news_search(args_json).await),
        "run_shell" => run_shell(args_json), // sets its own decision (approved/denied)
        other => auto(format!("ERROR: unknown tool '{other}'")),
    }
}

// ── Sandbox helper: turn a model-supplied relative path into a safe absolute ─
// one INSIDE the workspace, rejecting absolute paths and any '..' traversal.
fn safe_path(rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(format!("refused: '{rel}' is an absolute path (workspace-only)"));
    }
    if p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(format!("refused: '{rel}' contains '..' (no escaping the workspace)"));
    }
    Ok(Path::new(WORKSPACE).join(p))
}

// ── read_file ───────────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct ReadArgs { path: String }

fn read_file(args_json: &str) -> String {
    let args: ReadArgs = match serde_json::from_str(args_json) {
        Ok(a) => a,
        Err(e) => return format!("ERROR: bad arguments: {e}"),
    };
    let path = match safe_path(&args.path) { Ok(p) => p, Err(e) => return e };
    match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => format!("ERROR reading {}: {e}", path.display()),
    }
}

// ── write_file (Tier 1: auto, but we log it) ────────────────────────────────
#[derive(Deserialize)]
struct WriteArgs { path: String, content: String }

fn write_file(args_json: &str) -> String {
    let args: WriteArgs = match serde_json::from_str(args_json) {
        Ok(a) => a,
        Err(e) => return format!("ERROR: bad arguments: {e}"),
    };
    let path = match safe_path(&args.path) { Ok(p) => p, Err(e) => return e };
    // Make sure the workspace (and any parent dirs) exist.
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("ERROR creating dirs: {e}");
        }
    }
    match std::fs::write(&path, args.content.as_bytes()) {
        Ok(()) => {
            println!("  [audit] wrote {}", path.display()); // observability: log the write
            format!("wrote {} bytes to {}", args.content.len(), path.display())
        }
        Err(e) => format!("ERROR writing {}: {e}", path.display()),
    }
}

// ── fetch_url ───────────────────────────────────────────────────────────────
#[derive(Deserialize)]
struct FetchArgs { url: String }

async fn fetch_url(args_json: &str) -> String {
    let args: FetchArgs = match serde_json::from_str(args_json) {
        Ok(a) => a,
        Err(e) => return format!("ERROR: bad arguments: {e}"),
    };
    match reqwest::get(&args.url).await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            // Truncate so we don't blow up the context (and the token bill).
            let truncated: String = body.chars().take(2000).collect();
            format!("HTTP {status}\n{truncated}")
        }
        Err(e) => format!("ERROR fetching {}: {e}", args.url),
    }
}

// ── news_search (keyless, via the Hacker News Algolia API) ──────────────────
#[derive(Deserialize)]
struct SearchArgs { query: String }

// We only deserialize the fields we use; serde ignores the rest of the JSON.
#[derive(Deserialize)]
struct HnResponse { hits: Vec<HnHit> }
#[derive(Deserialize)]
struct HnHit {
    title: Option<String>,
    url: Option<String>,
    points: Option<i64>,
    num_comments: Option<i64>,
    created_at: Option<String>, // ISO timestamp, so we can show recency
    created_at_i: Option<i64>,  // unix seconds
}

async fn news_search(args_json: &str) -> String {
    let args: SearchArgs = match serde_json::from_str(args_json) {
        Ok(a) => a,
        Err(e) => return format!("ERROR: bad arguments: {e}"),
    };

    // Use search_by_date = NEWEST first (not relevance, which returns old
    // popular stories). Filter to the last ~14 days so results are actually
    // current. created_at_i is unix seconds; build the filter from "now".
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let since = now - 14 * 24 * 3600;
    let filter = format!("created_at_i>{since}");

    let client = reqwest::Client::new();
    let resp = client
        .get("https://hn.algolia.com/api/v1/search_by_date")
        .query(&[
            ("query", args.query.as_str()),
            ("tags", "story"),
            ("numericFilters", filter.as_str()),
            ("hitsPerPage", "8"),
        ])
        .send()
        .await;

    let body = match resp {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => return format!("ERROR fetching news: {e}"),
    };

    let parsed: HnResponse = match serde_json::from_str(&body) {
        Ok(p) => p,
        Err(e) => return format!("ERROR parsing news: {e}"),
    };

    if parsed.hits.is_empty() {
        return format!("No recent stories (last 14 days) for '{}'.", args.query);
    }

    let mut out = format!(
        "Most RECENT stories for '{}' (newest first; these are live, dated):\n",
        args.query
    );
    for (i, h) in parsed.hits.iter().enumerate() {
        let title = h.title.clone().unwrap_or_else(|| "(untitled)".into());
        let url = h.url.clone().unwrap_or_else(|| "(no url)".into());
        let pts = h.points.unwrap_or(0);
        // Prefer the human-readable date; fall back to age in hours.
        let when = h.created_at.clone().unwrap_or_else(|| {
            match h.created_at_i {
                Some(t) => format!("{}h ago", (now - t).max(0) / 3600),
                None => "recent".into(),
            }
        });
        out.push_str(&format!("{}. {title}  [{when}, {pts} pts]\n   {url}\n", i + 1));
    }
    out
}

// ── run_shell (Tier 2: HUMAN APPROVAL required) ─────────────────────────────
#[derive(Deserialize)]
struct ShellArgs { command: String }

fn run_shell(args_json: &str) -> ToolOutcome {
    let args: ShellArgs = match serde_json::from_str(args_json) {
        Ok(a) => a,
        Err(e) => return ToolOutcome { result: format!("ERROR: bad arguments: {e}"), decision: "auto".into(), ok: false },
    };

    // The approval gate. Show exactly what will run, then require a yes.
    println!("\n  ⚠  Jarvis wants to run a shell command:");
    println!("     {}", args.command);
    print!("  Approve? [y/N] ");
    io::stdout().flush().ok();

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return ToolOutcome { result: "DENIED: could not read approval".into(), decision: "denied".into(), ok: false };
    }
    if answer.trim().to_lowercase() != "y" {
        // DENIED is itself a valuable feedback signal (you rejected this action).
        return ToolOutcome { result: "DENIED by user".into(), decision: "denied".into(), ok: false };
    }

    // Approved. On Windows we run via PowerShell; elsewhere via sh.
    let output = if cfg!(windows) {
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &args.command])
            .output()
    } else {
        std::process::Command::new("sh").args(["-c", &args.command]).output()
    };

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            ToolOutcome {
                result: format!("exit={}\nstdout:\n{stdout}\nstderr:\n{stderr}", out.status),
                decision: "approved".to_string(),
                ok: out.status.success(),
            }
        }
        Err(e) => ToolOutcome { result: format!("ERROR running command: {e}"), decision: "approved".into(), ok: false },
    }
}
