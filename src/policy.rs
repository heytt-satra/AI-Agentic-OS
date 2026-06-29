// ── src/policy.rs : the safety gate ─────────────────────────────────────────
//
// Posture (per the owner): Jarvis is an agentic OS layer that should just DO
// things. We ONLY ask for approval when an action could damage the underlying
// operating system or destroy data irreversibly. Everything else runs silently.
//
// ASK only for:
//   - delete_path                    (irreversible)
//   - run_shell with a DANGEROUS command (rm/format/registry/shutdown/...)
//   - write_file / open_path into a SYSTEM location (Windows dir, Program Files)
// Everything else (read, list, write in user space, open apps, type, click,
// see screen, browse) runs automatically.

use serde_json::Value;

pub struct Risk {
    pub needs_approval: bool,
    pub label: String,
    pub key: String,
}

pub fn assess(tool: &str, args_json: &str) -> Risk {
    let v: Value = serde_json::from_str(args_json).unwrap_or(Value::Null);
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();

    let (needs_approval, salient, label) = match tool {
        // irreversible
        "delete_path" => (true, field("path"), format!("DELETE {}", field("path"))),

        // shell: only the dangerous ones
        "run_shell" => {
            let cmd = field("command");
            (is_dangerous_shell(&cmd), cmd.clone(), format!("run shell: {cmd}"))
        }

        // writing/opening only flagged if it targets the OS itself
        "write_file" => {
            let p = field("path");
            let sys = is_system_path(&p);
            let label = if sys { format!("write SYSTEM file {p}") } else { format!("write {p}") };
            (sys, p.clone(), label)
        }
        "open_path" => {
            let t = field("target");
            (is_system_path(&t), t.clone(), format!("open {t}"))
        }

        // code-builder runs in an isolated workspace, so it is autonomous —
        // except a destructive command inside code_exec still gets a prompt.
        "code_exec" => {
            let cmd = field("command");
            (is_dangerous_shell(&cmd), cmd.clone(), format!("code: {cmd}"))
        }

        // self-healing skills run a stored shell command we can't inspect here,
        // so they ALWAYS need approval - unless a capability token grants them.
        "skill_run" => {
            let name = field("name");
            (true, name.clone(), format!("run skill '{name}'"))
        }

        // everything else just runs
        _ => (false, String::new(), String::new()),
    };

    Risk { needs_approval, label, key: format!("{tool}:{salient}") }
}

// Commands that can wreck the system or destroy data. Matched case-insensitively
// as substrings — deliberately broad; false positives just cause one prompt.
fn is_dangerous_shell(cmd: &str) -> bool {
    let c = cmd.to_lowercase();
    const BAD: &[&str] = &[
        "rm -rf", "rm -r", "remove-item", "del ", "erase ", "rmdir", "rd /s", "rd ",
        "format ", "format-volume", "diskpart", "mkfs", "dd if=", "dd of=",
        "reg add", "reg delete", "reg ", "set-itemproperty hklm", "new-item hklm",
        "shutdown", "restart-computer", "stop-computer", "bcdedit", "takeown",
        "icacls", "cipher /w", "fsutil", "net user", "net localgroup", "schtasks /create",
        "\\windows\\system32", "c:\\windows", "/system/", "sudo rm", "sc delete", "sc config",
    ];
    BAD.iter().any(|b| c.contains(b))
}

// Paths inside the OS itself (not the user's own files).
fn is_system_path(p: &str) -> bool {
    let c = p.to_lowercase();
    c.contains("\\windows") || c.contains("/windows")
        || c.contains("program files")
        || c.contains("system32")
        || c.contains("hklm") || c.contains("hkey_")
        || c.starts_with("/etc") || c.starts_with("/usr") || c.starts_with("/bin") || c.starts_with("/sys")
}
