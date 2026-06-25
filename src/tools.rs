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
        f("open_app", "Launch an application by name (e.g. 'notepad', 'chrome', 'code'). Resolves via the OS so it works for installed apps. Requires approval. Use this for apps; use open_path for files/URLs.", str_prop("name", "application name")),
        f("wait", "Pause for N seconds. Use after opening an app to let it appear and take focus before typing.",
          serde_json::json!({"type":"object","properties":{"seconds":{"type":"integer"}},"required":["seconds"]})),
        f("paste_text", "Type text reliably by pasting it (clipboard + Ctrl+V) into the focused app. PREFER THIS over type_text for any real text. Requires approval.", str_prop("text", "the text to paste")),
        f("type_text", "Type text key-by-key into the focused app (use only for special cases; prefer paste_text). Requires approval.", str_prop("text", "the text to type")),
        f("press_keys", "Press a keyboard shortcut into the focused window, e.g. 'ctrl+s', 'alt+tab', 'enter'. Requires approval.", str_prop("combo", "key combo like ctrl+s")),
        f("mouse_click", "Move the mouse to screen coords (x,y) and left-click. Requires approval. Use with screen vision to know where to click.",
          serde_json::json!({"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"}},"required":["x","y"]})),
        f("see_screen", "Take a screenshot and analyze it with a vision model — lets you SEE what's on screen (read content, find UI elements, get click coordinates). Requires approval (sends your screen to a vision model).", str_prop("question", "what to look for, e.g. 'where is the Save button? give x,y'")),
        f("browse_url", "Open a URL in a real headless browser (runs JavaScript) and return the rendered page text. Better than fetch_url for modern sites.", str_prop("url", "the URL to load")),
        f("browse_js", "Open a URL in a headless browser and run a JavaScript snippet on the page (click, fill forms, extract data). Requires approval. Return value is sent back.",
          serde_json::json!({"type":"object","properties":{"url":{"type":"string"},"script":{"type":"string","description":"JS to evaluate, e.g. document.querySelector('#x').click()"}},"required":["url","script"]})),
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
        "open_app" => open_app(args_json),
        "wait" => wait_tool(args_json).await,
        "paste_text" => paste_text(args_json),
        "type_text" => type_text(args_json),
        "press_keys" => press_keys(args_json),
        "mouse_click" => mouse_click(args_json),
        "see_screen" => see_screen(args_json).await,
        "browse_url" => browse_url(args_json).await,
        "browse_js" => browse_js(args_json).await,
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

// ── app launch / wait / reliable paste ──────────────────────────────────────
#[derive(Deserialize)]
struct NameArg { name: String }
#[derive(Deserialize)]
struct SecondsArg { seconds: u64 }

fn open_app(args: &str) -> String {
    let a: NameArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    // Start-Process resolves installed apps + anything on PATH, and errors
    // clearly if the app isn't found (unlike the silent `start`).
    let escaped = a.name.replace('\'', "''");
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!("Start-Process '{escaped}'")])
        .output();
    match out {
        Ok(o) if o.status.success() => format!("launched {}", a.name),
        Ok(o) => format!("ERROR: couldn't launch '{}': {}", a.name, String::from_utf8_lossy(&o.stderr).trim().chars().take(200).collect::<String>()),
        Err(e) => format!("ERROR launching {}: {e}", a.name),
    }
}

async fn wait_tool(args: &str) -> String {
    let a: SecondsArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let secs = a.seconds.min(15);
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    format!("waited {secs}s")
}

