// ── src/policy.rs : the safety gate every action passes through ──────────────
//
// Before Jarvis runs ANY tool, we ask the policy: is this safe to do
// automatically, or must the human approve it? This is what makes "full device
// control" not be "a loaded gun" — the LLM proposes, the policy + human dispose.
//
// Tiered model (the posture you chose):
//   - read-only / reversible  -> AUTO (read_file, list_dir, news_search, ...)
//   - destructive / external / system-changing -> ASK (run_shell, write_file,
//     delete, open apps, ...)
// Approvals can be remembered ("always allow this exact action"), EXCEPT when
// the current turn has touched untrusted web content (injection defense).

use serde_json::Value;

pub struct Risk {
    pub needs_approval: bool,
    pub label: String, // human-readable "what will happen"
    pub key: String,   // stable id for remembered allow/deny rules
}

pub fn assess(tool: &str, args_json: &str) -> Risk {
    let v: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();

    // Pull the salient argument so remembered rules are SPECIFIC (e.g. allow
    // exactly `git status`, not "all shell commands").
    let (needs_approval, salient, label) = match tool {
        // --- auto: read-only / safe ---
        "read_file" => (false, field("path"), format!("read file {}", field("path"))),
        "list_dir" => (false, field("path"), format!("list {}", field("path"))),
        "news_search" => (false, field("query"), format!("search news '{}'", field("query"))),
        "fetch_url" => (false, field("url"), format!("fetch {}", field("url"))),
        "browse_url" => (false, field("url"), format!("browse {}", field("url"))),
        "get_current_time" => (false, String::new(), "check the time".into()),
        "wait" => (false, String::new(), "wait".into()),

        // --- ask: changes the system / world ---
        "run_shell" => (true, field("command"), format!("run shell: {}", field("command"))),
        "write_file" => (true, field("path"), format!("write file {}", field("path"))),
        "delete_path" => (true, field("path"), format!("DELETE {}", field("path"))),
        "open_path" => (true, field("target"), format!("open {}", field("target"))),
        "open_app" => (true, field("name"), format!("launch app: {}", field("name"))),
        "paste_text" => (true, field("text"), format!("paste: {}", field("text").chars().take(40).collect::<String>())),
        "type_text" => (true, field("text"), format!("type: {}", field("text").chars().take(40).collect::<String>())),
        "press_keys" => (true, field("combo"), format!("press keys: {}", field("combo"))),
        "mouse_click" => {
            let x = v.get("x").and_then(|x| x.as_i64()).unwrap_or(0);
            let y = v.get("y").and_then(|x| x.as_i64()).unwrap_or(0);
            (true, format!("{x},{y}"), format!("click at {x},{y}"))
        }
        "see_screen" => (true, String::new(), "screenshot your screen and send it to a vision model".into()),
        "browse_js" => (true, field("url"), format!("run JavaScript on {}", field("url"))),

        // unknown tools default to ASK (safe default)
        other => (true, String::new(), format!("run unknown tool '{other}'")),
    };

    Risk {
        needs_approval,
        label,
        key: format!("{tool}:{salient}"),
    }
}
