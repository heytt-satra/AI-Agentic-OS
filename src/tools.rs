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
        f("click_on", "See the screen and click on a described UI element (e.g. 'the Save button', 'the search box'). Screenshots, locates it with vision, then clicks. Requires approval. This is the reliable way to click things.", str_prop("target", "what to click, in plain words")),
        f("browse_url", "Open a URL in a real headless browser (runs JavaScript) and return the rendered page text. Better than fetch_url for modern sites.", str_prop("url", "the URL to load")),
        f("browse_js", "Open a URL in a headless browser and run a JavaScript snippet on the page (click, fill forms, extract data). Requires approval. Return value is sent back.",
          serde_json::json!({"type":"object","properties":{"url":{"type":"string"},"script":{"type":"string","description":"JS to evaluate, e.g. document.querySelector('#x').click()"}},"required":["url","script"]})),
        f("fetch_url", "HTTP GET a URL, return the body text (truncated).", str_prop("url", "the URL")),
        f("news_search", "Search recent tech/startup/finance news (Hacker News, newest first). Use once for current events.", str_prop("query", "topic")),
        f("recall_activity", "Look up what the user has been doing (their tracked app/window/clipboard activity = their 'second brain'). Use for 'what was I doing', 'what apps did I use', 'how long in X'. Optional query filters by app/keyword.",
          serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"optional app/keyword filter; empty = most recent activity"}},"required":[]})),
    ]
}

// Dispatch. async because some tools await the network. `mem` is passed so
// memory-backed tools (recall_activity) can query the second brain.
pub async fn execute(name: &str, args_json: &str, mem: &crate::memory::MemoryHandle) -> String {
    match name {
        "recall_activity" => recall_activity(args_json, mem).await,
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
        "click_on" => click_on(args_json).await,
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

// Resolve natural paths to the user's REAL folders using the OS known-folder
// API (so 'desktop' maps to OneDrive\Desktop when redirected, not a fake one).
fn resolve_path(p: &str) -> String {
    let t = p.trim();
    let home = dirs::home_dir().map(|h| h.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    let mut norm = t.replace('\\', "/");

    // Strip a leading ~ or the home-dir prefix so an absolute path like
    // C:/Users/heytt/Desktop/x OR ~/Desktop/x reduces to "desktop/x", which
    // then remaps to the REAL (OneDrive-redirected) Desktop below.
    if norm == "~" {
        return home.replace('/', "\\");
    }
    if let Some(r) = norm.strip_prefix("~/") {
        norm = r.to_string();
    } else if !home.is_empty() {
        let nl = norm.to_lowercase();
        let hl = home.to_lowercase();
        if let Some(stripped) = nl.strip_prefix(&format!("{hl}/")) {
            let _ = stripped;
            norm = norm[home.len() + 1..].to_string();
        }
    }

    let mut parts = norm.splitn(2, '/');
    let first = parts.next().unwrap_or("").to_lowercase();
    let rest = parts.next().filter(|r| !r.is_empty());

    let base: Option<std::path::PathBuf> = match first.as_str() {
        "~" | "home" => dirs::home_dir(),
        "desktop" => dirs::desktop_dir(),
        "downloads" | "download" => dirs::download_dir(),
        "documents" | "docs" => dirs::document_dir(),
        "pictures" => dirs::picture_dir(),
        "music" => dirs::audio_dir(),
        "videos" | "video" => dirs::video_dir(),
        _ => None,
    };
    if let Some(mut b) = base {
        if let Some(r) = rest {
            b.push(r);
        }
        return b.to_string_lossy().to_string();
    }
    t.to_string()
}

fn read_file(args: &str) -> String {
    let a: PathArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let path = resolve_path(&a.path);
    match std::fs::read_to_string(&path) {
        Ok(t) => t.chars().take(8000).collect(),
        Err(e) => format!("ERROR reading {path}: {e}"),
    }
}

fn write_file(args: &str) -> String {
    let a: WriteArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let path = resolve_path(&a.path);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    match std::fs::write(&path, a.content.as_bytes()) {
        Ok(()) => format!("wrote {} bytes to {path}", a.content.len()),
        Err(e) => format!("ERROR writing {path}: {e}"),
    }
}

fn list_dir(args: &str) -> String {
    let a: PathArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let path = resolve_path(&a.path);
    match std::fs::read_dir(&path) {
        Ok(rd) => {
            let mut entries: Vec<(bool, String, u64)> = Vec::new();
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let md = entry.metadata().ok();
                let is_dir = md.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                let size = md.map(|m| m.len()).unwrap_or(0);
                entries.push((is_dir, name, size));
            }
            entries.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.to_lowercase().cmp(&b.1.to_lowercase())));
            let mut out = format!("Contents of {path} ({} items):\n", entries.len());
            for (is_dir, name, size) in entries {
                if is_dir {
                    out.push_str(&format!("  [dir]  {name}\n"));
                } else {
                    out.push_str(&format!("  [file] {name} ({})\n", human_size(size)));
                }
            }
            out
        }
        Err(e) => format!("ERROR listing {path}: {e}"),
    }
}