// Reliable text entry: put the text on the clipboard, then send Ctrl+V. Avoids
// the stuck/repeated-key problems of per-character simulation.
fn paste_text(args: &str) -> String {
    let a: TextArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let mut clipboard = match arboard::Clipboard::new() { Ok(c) => c, Err(e) => return format!("ERROR: clipboard: {e}") };
    if let Err(e) = clipboard.set_text(a.text.clone()) {
        return format!("ERROR: set clipboard: {e}");
    }
    std::thread::sleep(std::time::Duration::from_millis(400)); // let focus settle
    let mut enigo = match new_enigo() { Ok(e) => e, Err(e) => return e };
    let _ = enigo.key(Key::Control, Direction::Press);
    let _ = enigo.key(Key::Unicode('v'), Direction::Click);
    let _ = enigo.key(Key::Control, Direction::Release);
    std::thread::sleep(std::time::Duration::from_millis(100)); // keep clipboard alive through paste
    format!("pasted {} chars", a.text.len())
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

// ── screen vision: screenshot + a vision model ──────────────────────────────
#[derive(Deserialize)]
struct VisionArg { question: String }

// Sync helper: capture + encode the screen to a base64 data URL. ALL the
// non-Send types (Monitor, image) live and die here, so the async caller's
// future stays Send (required by spawned tasks).
fn screenshot_data_url() -> Result<String, String> {
    use base64::Engine as _;
    let monitors = xcap::Monitor::all().map_err(|e| format!("ERROR: screen capture: {e}"))?;
    let monitor = monitors.into_iter().next().ok_or("ERROR: no monitor found")?;
    let img = monitor.capture_image().map_err(|e| format!("ERROR capturing screen: {e}"))?;
    let mut bytes: Vec<u8> = Vec::new();
    let dynimg = xcap::image::DynamicImage::ImageRgba8(img);
    let mut cursor = std::io::Cursor::new(&mut bytes);
    dynimg
        .write_to(&mut cursor, xcap::image::ImageFormat::Png)
        .map_err(|e| format!("ERROR encoding screenshot: {e}"))?;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes)))
}

async fn see_screen(args: &str) -> String {
    let a: VisionArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };

    let data_url = match screenshot_data_url() { Ok(u) => u, Err(e) => return e };

    // ask a VISION model about it (via OpenRouter)
    let key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
    if key.is_empty() {
        return "ERROR: OPENROUTER_API_KEY not set".into();
    }
    let model = std::env::var("OPENROUTER_VISION_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".into());
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 700,
        "messages": [{ "role": "user", "content": [
            { "type": "text", "text": a.question },
            { "type": "image_url", "image_url": { "url": data_url } }
        ]}]
    });
    let client = reqwest::Client::new();
    let resp = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("Authorization", format!("Bearer {key}"))
        .header("HTTP-Referer", "https://lensr.in")
        .header("X-Title", "Jarvis-OS")
        .json(&body)
        .send()
        .await;
    let text = match resp {
        Ok(r) => {
            let s = r.status();
            let t = r.text().await.unwrap_or_default();
            if !s.is_success() {
                return format!("ERROR vision {s}: {} (set OPENROUTER_VISION_MODEL to a vision-capable model)", t.chars().take(300).collect::<String>());
            }
            t
        }
        Err(e) => return format!("ERROR vision request: {e}"),
    };
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    let answer = v["choices"][0]["message"]["content"].as_str().unwrap_or("(no vision response)");
    format!("Screen analysis: {answer}")
}

// ── browser automation (real headless Chrome via CDP) ───────────────────────
#[derive(Deserialize)]
struct BrowseArg { url: String }
#[derive(Deserialize)]
struct BrowseJsArgs { url: String, script: String }

// headless_chrome is synchronous, so we run it on a blocking thread. The
// browser is created and dropped inside the closure (so nothing !Send escapes).
fn run_browser(url: String, script: Option<String>) -> Result<String, String> {
    use headless_chrome::{Browser, LaunchOptions};
    let opts = LaunchOptions::default_builder()
        .headless(true)
        .build()
        .map_err(|e| format!("ERROR: browser options: {e}"))?;
    let browser = Browser::new(opts)
        .map_err(|e| format!("ERROR launching Chrome ({e}). Is Chrome or Edge installed?"))?;
    let tab = browser.new_tab().map_err(|e| format!("ERROR: new tab: {e}"))?;
    tab.navigate_to(&url).map_err(|e| format!("ERROR navigating: {e}"))?;
    tab.wait_until_navigated().map_err(|e| format!("ERROR loading: {e}"))?;

    let js = script.unwrap_or_else(|| "document.body.innerText".to_string());
    let result = tab.evaluate(&js, true).map_err(|e| format!("ERROR running JS: {e}"))?;
    let out = match result.value {
        Some(v) => v.as_str().map(|s| s.to_string()).unwrap_or_else(|| v.to_string()),
        None => "(no return value)".to_string(),
    };
    Ok(out.chars().take(4000).collect())
}

async fn browse_url(args: &str) -> String {
    let a: BrowseArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    match tokio::task::spawn_blocking(move || run_browser(a.url, None)).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => e,
        Err(e) => format!("ERROR: browser task: {e}"),
    }
}

async fn browse_js(args: &str) -> String {
    let a: BrowseJsArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    match tokio::task::spawn_blocking(move || run_browser(a.url, Some(a.script))).await {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => e,
        Err(e) => format!("ERROR: browser task: {e}"),
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
