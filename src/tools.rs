// ── src/tools.rs : Jarvis's hands (now full-device) ─────────────────────────
//
// The SANDBOX IS GONE on purpose: these tools can touch the whole machine.
// Safety no longer lives here — it lives in policy.rs + the approval flow in
// the agent loop. Dangerous tools (write_file, run_shell, delete_path,
// open_path) are gated there; safe ones (read_file, list_dir, fetch_url,
// news_search) run automatically.
//
// execute() returns a plain result string fed back to the model.

use crate::provider::{FunctionDef, Tool};
use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use serde::Deserialize;

pub fn definitions() -> Vec<Tool> {
    let f = |name: &str, description: &str, params: serde_json::Value| Tool {
        kind: "function".to_string(),
        function: FunctionDef { name: name.to_string(), description: description.to_string(), parameters: params },
    };
    let str_prop = |name: &str, desc: &str| {
        serde_json::json!({"type":"object","properties":{name:{"type":"string","description":desc}},"required":[name]})
    };
    vec![
        f("read_file", "Read a text file by path (absolute or relative).", str_prop("path", "file path")),
        f("write_file", "Create or overwrite a text file at a path. Requires approval.",
          serde_json::json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]})),
        f("list_dir", "List the files and folders in a directory.", str_prop("path", "directory path (use '.' for current)")),
        f("delete_path", "Delete a file or folder. Requires approval. Irreversible.", str_prop("path", "path to delete")),
        f("open_path", "Open an app, file, or URL with the OS default handler. Requires approval.", str_prop("target", "app name, file path, or URL")),
        f("run_shell", "Run any shell command on this machine (PowerShell on Windows). Requires approval. This is the universal tool — file ops, apps, system settings, installs, automation.", str_prop("command", "the command")),
        f("type_text", "Type text into whatever app/window is currently focused. Requires approval.", str_prop("text", "the text to type")),
        f("press_keys", "Press a keyboard shortcut into the focused window, e.g. 'ctrl+s', 'alt+tab', 'enter'. Requires approval.", str_prop("combo", "key combo like ctrl+s")),
        f("mouse_click", "Move the mouse to screen coords (x,y) and left-click. Requires approval. Use with screen vision to know where to click.",
          serde_json::json!({"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"}},"required":["x","y"]})),
        f("fetch_url", "HTTP GET a URL, return the body text (truncated).", str_prop("url", "the URL")),
        f("news_search", "Search recent tech/startup/finance news (Hacker News, newest first). Use once for current events.", str_prop("query", "topic")),
    ]
}

// Dispatch. async because some tools await the network.
pub async fn execute(name: &str, args_json: &str) -> String {
    match name {
        "read_file" => read_file(args_json),
        "write_file" => write_file(args_json),
        "list_dir" => list_dir(args_json),
        "delete_path" => delete_path(args_json),
        "open_path" => open_path(args_json),
        "run_shell" => run_shell(args_json),
        "type_text" => type_text(args_json),
        "press_keys" => press_keys(args_json),
        "mouse_click" => mouse_click(args_json),
        "fetch_url" => fetch_url(args_json).await,
        "news_search" => news_search(args_json).await,
        other => format!("ERROR: unknown tool '{other}'"),
    }
}

// A result is "ok" unless it begins with our error/denied markers.
pub fn result_ok(result: &str) -> bool {
    !(result.starts_with("ERROR") || result.starts_with("DENIED"))
}

#[derive(Deserialize)]
struct PathArg { path: String }
#[derive(Deserialize)]
struct WriteArgs { path: String, content: String }
#[derive(Deserialize)]
struct TargetArg { target: String }
#[derive(Deserialize)]
struct UrlArg { url: String }
#[derive(Deserialize)]
struct ShellArg { command: String }
#[derive(Deserialize)]
struct SearchArgs { query: String }

fn read_file(args: &str) -> String {
    let a: PathArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    match std::fs::read_to_string(&a.path) {
        Ok(t) => t.chars().take(8000).collect(),
        Err(e) => format!("ERROR reading {}: {e}", a.path),
    }
}

fn write_file(args: &str) -> String {
    let a: WriteArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    if let Some(parent) = std::path::Path::new(&a.path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    match std::fs::write(&a.path, a.content.as_bytes()) {
        Ok(()) => format!("wrote {} bytes to {}", a.content.len(), a.path),
        Err(e) => format!("ERROR writing {}: {e}", a.path),
    }
}

fn list_dir(args: &str) -> String {
    let a: PathArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    match std::fs::read_dir(&a.path) {
        Ok(rd) => {
            let mut out = format!("Contents of {}:\n", a.path);
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let kind = if entry.path().is_dir() { "dir " } else { "file" };
                out.push_str(&format!("  [{kind}] {name}\n"));
            }
            out
        }
        Err(e) => format!("ERROR listing {}: {e}", a.path),
    }
}

fn delete_path(args: &str) -> String {
    let a: PathArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let p = std::path::Path::new(&a.path);
    let r = if p.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
    match r {
        Ok(()) => format!("deleted {}", a.path),
        Err(e) => format!("ERROR deleting {}: {e}", a.path),
    }
}

fn open_path(args: &str) -> String {
    let a: TargetArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let spawn = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/c", "start", "", &a.target]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(&a.target).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(&a.target).spawn()
    };
    match spawn {
        Ok(_) => format!("opened {}", a.target),
        Err(e) => format!("ERROR opening {}: {e}", a.target),
    }
}

