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
        // recoverable (goes to the Recycle Bin) but still removes the file
        "recycle_path" => (true, field("path"), format!("move to Recycle Bin: {}", field("path"))),

        // force-quitting a process ends it immediately (unsaved work lost)
        "kill_process" => {
            let who = if !field("name").is_empty() {
                field("name")
            } else {
                match v.get("pid").and_then(|x| x.as_u64()) {
                    Some(pid) => format!("PID {pid}"),
                    None => "a process".to_string(),
                }
            };
            (true, who.clone(), format!("KILL process {who}"))
        }

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

    // Financial override (roadmap 4.3): a distinct "spend" category. Anything that
    // could MOVE MONEY always gets a nod, even in autonomous mode, and is labeled
    // as a spend so the user knows exactly why. Money-safe by design: a false
    // positive just costs one prompt. Scoped to the money-capable tools so normal
    // reads/clicks aren't inspected.
    let money = matches!(tool, "run_shell" | "code_exec" | "browse_url" | "browse_js" | "open_path" | "fetch_url")
        && is_financial(args_json);
    if money {
        let what = if salient.is_empty() { tool.to_string() } else { salient.chars().take(80).collect() };
        return Risk {
            needs_approval: true,
            label: format!("SPEND (financial action): {what}"),
            key: format!("{tool}:spend:{salient}"),
        };
    }

    Risk { needs_approval, label, key: format!("{tool}:{salient}") }
}

// Intent that would move money. Deliberately narrow (strong transaction signals
// only) to avoid prompt fatigue on ordinary browsing, but broad enough to catch
// a checkout/payment/transfer. Matched case-insensitively over the raw args.
fn is_financial(args_json: &str) -> bool {
    let c = args_json.to_lowercase();
    const MONEY: &[&str] = &[
        "checkout", "place order", "confirm order", "confirm payment", "complete purchase",
        "buy now", "pay now", "pay $", "payment", "paypal", "stripe", "credit card",
        "card number", "cvv", "wire transfer", "bank transfer", "venmo", "zelle",
        "send money", "transfer money", "send bitcoin", "send eth",
    ];
    MONEY.iter().any(|m| c.contains(m))
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

#[cfg(test)]
mod tests {
    use super::assess;

    #[test]
    fn spend_actions_always_need_approval_and_are_labeled() {
        // a browse action that reaches a checkout -> spend prompt
        let r = assess("browse_js", r#"{"script":"click the Place order button and confirm payment"}"#);
        assert!(r.needs_approval);
        assert!(r.label.starts_with("SPEND"), "label was: {}", r.label);
        assert!(r.key.contains(":spend:"));

        // a shell command that wires money -> spend prompt even though it isn't a
        // system-dangerous command
        let r = assess("run_shell", r#"{"command":"paypal-cli send-money --to x --amount 50"}"#);
        assert!(r.needs_approval);
        assert!(r.label.starts_with("SPEND"));
    }

    #[test]
    fn recycle_needs_approval_but_reads_as_recoverable() {
        let r = assess("recycle_path", r#"{"path":"desktop/old.txt"}"#);
        assert!(r.needs_approval);
        assert!(r.label.contains("Recycle Bin") && !r.label.contains("DELETE"));
    }

    #[test]
    fn killing_a_process_needs_approval_and_names_the_target() {
        let r = assess("kill_process", r#"{"name":"spotify"}"#);
        assert!(r.needs_approval);
        assert!(r.label.contains("KILL") && r.label.contains("spotify"));
        // pid is an integer - the label must still name it, not print "PID "
        let r = assess("kill_process", r#"{"pid":4321}"#);
        assert!(r.needs_approval);
        assert!(r.label.contains("4321"), "label was: {}", r.label);
    }

    #[test]
    fn ordinary_actions_are_not_flagged_as_spend() {
        // reading a page about pricing is not a transaction
        let r = assess("browse_url", r#"{"url":"https://example.com/features"}"#);
        assert!(!r.needs_approval);
        // a plain read tool is never inspected for money
        let r = assess("read_file", r#"{"path":"notes about payment.txt"}"#);
        assert!(!r.needs_approval);
    }
}