fn human_size(bytes: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut s = bytes as f64;
    let mut i = 0;
    while s >= 1024.0 && i < 3 {
        s /= 1024.0;
        i += 1;
    }
    format!("{s:.1} {}", U[i])
}

fn delete_path(args: &str) -> String {
    let a: PathArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let resolved = resolve_path(&a.path);
    let p = std::path::Path::new(&resolved);
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
    // Search the Start Menu for a matching app shortcut (handles installed GUI
    // apps like the Codex app), and launch it. Fall back to Start-Process by
    // name (PATH / App Paths) if no shortcut matches.
    let name_lit = format!("'{}'", a.name.replace('\'', "''"));
    // 1) GUI app shortcut in the Start Menu -> launch it.
    // 2) else a CLI tool on PATH (e.g. codex) -> open it in a terminal window.
    // 3) else let the OS try by name.
    let script = r#"$n=__NAME__;
$sm=@("$env:ProgramData\Microsoft\Windows\Start Menu\Programs","$env:APPDATA\Microsoft\Windows\Start Menu\Programs");
$lnk=Get-ChildItem -Path $sm -Recurse -Filter *.lnk -ErrorAction SilentlyContinue | Where-Object { $_.BaseName -like "*$n*" } | Select-Object -First 1 -ExpandProperty FullName;
if($lnk){ Start-Process $lnk; "opened app: $lnk" }
elseif(Get-Command $n -ErrorAction SilentlyContinue){ Start-Process powershell -ArgumentList '-NoExit','-Command',$n; "opened $n in a terminal" }
else { Start-Process $n; "started $n" }"#
        .replace("__NAME__", &name_lit);
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { format!("opened {}", a.name) } else { s }
        }
        Ok(o) => format!("ERROR: couldn't open '{}': {}", a.name, String::from_utf8_lossy(&o.stderr).trim().chars().take(200).collect::<String>()),
        Err(e) => format!("ERROR opening {}: {e}", a.name),
    }
}

async fn wait_tool(args: &str) -> String {
    let a: SecondsArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let secs = a.seconds.min(15);
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    format!("waited {secs}s")
}

// Reliable text entry: clipboard + Ctrl+V. Per-character key simulation caused
// the stuck/repeated-key bug ("Lensr mmmm"), so BOTH paste_text and type_text
// route here.
fn do_paste(text: &str) -> String {
    let mut clipboard = match arboard::Clipboard::new() { Ok(c) => c, Err(e) => return format!("ERROR: clipboard: {e}") };
    if let Err(e) = clipboard.set_text(text.to_string()) {
        return format!("ERROR: set clipboard: {e}");
    }
    std::thread::sleep(std::time::Duration::from_millis(350)); // let focus settle
    let mut enigo = match new_enigo() { Ok(e) => e, Err(e) => return e };
    let _ = enigo.key(Key::Control, Direction::Press);
    let _ = enigo.key(Key::Unicode('v'), Direction::Click);
    let _ = enigo.key(Key::Control, Direction::Release);
    std::thread::sleep(std::time::Duration::from_millis(80));
    format!("entered {} chars", text.len())
}

fn paste_text(args: &str) -> String {
    match serde_json::from_str::<TextArg>(args) {
        Ok(a) => do_paste(&a.text),
        Err(e) => format!("ERROR: bad args: {e}"),
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
    paste_text(args) // route to the reliable clipboard paste
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
fn screenshot_data_url() -> Result<(String, u32, u32), String> {
    use base64::Engine as _;
    let monitors = xcap::Monitor::all().map_err(|e| format!("ERROR: screen capture: {e}"))?;
    let monitor = monitors.into_iter().next().ok_or("ERROR: no monitor found")?;
    let img = monitor.capture_image().map_err(|e| format!("ERROR capturing screen: {e}"))?;
    let (w, h) = (img.width(), img.height());
    let mut bytes: Vec<u8> = Vec::new();
    let dynimg = xcap::image::DynamicImage::ImageRgba8(img);
    let mut cursor = std::io::Cursor::new(&mut bytes);
    dynimg
        .write_to(&mut cursor, xcap::image::ImageFormat::Png)
        .map_err(|e| format!("ERROR encoding screenshot: {e}"))?;
    let url = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes));
    Ok((url, w, h))
}

