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
        f("install_software", "Download and install an application using the system package manager (winget on Windows, brew on macOS, apt on Linux). Use this to ACQUIRE software the user asks for, then open_app to launch it. e.g. 'Visual Studio Code', '7zip', 'git'.", str_prop("name", "app or package name/id")),
        f("wait", "Pause for N seconds. Use after opening an app to let it appear and take focus before typing.",
          serde_json::json!({"type":"object","properties":{"seconds":{"type":"integer"}},"required":["seconds"]})),
        f("paste_text", "Type text reliably by pasting it (clipboard + Ctrl+V) into the focused app. PREFER THIS over type_text for any real text. Requires approval.", str_prop("text", "the text to paste")),
        f("type_text", "Type text key-by-key into the focused app (use only for special cases; prefer paste_text). Requires approval.", str_prop("text", "the text to type")),
        f("press_keys", "Press a keyboard shortcut into the focused window, e.g. 'ctrl+s', 'alt+tab', 'enter'. Requires approval.", str_prop("combo", "key combo like ctrl+s")),
        f("mouse_click", "Move the mouse to screen coords (x,y) and left-click. Requires approval. Use with screen vision to know where to click.",
          serde_json::json!({"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"}},"required":["x","y"]})),
        f("see_screen", "Take a screenshot and analyze it with a vision model — lets you SEE what's on screen (read content, find UI elements, get click coordinates). Requires approval (sends your screen to a vision model).", str_prop("question", "what to look for, e.g. 'where is the Save button? give x,y'")),
        f("click_on", "See the screen and click on a described UI element (e.g. 'the Save button', 'the search box'). Screenshots, locates it with vision, then clicks. Requires approval. This is the reliable way to click things.", str_prop("target", "what to click, in plain words")),
        f("operate_app", "Autonomously operate whatever is on screen to accomplish a goal. It loops: screenshot, decide ONE action (click/type/key), do it, re-check, until done. Use this to DRIVE an already-open GUI app to a result, e.g. 'in the open editor, make a new file, type a hello world, and save it'. For one-off clicks use click_on instead.", str_prop("goal", "what to accomplish on screen, in plain words")),
        f("browse_url", "Open a URL in a real headless browser (runs JavaScript) and return the rendered page text. Better than fetch_url for modern sites.", str_prop("url", "the URL to load")),
        f("browse_js", "Open a URL in a headless browser and run a JavaScript snippet on the page (click, fill forms, extract data). Requires approval. Return value is sent back.",
          serde_json::json!({"type":"object","properties":{"url":{"type":"string"},"script":{"type":"string","description":"JS to evaluate, e.g. document.querySelector('#x').click()"}},"required":["url","script"]})),
        f("fetch_url", "HTTP GET a URL, return the body text (truncated).", str_prop("url", "the URL")),
        f("web_search", "Search the web for ANYTHING - leads, companies, people, jobs, suppliers, research, current facts - and get back the top results as title, url, and snippet. This is how you FIND things online before fetching or browsing them. Use it whenever the user wants you to find or look something up.", str_prop("query", "what to search for")),
        f("news_search", "Search recent tech/startup/finance news (Hacker News, newest first). Use once for current events.", str_prop("query", "topic")),

        // ── research + outreach engine: find -> collect -> reach out
        f("extract_contacts", "Fetch a web page and pull out the email addresses and phone numbers on it. Use on a lead's website (often the home or contact page) to find how to reach them.", str_prop("url", "the page URL to scan")),
        f("lead_add", "Save a lead/contact to the outreach list (survives restarts). Use after web_search/extract_contacts to keep the good ones. Only name is required; include email, phone, org, url, note when known.",
          serde_json::json!({"type":"object","properties":{"name":{"type":"string"},"org":{"type":"string"},"email":{"type":"string"},"phone":{"type":"string"},"url":{"type":"string"},"note":{"type":"string"}},"required":["name"]})),
        f("lead_list", "List saved leads with id, name, org, email, phone, url and status (new/contacted/replied/dropped).",
          serde_json::json!({"type":"object","properties":{},"required":[]})),
        f("lead_update", "Update a lead's status by id: new | contacted | replied | dropped.",
          serde_json::json!({"type":"object","properties":{"id":{"type":"integer"},"status":{"type":"string"}},"required":["id","status"]})),
        f("email_compose", "Open a prefilled email in the user's Gmail in their browser, ready to review and send (they are already logged in, so they just glance and hit Send). Use this to send outreach. After composing, mark the lead 'contacted' with lead_update.",
          serde_json::json!({"type":"object","properties":{"to":{"type":"string"},"subject":{"type":"string"},"body":{"type":"string"}},"required":["to","subject","body"]})),
        f("recall_activity", "Look up what the user has been doing (their tracked app/window/clipboard activity = their 'second brain'). Use for 'what was I doing', 'what apps did I use', 'how long in X'. Optional query filters by app/keyword.",
          serde_json::json!({"type":"object","properties":{"query":{"type":"string","description":"optional app/keyword filter; empty = most recent activity"}},"required":[]})),

        // ── code-builder mode: write/build/test real software in an isolated workspace
        f("code_new_project", "Start a new software project in an isolated workspace (under ~/jarvis-projects/<name>). Optionally scaffolds a toolchain. Use this FIRST whenever asked to build code or software. Returns the project path and suggested build/test commands.",
          serde_json::json!({"type":"object","properties":{"name":{"type":"string","description":"project name, e.g. 'todo-cli'"},"language":{"type":"string","description":"rust | node | python | go | web | (empty for plain folder)"}},"required":["name"]})),
        f("code_write_file", "Write a source file INSIDE a project (path is relative to the project root, e.g. 'src/main.rs'). Creates parent folders. Use this for all code, not write_file.",
          serde_json::json!({"type":"object","properties":{"project":{"type":"string"},"path":{"type":"string","description":"path relative to the project root"},"content":{"type":"string"}},"required":["project","path","content"]})),
        f("code_read_file", "Read a source file from a project (path relative to the project root).",
          serde_json::json!({"type":"object","properties":{"project":{"type":"string"},"path":{"type":"string"}},"required":["project","path"]})),
        f("code_list", "Show a project's file tree (skips target/node_modules/.git).", str_prop("project", "project name")),
        f("code_exec", "Run a command with the project as the working directory. This is how you build, test, run, and use git: e.g. 'cargo build', 'cargo test', 'npm install', 'pytest', 'git init', 'git commit -m ...'. Returns exit code, stdout, and stderr so you can read failures and fix them.",
          serde_json::json!({"type":"object","properties":{"project":{"type":"string"},"command":{"type":"string","description":"the shell command to run inside the project"}},"required":["project","command"]})),

        // ── deeper autonomy: a durable task list that survives restarts
        f("task_add", "Add a step to your durable to-do list (it survives restarts). Before a multi-step job, plan it by adding one task per step. Returns the new task id.", str_prop("title", "the task/step")),
        f("task_list", "List your current tasks and their status (open/done). Use to see what's left, or to resume a job after a restart.",
          serde_json::json!({"type":"object","properties":{},"required":[]})),
        f("task_done", "Mark a task done by its id once you have actually finished it.",
          serde_json::json!({"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]})),
        f("task_cancel", "Drop a task by its id (no longer needed).",
          serde_json::json!({"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]})),
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
        "install_software" => install_software(args_json),
        "wait" => wait_tool(args_json).await,
        "paste_text" => paste_text(args_json),
        "type_text" => type_text(args_json),
        "press_keys" => press_keys(args_json),
        "mouse_click" => mouse_click(args_json),
        "see_screen" => see_screen(args_json).await,
        "click_on" => click_on(args_json).await,
        "operate_app" => operate_app(args_json).await,
        "browse_url" => browse_url(args_json).await,
        "browse_js" => browse_js(args_json).await,
        "fetch_url" => fetch_url(args_json).await,
        "news_search" => news_search(args_json).await,
        "web_search" => web_search(args_json).await,
        "extract_contacts" => extract_contacts(args_json).await,
        "lead_add" => lead_add_tool(args_json, mem).await,
        "lead_list" => lead_list_tool(mem).await,
        "lead_update" => lead_update_tool(args_json, mem).await,
        "email_compose" => email_compose(args_json),
        "code_new_project" => code_new_project(args_json),
        "code_write_file" => code_write_file(args_json),
        "code_read_file" => code_read_file(args_json),
        "code_list" => code_list(args_json),
        "code_exec" => code_exec(args_json),
        "task_add" => task_add_tool(args_json, mem).await,
        "task_list" => task_list_tool(mem).await,
        "task_done" => task_status_tool(args_json, mem, "done").await,
        "task_cancel" => task_status_tool(args_json, mem, "cancelled").await,
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

// ── code-builder mode (Power 1) ─────────────────────────────────────────────
// All of these route through crate::coder for workspace + path safety. They let
// Jarvis scaffold a project, write files into it, and build/test/run/git inside
// it — the agent loop reads code_exec failures and self-corrects.

#[derive(Deserialize)]
struct NewProjectArgs { name: String, #[serde(default)] language: String }
#[derive(Deserialize)]
struct ProjWriteArgs { project: String, path: String, content: String }
#[derive(Deserialize)]
struct ProjReadArgs { project: String, path: String }
#[derive(Deserialize)]
struct ProjArg { project: String }
#[derive(Deserialize)]
struct ProjExecArgs { project: String, command: String }

// PATH augmented with the usual toolchain bin dirs, so code_exec finds cargo,
// rustc, npm, etc. even from a non-interactive shell that didn't load the user
// profile PATH. Without this the model has to reconstruct PATH from the registry.
fn toolchain_path() -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        let h = home.to_string_lossy();
        for sub in [".cargo/bin", ".local/bin", "go/bin", ".dotnet/tools"] {
            parts.push(format!("{h}/{sub}").replace('/', std::path::MAIN_SEPARATOR_STR));
        }
    }
    if !cfg!(windows) {
        for p in ["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin"] {
            parts.push(p.to_string());
        }
    }
    if let Ok(existing) = std::env::var("PATH") {
        parts.push(existing);
    }
    parts.join(if cfg!(windows) { ";" } else { ":" })
}

// Run a shell command with `dir` as the working directory. Same shape as
// run_shell but cwd-scoped, PATH-augmented for toolchains, and with a larger
// output budget so build/test failures come back readable.
fn run_in(dir: &std::path::Path, command: &str) -> String {
    let path = toolchain_path();
    let output = if cfg!(windows) {
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", command])
            .current_dir(dir)
            .env("PATH", &path)
            .output()
    } else {
        std::process::Command::new("sh")
            .args(["-c", command])
            .current_dir(dir)
            .env("PATH", &path)
            .output()
    };
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut s = format!("exit={}\n", out.status);
            if !stdout.trim().is_empty() {
                s.push_str(&format!("stdout:\n{}\n", stdout.chars().take(8000).collect::<String>()));
            }
            if !stderr.trim().is_empty() {
                s.push_str(&format!("stderr:\n{}\n", stderr.chars().take(6000).collect::<String>()));
            }
            s
        }
        Err(e) => format!("ERROR running command: {e}"),
    }
}

fn code_new_project(args: &str) -> String {
    let a: NewProjectArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let dir = crate::coder::project_dir(&a.name);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return format!("ERROR creating project dir {}: {e}", dir.display());
    }
    let lang = a.language.trim().to_lowercase();
    let mut notes = String::new();

    // Toolchain scaffold (cargo init / npm init / go mod), if applicable.
    if let Some(cmd) = crate::coder::scaffold_command(&lang) {
        let out = run_in(&dir, cmd);
        notes.push_str(&format!("scaffold `{cmd}`:\n{out}\n"));
    } else {
        // Drop a starter file for languages without a scaffolder.
        match lang.as_str() {
            "python" => { let _ = std::fs::write(dir.join("main.py"), "def main():\n    print(\"hello\")\n\n\nif __name__ == \"__main__\":\n    main()\n"); let _ = std::fs::write(dir.join("requirements.txt"), ""); }
            "web" => { let _ = std::fs::write(dir.join("index.html"), "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>app</title></head>\n<body><h1>hello</h1></body></html>\n"); }
            _ => {}
        }
    }

    let detected = crate::coder::detect_language(&dir);
    let (build, test) = crate::coder::hints(detected);
    format!(
        "Project '{}' ready at {}\nlanguage: {}\nbuild with code_exec: {}\ntest with code_exec:  {}\n{}",
        crate::coder::slugify(&a.name), dir.display(), detected, build, test,
        if notes.is_empty() { String::new() } else { format!("\n{notes}") }
    )
}

fn code_write_file(args: &str) -> String {
    let a: ProjWriteArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let dir = crate::coder::project_dir(&a.project);
    if !dir.exists() {
        return format!("ERROR: project '{}' does not exist — call code_new_project first", a.project);
    }
    let path = match crate::coder::safe_join(&dir, &a.path) { Ok(p) => p, Err(e) => return e };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, a.content.as_bytes()) {
        Ok(()) => format!("wrote {} bytes to {}", a.content.len(), a.path),
        Err(e) => format!("ERROR writing {}: {e}", a.path),
    }
}

fn code_read_file(args: &str) -> String {
    let a: ProjReadArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let dir = crate::coder::project_dir(&a.project);
    let path = match crate::coder::safe_join(&dir, &a.path) { Ok(p) => p, Err(e) => return e };
    match std::fs::read_to_string(&path) {
        Ok(t) => t.chars().take(12000).collect(),
        Err(e) => format!("ERROR reading {}: {e}", a.path),
    }
}

fn code_list(args: &str) -> String {
    let a: ProjArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let dir = crate::coder::project_dir(&a.project);
    if !dir.exists() {
        return format!("ERROR: project '{}' does not exist", a.project);
    }
    format!("{} ({}):\n{}", a.project, dir.display(), crate::coder::tree(&dir))
}

fn code_exec(args: &str) -> String {
    let a: ProjExecArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let dir = crate::coder::project_dir(&a.project);
    if !dir.exists() {
        return format!("ERROR: project '{}' does not exist — call code_new_project first", a.project);
    }
    run_in(&dir, &a.command)
}

// ── durable task list (Power 4) ─────────────────────────────────────────────
#[derive(Deserialize)]
struct TitleArg { title: String }
#[derive(Deserialize)]
struct IdArg { id: i64 }

async fn task_add_tool(args: &str, mem: &crate::memory::MemoryHandle) -> String {
    let a: TitleArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let id = mem.task_add(&a.title).await;
    if id < 0 { return "ERROR: could not add task".to_string(); }
    format!("added task #{id}: {}", a.title)
}

async fn task_list_tool(mem: &crate::memory::MemoryHandle) -> String {
    let rows = mem.task_list().await;
    if rows.is_empty() {
        return "No tasks yet.".to_string();
    }
    let mut out = String::from("Tasks:\n");
    for (id, title, status) in rows {
        let mark = match status.as_str() { "done" => "x", _ => " " };
        out.push_str(&format!("  [{mark}] #{id} {title}\n"));
    }
    out
}

async fn task_status_tool(args: &str, mem: &crate::memory::MemoryHandle, status: &str) -> String {
    let a: IdArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    if mem.task_set_status(a.id, status).await {
        format!("task #{} marked {status}", a.id)
    } else {
        format!("ERROR: no task #{}", a.id)
    }
}

// ── app launch / wait / reliable paste ──────────────────────────────────────
#[derive(Deserialize)]
struct NameArg { name: String }
#[derive(Deserialize)]
struct SecondsArg { seconds: u64 }

fn open_app(args: &str) -> String {
    let a: NameArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    open_app_os(&a.name)
}

// Windows: search the Start Menu for a matching shortcut (handles installed GUI
// apps like the Codex app); else a CLI tool on PATH -> open it in a terminal;
// else let the OS try by name.
#[cfg(windows)]
fn open_app_os(name: &str) -> String {
    let name_lit = format!("'{}'", name.replace('\'', "''"));
    // 1) Start Menu shortcut -> launch it (GUI).
    // 2) else a command on PATH: if it's a real .exe, Start-Process it directly
    //    (notepad/calc/chrome/code are GUI apps and must NOT be wrapped in a
    //    terminal). Only actual scripts/shims (.ps1/.cmd/.bat, e.g. codex) open
    //    in a visible terminal so their output is seen.
    // 3) else hand the bare name to the OS.
    let script = r#"$n=__NAME__;
$sm=@("$env:ProgramData\Microsoft\Windows\Start Menu\Programs","$env:APPDATA\Microsoft\Windows\Start Menu\Programs");
$lnk=Get-ChildItem -Path $sm -Recurse -Filter *.lnk -ErrorAction SilentlyContinue | Where-Object { $_.BaseName -like "*$n*" } | Select-Object -First 1 -ExpandProperty FullName;
if($lnk){ Start-Process $lnk; "opened app: $lnk" }
else {
  $c = Get-Command $n -ErrorAction SilentlyContinue;
  if($c -and $c.Source -and ($c.Source -match '\.exe$')){ Start-Process $c.Source; "opened $($c.Source)" }
  elseif($c){ Start-Process powershell -ArgumentList '-NoExit','-Command',$n; "opened $n in a terminal" }
  else { Start-Process $n; "started $n" }
}"#
        .replace("__NAME__", &name_lit);
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() { format!("opened {name}") } else { s }
        }
        Ok(o) => format!("ERROR: couldn't open '{name}': {}", String::from_utf8_lossy(&o.stderr).trim().chars().take(200).collect::<String>()),
        Err(e) => format!("ERROR opening {name}: {e}"),
    }
}