fn run_shell(args: &str) -> String {
    let a: ShellArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let output = if cfg!(windows) {
        std::process::Command::new("powershell").args(["-NoProfile", "-Command", &a.command]).output()
    } else {
        std::process::Command::new("sh").args(["-c", &a.command]).output()
    };
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut s = format!("exit={}\n", out.status);
            if !stdout.trim().is_empty() { s.push_str(&format!("stdout:\n{}\n", stdout.chars().take(4000).collect::<String>())); }
            if !stderr.trim().is_empty() { s.push_str(&format!("stderr:\n{}\n", stderr.chars().take(2000).collect::<String>())); }
            s
        }
        Err(e) => format!("ERROR running command: {e}"),
    }
}

// ── input simulation (app & window control) ────────────────────────────────
#[derive(Deserialize)]
struct TextArg { text: String }
#[derive(Deserialize)]
struct ComboArg { combo: String }
#[derive(Deserialize)]
struct ClickArgs { x: i32, y: i32 }

fn new_enigo() -> Result<Enigo, String> {
    Enigo::new(&Settings::default()).map_err(|e| format!("ERROR: input device: {e}"))
}

fn type_text(args: &str) -> String {
    let a: TextArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let mut enigo = match new_enigo() { Ok(e) => e, Err(e) => return e };
    match enigo.text(&a.text) {
        Ok(()) => format!("typed {} chars", a.text.len()),
        Err(e) => format!("ERROR typing: {e}"),
    }
}

fn map_key(token: &str) -> Key {
    match token {
        "ctrl" | "control" => Key::Control,
        "alt" => Key::Alt,
        "shift" => Key::Shift,
        "win" | "meta" | "cmd" | "super" => Key::Meta,
        "enter" | "return" => Key::Return,
        "tab" => Key::Tab,
        "esc" | "escape" => Key::Escape,
        "space" => Key::Space,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        other => Key::Unicode(other.chars().next().unwrap_or(' ')),
    }
}
fn is_modifier(t: &str) -> bool {
    matches!(t, "ctrl" | "control" | "alt" | "shift" | "win" | "meta" | "cmd" | "super")
}

fn press_keys(args: &str) -> String {
    let a: ComboArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let mut enigo = match new_enigo() { Ok(e) => e, Err(e) => return e };
    let parts: Vec<String> = a.combo.split('+').map(|s| s.trim().to_lowercase()).collect();
    let mods: Vec<Key> = parts.iter().filter(|p| is_modifier(p)).map(|p| map_key(p)).collect();
    let finals: Vec<Key> = parts.iter().filter(|p| !is_modifier(p)).map(|p| map_key(p)).collect();
    for m in &mods { let _ = enigo.key(*m, Direction::Press); }
    for k in &finals { let _ = enigo.key(*k, Direction::Click); }
    for m in mods.iter().rev() { let _ = enigo.key(*m, Direction::Release); }
    format!("pressed {}", a.combo)
}

fn mouse_click(args: &str) -> String {
    let a: ClickArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let mut enigo = match new_enigo() { Ok(e) => e, Err(e) => return e };
    let _ = enigo.move_mouse(a.x, a.y, Coordinate::Abs);
    match enigo.button(Button::Left, Direction::Click) {
        Ok(()) => format!("clicked at {},{}", a.x, a.y),
        Err(e) => format!("ERROR clicking: {e}"),
    }
}

async fn fetch_url(args: &str) -> String {
    let a: UrlArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    match reqwest::get(&a.url).await {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            format!("HTTP {status}\n{}", body.chars().take(2000).collect::<String>())
        }
        Err(e) => format!("ERROR fetching {}: {e}", a.url),
    }
}

// ── news_search via Hacker News Algolia (by date = newest first) ────────────
#[derive(Deserialize)]
struct HnResponse { hits: Vec<HnHit> }
#[derive(Deserialize)]
struct HnHit {
    title: Option<String>,
    url: Option<String>,
    points: Option<i64>,
    created_at: Option<String>,
    created_at_i: Option<i64>,
}

async fn news_search(args: &str) -> String {
    let a: SearchArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let filter = format!("created_at_i>{}", now - 14 * 24 * 3600);
    let client = reqwest::Client::new();
    let resp = client
        .get("https://hn.algolia.com/api/v1/search_by_date")
        .query(&[("query", a.query.as_str()), ("tags", "story"), ("numericFilters", filter.as_str()), ("hitsPerPage", "8")])
        .send().await;
    let body = match resp { Ok(r) => r.text().await.unwrap_or_default(), Err(e) => return format!("ERROR fetching news: {e}") };
    let parsed: HnResponse = match serde_json::from_str(&body) { Ok(p) => p, Err(e) => return format!("ERROR parsing news: {e}") };
    if parsed.hits.is_empty() { return format!("No recent stories for '{}'.", a.query); }
    let mut out = format!("Most RECENT stories for '{}' (newest first):\n", a.query);
    for (i, h) in parsed.hits.iter().enumerate() {
        let title = h.title.clone().unwrap_or_else(|| "(untitled)".into());
        let url = h.url.clone().unwrap_or_else(|| "(no url)".into());
        let pts = h.points.unwrap_or(0);
        let when = h.created_at.clone().unwrap_or_else(|| match h.created_at_i { Some(t) => format!("{}h ago", (now - t).max(0) / 3600), None => "recent".into() });
        out.push_str(&format!("{}. {title}  [{when}, {pts} pts]\n   {url}\n", i + 1));
    }
    out
}