// Reusable: ask a vision model a question about an image (returns text or ERROR).
async fn vision_ask(data_url: &str, prompt: &str) -> String {
    let key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
    if key.is_empty() {
        return "ERROR: OPENROUTER_API_KEY not set".into();
    }
    let model = std::env::var("OPENROUTER_VISION_MODEL").unwrap_or_else(|_| "openai/gpt-4o-mini".into());
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 700,
        "messages": [{ "role": "user", "content": [
            { "type": "text", "text": prompt },
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
    match resp {
        Ok(r) => {
            let s = r.status();
            let t = r.text().await.unwrap_or_default();
            if !s.is_success() {
                return format!("ERROR vision {s}: {} (set OPENROUTER_VISION_MODEL to a vision-capable model)", t.chars().take(300).collect::<String>());
            }
            let v: serde_json::Value = serde_json::from_str(&t).unwrap_or_default();
            v["choices"][0]["message"]["content"].as_str().unwrap_or("(no vision response)").to_string()
        }
        Err(e) => format!("ERROR vision request: {e}"),
    }
}

async fn see_screen(args: &str) -> String {
    let a: VisionArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let (data_url, _w, _h) = match screenshot_data_url() { Ok(x) => x, Err(e) => return e };
    let answer = vision_ask(&data_url, &a.question).await;
    if answer.starts_with("ERROR") { return answer; }
    format!("Screen analysis: {answer}")
}

// ── see-then-act: screenshot, ask vision for coordinates, then click ─────────
#[derive(Deserialize)]
struct ClickOnArg { target: String }

fn extract_xy(s: &str) -> Option<(i64, i64)> {
    // The model may wrap JSON in prose/code fences; grab the first {...}.
    let start = s.find('{')?;
    let end = s[start..].find('}')? + start + 1;
    let v: serde_json::Value = serde_json::from_str(&s[start..end]).ok()?;
    Some((v.get("x")?.as_i64()?, v.get("y")?.as_i64()?))
}

async fn click_on(args: &str) -> String {
    let a: ClickOnArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let (data_url, w, h) = match screenshot_data_url() { Ok(x) => x, Err(e) => return e };
    let prompt = format!(
        "This is a {w}x{h} pixel screenshot. Find the UI element that best matches: \"{}\". \
         Reply with ONLY JSON giving the pixel coordinates of its CENTER to click: \
         {{\"x\":<int>,\"y\":<int>}}. If it is not visible, reply {{\"x\":-1,\"y\":-1}}.",
        a.target
    );
    let ans = vision_ask(&data_url, &prompt).await;
    if ans.starts_with("ERROR") { return ans; }
    match extract_xy(&ans) {
        Some((x, y)) if x >= 0 && y >= 0 => {
            let mut enigo = match new_enigo() { Ok(e) => e, Err(e) => return e };
            let _ = enigo.move_mouse(x as i32, y as i32, Coordinate::Abs);
            let _ = enigo.button(Button::Left, Direction::Click);
            format!("clicked '{}' at {x},{y}", a.target)
        }
        _ => format!("Could not locate '{}' on screen (vision: {})", a.target, ans.chars().take(120).collect::<String>()),
    }
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

async fn recall_activity(args: &str, mem: &crate::memory::MemoryHandle) -> String {
    let q = serde_json::from_str::<serde_json::Value>(args)
        .ok()
        .and_then(|v| v.get("query").and_then(|x| x.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();
    let rows = if q.trim().is_empty() {
        mem.activity_recent(40).await
    } else {
        mem.activity_search(&q, 40).await
    };
    if rows.is_empty() {
        return "No tracked activity yet (the second-brain tracker may be off or just started).".into();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut out = String::from("Your recent activity (most recent last):\n");
    for (ts, kind, app, detail) in rows {
        let mins = ((now - ts).max(0)) / 60;
        let d: String = detail.chars().take(90).collect();
        out.push_str(&format!("- {mins}m ago [{kind}] {app} {d}\n"));
    }
    out
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