// macOS: `open -a <App>` launches a GUI app by name. If that fails, treat the
// name as a CLI tool and run it in a new Terminal window via AppleScript.
#[cfg(target_os = "macos")]
fn open_app_os(name: &str) -> String {
    let gui = std::process::Command::new("open").args(["-a", name]).output();
    if let Ok(o) = &gui {
        if o.status.success() {
            return format!("opened {name}");
        }
    }
    // Fall back: run it as a command in a new Terminal window.
    let osa = format!("tell application \"Terminal\" to do script \"{}\"", name.replace('"', "\\\""));
    match std::process::Command::new("osascript").args(["-e", &osa]).output() {
        Ok(o) if o.status.success() => format!("opened {name} in Terminal"),
        _ => format!("ERROR: couldn't open '{name}' (tried open -a and Terminal)"),
    }
}

// Linux: prefer a desktop launcher via gtk-launch; else run the binary if it is
// on PATH; else hand it to xdg-open.
#[cfg(all(unix, not(target_os = "macos")))]
fn open_app_os(name: &str) -> String {
    if std::process::Command::new("gtk-launch").arg(name).spawn().is_ok() {
        return format!("opened {name}");
    }
    if std::process::Command::new(name).spawn().is_ok() {
        return format!("started {name}");
    }
    match std::process::Command::new("xdg-open").arg(name).spawn() {
        Ok(_) => format!("opened {name}"),
        Err(e) => format!("ERROR opening {name}: {e}"),
    }
}

