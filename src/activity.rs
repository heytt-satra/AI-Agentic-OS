// ── src/activity.rs : the "second brain" — always-on activity tracking ──────
//
// Background watchers that record what you're doing into the `activity` table,
// so Jarvis remembers your day and can answer "what was I doing at 3pm?".
//
// Watchers (all local, no keylogging):
//   - foreground window: which app + window title is focused (every few sec)
//   - clipboard: text you copy (deduped)
//   - screenshots: optional, only if SCREENSHOT_INTERVAL_SECS > 0
//
// Toggle off entirely with JARVIS_TRACKING=off.

use crate::memory::MemoryHandle;
use std::time::Duration;

pub fn spawn(mem: MemoryHandle) {
    if std::env::var("JARVIS_TRACKING").unwrap_or_default() == "off" {
        eprintln!("[activity] tracking disabled (JARVIS_TRACKING=off)");
        return;
    }
    let win_secs: u64 = env_u64("ACTIVITY_INTERVAL_SECS", 5);
    let shot_secs: u64 = env_u64("SCREENSHOT_INTERVAL_SECS", 0); // 0 = off by default

    // 1) foreground window watcher
    {
        let mem = mem.clone();
        tokio::spawn(async move {
            let mut last = String::new();
            loop {
                tokio::time::sleep(Duration::from_secs(win_secs)).await;
                if let Ok(w) = active_win_pos_rs::get_active_window() {
                    let sig = format!("{}|{}", w.app_name, w.title);
                    if !sig.trim().is_empty() && sig != last {
                        last = sig;
                        mem.log_activity("window", &w.app_name, &w.title).await;
                    }
                }
            }
        });
    }

    // 2) clipboard watcher
    {
        let mem = mem.clone();
        tokio::spawn(async move {
            let mut last = String::new();
            loop {
                tokio::time::sleep(Duration::from_secs(win_secs.max(8))).await;
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if let Ok(text) = cb.get_text() {
                        let text = text.trim().to_string();
                        if !text.is_empty() && text != last {
                            last = text.clone();
                            let snippet: String = text.chars().take(2000).collect();
                            mem.log_activity("clipboard", "", &snippet).await;
                        }
                    }
                }
            }
        });
    }

    // 3) optional periodic screenshots (off unless SCREENSHOT_INTERVAL_SECS set)
    if shot_secs > 0 {
        let mem = mem.clone();
        tokio::spawn(async move {
            let _ = std::fs::create_dir_all("memory/screenshots");
            loop {
                tokio::time::sleep(Duration::from_secs(shot_secs)).await;
                // capture on a blocking thread (xcap types are !Send)
                let path = tokio::task::spawn_blocking(save_screenshot).await;
                if let Ok(Ok(p)) = path {
                    mem.log_activity("screenshot", "", &p).await;
                }
            }
        });
        eprintln!("[activity] screenshots every {shot_secs}s -> memory/screenshots/");
    }

    eprintln!("[activity] second-brain tracking on (window + clipboard every {win_secs}s)");
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn save_screenshot() -> Result<String, String> {
    let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors.into_iter().next().ok_or("no monitor")?;
    let img = monitor.capture_image().map_err(|e| e.to_string())?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = format!("memory/screenshots/{ts}.png");
    xcap::image::DynamicImage::ImageRgba8(img)
        .save(&path)
        .map_err(|e| e.to_string())?;
    Ok(path)
}
