// ── src/fswatch.rs : deep OS integration, rung 1 - filesystem awareness ────────
//
// Instead of screenshotting Explorer and reading it like a human, Jarvis consumes
// real OS filesystem EVENTS (ReadDirectoryChangesW on Windows, inotify on Linux,
// FSEvents on macOS - via the `notify` crate) and records meaningful file changes
// into the second-brain activity log. So "what did I change in the last hour?"
// gets a real answer, and the OS layer is genuinely integrated - an event consumer
// at the OS level, not a puppeteer clicking around Explorer.
//
// Off with JARVIS_FSWATCH=off. Folders via JARVIS_FSWATCH_DIRS (semicolon list),
// else Desktop + Downloads + Documents + the current working directory.

use crate::memory::MemoryHandle;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn spawn(mem: MemoryHandle) {
    if std::env::var("JARVIS_FSWATCH").unwrap_or_default() == "off" {
        eprintln!("[fswatch] filesystem awareness disabled (JARVIS_FSWATCH=off)");
        return;
    }
    let dirs = watch_dirs();
    if dirs.is_empty() {
        eprintln!("[fswatch] no folders to watch");
        return;
    }
    let handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel::<notify::Result<Event>>();
        let mut watcher = match notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[fswatch] cannot start watcher: {e}");
                return;
            }
        };
        let mut watched = 0;
        for d in &dirs {
            if watcher.watch(d.as_path(), RecursiveMode::Recursive).is_ok() {
                watched += 1;
            }
        }
        eprintln!("[fswatch] watching {watched} folder(s) for file changes");

        // Editors fire many events per save; dedupe the same (verb, path) within a
        // few seconds so the log stays meaningful, not a firehose.
        let mut recent: HashMap<String, u64> = HashMap::new();
        for res in rx {
            // blocks here; `watcher` stays alive in scope for the thread's life
            let event = match res {
                Ok(e) => e,
                Err(_) => continue,
            };
            let verb = match event.kind {
                EventKind::Create(_) => "created",
                EventKind::Remove(_) => "deleted",
                EventKind::Modify(_) => "modified",
                _ => continue, // ignore access/other
            };
            let now = unix_now();
            for path in event.paths {
                let p = path.to_string_lossy().to_string();
                if is_noise(&p) {
                    continue;
                }
                let key = format!("{verb}|{p}");
                if let Some(t) = recent.get(&key) {
                    if now.saturating_sub(*t) < 3 {
                        continue;
                    }
                }
                recent.insert(key, now);
                if recent.len() > 512 {
                    recent.retain(|_, t| now.saturating_sub(*t) < 30);
                }
                handle.block_on(mem.log_activity("file", verb, &p));
            }
        }
        drop(watcher);
    });
}

fn watch_dirs() -> Vec<PathBuf> {
    if let Ok(list) = std::env::var("JARVIS_FSWATCH_DIRS") {
        return list
            .split(';')
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .collect();
    }
    let mut v = Vec::new();
    if let Some(d) = dirs::desktop_dir() {
        v.push(d);
    }
    if let Some(d) = dirs::download_dir() {
        v.push(d);
    }
    if let Some(d) = dirs::document_dir() {
        v.push(d);
    }
    if let Ok(cwd) = std::env::current_dir() {
        v.push(cwd);
    }
    v.into_iter().filter(|p| p.exists()).collect()
}

// Skip churny/system/temp paths so the log records user-meaningful file changes.
fn is_noise(p: &str) -> bool {
    let l = p.to_lowercase();
    const SKIP: &[&str] = &[
        "\\appdata\\", "/appdata/", "\\.git\\", "/.git/", "node_modules", "\\target\\",
        "/target/", "$recycle.bin", "\\.cargo\\", "/.cargo/", "thumbs.db", ".ds_store",
        "~$", ".tmp", ".temp", ".crdownload", ".partial", ".swp", ".swx", ".lock",
        "jarvis.db", ".jarvis-key", "\\workspace\\", "/workspace/",
    ];
    SKIP.iter().any(|s| l.contains(s))
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