// Acquire software via the OS package manager (Power: download + install).
// Part of the autonomous "acquire -> open -> operate" loop.
fn install_software(args: &str) -> String {
    let a: NameArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    install_software_os(&a.name)
}

#[cfg(windows)]
fn install_software_os(name: &str) -> String {
    let out = std::process::Command::new("winget")
        .args([
            "install", "--accept-package-agreements", "--accept-source-agreements",
            "--silent", "--disable-interactivity", name,
        ])
        .output();
    finish_install(name, out, "winget")
}

#[cfg(target_os = "macos")]
fn install_software_os(name: &str) -> String {
    let out = std::process::Command::new("brew").args(["install", name]).output();
    finish_install(name, out, "brew")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn install_software_os(name: &str) -> String {
    // apt needs root; try without sudo first, fall back to sudo -n (non-interactive).
    let cmd = format!("apt-get install -y {0} || sudo -n apt-get install -y {0}", name);
    let out = std::process::Command::new("sh").args(["-c", &cmd]).output();
    finish_install(name, out, "apt-get")
}

fn finish_install(name: &str, out: std::io::Result<std::process::Output>, mgr: &str) -> String {
    match out {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            if o.status.success() {
                format!("installed '{name}' via {mgr}.\n{}", stdout.chars().take(1200).collect::<String>())
            } else {
                format!(
                    "ERROR installing '{name}' via {mgr} (exit {}):\n{}\n{}",
                    o.status,
                    stdout.chars().take(800).collect::<String>(),
                    stderr.chars().take(800).collect::<String>()
                )
            }
        }
        Err(e) => format!("ERROR: could not run {mgr} for '{name}': {e}"),
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

// Press a key combo like "ctrl+s" / "enter" / "alt+tab". Shared by press_keys
// and the autonomous operate loop.
fn do_combo(combo: &str) -> String {
    let mut enigo = match new_enigo() { Ok(e) => e, Err(e) => return e };
    let parts: Vec<String> = combo.split('+').map(|s| s.trim().to_lowercase()).collect();
    let mods: Vec<Key> = parts.iter().filter(|p| is_modifier(p)).map(|p| map_key(p)).collect();
    let finals: Vec<Key> = parts.iter().filter(|p| !is_modifier(p)).map(|p| map_key(p)).collect();
    for m in &mods { let _ = enigo.key(*m, Direction::Press); }
    for k in &finals { let _ = enigo.key(*k, Direction::Click); }
    for m in mods.iter().rev() { let _ = enigo.key(*m, Direction::Release); }
    format!("pressed {combo}")
}

fn press_keys(args: &str) -> String {
    let a: ComboArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    do_combo(&a.combo)
}

// Move + left-click at an absolute pixel. Shared by mouse_click, click_on, operate.
fn do_click(x: i32, y: i32) -> String {
    let mut enigo = match new_enigo() { Ok(e) => e, Err(e) => return e };
    let _ = enigo.move_mouse(x, y, Coordinate::Abs);
    match enigo.button(Button::Left, Direction::Click) {
        Ok(()) => format!("clicked at {x},{y}"),
        Err(e) => format!("ERROR clicking: {e}"),
    }
}

fn mouse_click(args: &str) -> String {
    let a: ClickArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    do_click(a.x, a.y)
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

// ── autonomous operate loop: perceive -> act -> verify -> recover ────────────
// This is the computer-use core: given a goal, repeatedly screenshot, ask the
// vision model for the SINGLE next action, do it, and re-check, until the model
// says done/fail or we hit the step cap. Reuses do_click / do_paste / do_combo.
#[derive(Deserialize)]
struct GoalArg { goal: String }

#[derive(Deserialize)]
struct Action {
    action: String,
    #[serde(default)] x: Option<i64>,
    #[serde(default)] y: Option<i64>,
    #[serde(default)] text: Option<String>,
    #[serde(default)] combo: Option<String>,
    #[serde(default)] why: String,
}

// Pull the first {...} JSON object out of the model's reply and parse it.
fn parse_action(s: &str) -> Option<Action> {
    let start = s.find('{')?;
    let end = s[start..].find('}')? + start + 1;
    serde_json::from_str(&s[start..end]).ok()
}

async fn operate_app(args: &str) -> String {
    let a: GoalArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let max = std::env::var("JARVIS_OPERATE_STEPS")
        .ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or(8u32);

    let mut history: Vec<String> = Vec::new();
    for step in 1..=max {
        let (data_url, w, h) = match screenshot_data_url() {
            Ok(x) => x,
            Err(e) => return format!("operate stopped at step {step}: {e}\nactions: {}", history.join("; ")),
        };
        let hist = if history.is_empty() { "(none yet)".to_string() } else { history.join("; ") };
        let prompt = format!(
            "You are operating a desktop to accomplish a GOAL by choosing ONE action at a time.\n\
             The screenshot is {w}x{h} pixels, origin top-left.\n\
             GOAL: {}\n\
             ACTIONS SO FAR: {hist}\n\
             Reply with ONLY one JSON object, no prose, exactly one of:\n\
             {{\"action\":\"click\",\"x\":INT,\"y\":INT,\"why\":STR}}\n\
             {{\"action\":\"type\",\"text\":STR,\"why\":STR}}\n\
             {{\"action\":\"key\",\"combo\":STR,\"why\":STR}}  (combo like \"ctrl+s\" or \"enter\")\n\
             {{\"action\":\"done\",\"why\":STR}}  when the goal is achieved\n\
             {{\"action\":\"fail\",\"why\":STR}}  if you cannot proceed\n\
             For click, give the CENTER pixel of the target element.\n\
             IMPORTANT: act ONLY inside the app the goal is about. If that window is \
             not in front, your FIRST action should be to click it to focus it. NEVER \
             click a web browser's address bar or search box, and never type a search \
             query, unless the goal is explicitly about the browser - doing so can \
             navigate away and lose this session.",
            a.goal
        );
        let ans = vision_ask(&data_url, &prompt).await;
        if ans.starts_with("ERROR") {
            return format!("operate stopped: {ans}\nactions: {}", history.join("; "));
        }
        let act = match parse_action(&ans) {
            Some(act) => act,
            None => {
                history.push(format!("step {step}: unparseable ({})", ans.chars().take(50).collect::<String>()));
                continue;
            }
        };
        match act.action.as_str() {
            "done" => return format!("Done, sir. {}\nSteps: {}", act.why, history.join("; ")),
            "fail" => return format!("I got stuck: {}\nSteps: {}", act.why, history.join("; ")),
            "click" => match (act.x, act.y) {
                (Some(x), Some(y)) => { do_click(x as i32, y as i32); history.push(format!("clicked {x},{y} ({})", act.why)); }
                _ => history.push("click had no coordinates".into()),
            },
            "type" => {
                let t = act.text.unwrap_or_default();
                do_paste(&t);
                history.push(format!("typed \"{}\" ({})", t.chars().take(40).collect::<String>(), act.why));
            }
            "key" => {
                let c = act.combo.unwrap_or_default();
                do_combo(&c);
                history.push(format!("key {c} ({})", act.why));
            }
            other => history.push(format!("unknown action '{other}'")),
        }
        // Let the UI settle before the next screenshot.
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    }
    format!("Reached the {max}-step operate limit before finishing.\nSteps: {}", history.join("; "))
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

// ── general web search (DuckDuckGo HTML, no API key) ────────────────────────
// The foundation capability for "go find X": leads, jobs, companies, facts.
async fn web_search(args: &str) -> String {
    let a: SearchArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let client = match reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36")
        .build()
    {
        Ok(c) => c,
        Err(e) => return format!("ERROR: http client: {e}"),
    };
    let resp = client
        .get("https://html.duckduckgo.com/html/")
        .query(&[("q", a.query.as_str())])
        .send()
        .await;
    let html = match resp {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => return format!("ERROR searching for '{}': {e}", a.query),
    };
    let titles = extract_blocks(&html, "result__a", "</a>");
    let urls = extract_blocks(&html, "result__url", "</a>");
    let snips = extract_blocks(&html, "result__snippet", "</a>");
    if titles.is_empty() || urls.is_empty() {
        return format!("No web results for '{}' (the search may have been rate-limited).", a.query);
    }
    let n = titles.len().min(urls.len());
    let mut out = format!("Top web results for \"{}\":\n", a.query);
    for i in 0..n.min(8) {
        let url = urls[i].trim();
        let url = if url.starts_with("http") { url.to_string() } else { format!("https://{url}") };
        let snip = snips.get(i).map(|s| s.as_str()).unwrap_or("");
        out.push_str(&format!("{}. {}\n   {}\n   {}\n", i + 1, titles[i], url, snip));
    }
    out
}

// Pull the inner text of every element with the given class, up to `end`.
fn extract_blocks(html: &str, class: &str, end: &str) -> Vec<String> {
    let needle = format!("class=\"{class}\"");
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(i) = html[pos..].find(&needle) {
        let start = pos + i;
        if let Some(gt) = html[start..].find('>') {
            let content_start = start + gt + 1;
            if let Some(e) = html[content_start..].find(end) {
                out.push(strip_tags(&html[content_start..content_start + e]));
                pos = content_start + e;
                continue;
            }
        }
        pos = start + needle.len();
    }
    out
}

// Strip HTML tags and decode the few entities DuckDuckGo emits.
fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim()
        .replace("&amp;", "&")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

// ── research + outreach engine ──────────────────────────────────────────────
#[derive(Deserialize)]
struct LeadArgs {
    name: String,
    #[serde(default)] org: String,
    #[serde(default)] email: String,
    #[serde(default)] phone: String,
    #[serde(default)] url: String,
    #[serde(default)] note: String,
}
#[derive(Deserialize)]
struct LeadUpdateArgs { id: i64, status: String }
#[derive(Deserialize)]
struct EmailArgs { to: String, subject: String, body: String }

// Fetch a page and pull out emails + phone numbers.
async fn extract_contacts(args: &str) -> String {
    let a: UrlArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let client = match reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36")
        .build()
    { Ok(c) => c, Err(e) => return format!("ERROR: http client: {e}") };
    let html = match client.get(&a.url).send().await {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => return format!("ERROR fetching {}: {e}", a.url),
    };
    let emails = find_emails(&html);
    let phones = find_phones(&html);
    if emails.is_empty() && phones.is_empty() {
        return format!("No emails or phone numbers found on {}", a.url);
    }
    let mut out = format!("Contacts on {}:\n", a.url);
    if !emails.is_empty() { out.push_str(&format!("emails: {}\n", emails.join(", "))); }
    if !phones.is_empty() { out.push_str(&format!("phones: {}\n", phones.join(", "))); }
    out
}

// Find email addresses by expanding around each '@' over allowed ASCII chars.
fn find_emails(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let local_ok = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'%' | b'+' | b'-');
    let dom_ok = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-');
    let mut set = std::collections::BTreeSet::new();
    for (i, &c) in bytes.iter().enumerate() {
        if c != b'@' { continue; }
        let mut l = i;
        while l > 0 && local_ok(bytes[l - 1]) { l -= 1; }
        let mut r = i + 1;
        while r < bytes.len() && dom_ok(bytes[r]) { r += 1; }
        if l < i && r > i + 1 {
            let cand = text[l..r].trim_end_matches('.').to_lowercase();
            if let Some(at) = cand.find('@') {
                let dom = &cand[at + 1..];
                if dom.contains('.') && !dom.starts_with('.') && !dom.ends_with('.') {
                    set.insert(cand);
                }
            }
        }
    }
    set.into_iter().take(25).collect()
}

// Find phone-like runs: 10-15 digits with the usual separators.
fn find_phones(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let is_phone_char = |c: char| c.is_ascii_digit() || matches!(c, '+' | '-' | ' ' | '(' | ')' | '.');
    let mut set = std::collections::BTreeSet::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() || chars[i] == '+' {
            let start = i;
            while i < chars.len() && is_phone_char(chars[i]) { i += 1; }
            let run: String = chars[start..i].iter().collect();
            let digits = run.chars().filter(|c| c.is_ascii_digit()).count();
            if (10..=15).contains(&digits) {
                set.insert(run.trim().to_string());
            }
        } else {
            i += 1;
        }
    }
    set.into_iter().take(15).collect()
}

async fn lead_add_tool(args: &str, mem: &crate::memory::MemoryHandle) -> String {
    let a: LeadArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let lead = crate::memory::Lead {
        name: a.name.clone(), org: a.org, email: a.email.clone(), phone: a.phone.clone(),
        url: a.url, note: a.note, status: String::new(),
    };
    let id = mem.lead_add(lead).await;
    if id < 0 { return "ERROR: could not save lead".to_string(); }
    let mut tail = String::new();
    if !a.email.is_empty() { tail.push_str(&format!(" {}", a.email)); }
    if !a.phone.is_empty() { tail.push_str(&format!(" {}", a.phone)); }
    format!("saved lead #{id}: {}{tail}", a.name)
}

async fn lead_list_tool(mem: &crate::memory::MemoryHandle) -> String {
    let rows = mem.lead_list().await;
    if rows.is_empty() { return "No leads yet.".to_string(); }
    let mut out = format!("Leads ({}):\n", rows.len());
    for (id, l) in rows {
        let mut parts = vec![format!("#{id} [{}] {}", l.status, l.name)];
        if !l.org.is_empty() { parts.push(l.org); }
        if !l.email.is_empty() { parts.push(l.email); }
        if !l.phone.is_empty() { parts.push(l.phone); }
        if !l.url.is_empty() { parts.push(l.url); }
        out.push_str(&format!("  {}\n", parts.join(" | ")));
    }
    out
}

async fn lead_update_tool(args: &str, mem: &crate::memory::MemoryHandle) -> String {
    let a: LeadUpdateArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    if mem.lead_set_status(a.id, &a.status).await {
        format!("lead #{} marked {}", a.id, a.status)
    } else {
        format!("ERROR: no lead #{}", a.id)
    }
}

// Open a prefilled Gmail compose window in the user's default browser. They are
// already logged in, so they review and click Send - no credentials, and a human
// check before any outbound email (it is not auto-sent).
fn email_compose(args: &str) -> String {
    let a: EmailArgs = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    let url = format!(
        "https://mail.google.com/mail/?view=cm&fs=1&to={}&su={}&body={}",
        percent_encode(&a.to), percent_encode(&a.subject), percent_encode(&a.body)
    );
    match open_url_default(&url) {
        Ok(()) => format!("opened a Gmail draft to {} (review it and hit Send).", a.to),
        Err(e) => e,
    }
}

// Percent-encode a string for use in a URL query value.
fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// Open a URL in the OS default browser.
fn open_url_default(url: &str) -> Result<(), String> {
    let r = if cfg!(windows) {
        let ps = format!("Start-Process '{}'", url.replace('\'', "''"));
        std::process::Command::new("powershell").args(["-NoProfile", "-Command", &ps]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    r.map(|_| ()).map_err(|e| format!("ERROR opening browser: {e}"))
}

async fn fetch_url(args: &str) -> String {
    let a: UrlArg = match serde_json::from_str(args) { Ok(a) => a, Err(e) => return format!("ERROR: bad args: {e}") };
    // Retry-with-backoff for transient network failures (Power 4 recovery).
    let mut last_err = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(300 * (1 << (attempt - 1)))).await;
        }
        match reqwest::get(&a.url).await {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return format!("HTTP {status}\n{}", body.chars().take(2000).collect::<String>());
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    format!("ERROR fetching {} after 3 tries: {last_err}", a.url)
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
