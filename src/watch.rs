// ── src/watch.rs : live "watch-along" — Jarvis sees (and later hears) a video ──
//
// When watch mode is ON, a background loop samples the screen every few seconds
// and captions each frame with the vision model. (Stage 2 adds an audio loop
// that transcribes whatever is playing through the speakers.) Both streams land
// in one rolling, timestamped buffer that is injected into the agent's context
// every turn — so while a video plays the user can just ask "what did he say
// about X" or "help me with this step" and Jarvis already has the context.
//
// State is a single process-global buffer so the background loop, the agent's
// context builder, and the watch_* tools all see the same thing without
// threading a new handle through every signature.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_NOTES: usize = 300; // hard cap on buffered observations (bounded RAM)
const KEEP_SECS: u64 = 15 * 60; // only the last 15 min is surfaced into context

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    See,
    Hear,
}

#[derive(Clone)]
struct Note {
    ts: u64,
    kind: Kind,
    text: String,
}

struct WatchState {
    active: bool,
    started: u64,
    notes: VecDeque<Note>,
}

impl WatchState {
    fn new() -> Self {
        Self { active: false, started: 0, notes: VecDeque::new() }
    }
}

fn cell() -> &'static Mutex<WatchState> {
    static S: OnceLock<Mutex<WatchState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(WatchState::new()))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).filter(|n| *n > 0).unwrap_or(default)
}

/// Is Jarvis currently watching the screen?
pub fn is_active() -> bool {
    cell().lock().map(|s| s.active).unwrap_or(false)
}

fn push(kind: Kind, text: String) {
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    if let Ok(mut s) = cell().lock() {
        if !s.active {
            return; // a late frame after stop(); drop it
        }
        s.notes.push_back(Note { ts: now(), kind, text });
        while s.notes.len() > MAX_NOTES {
            s.notes.pop_front();
        }
    }
}

/// Record one captioned video frame (the "eyes").
pub fn push_see(caption: String) {
    push(Kind::See, caption);
}

/// Record one chunk of transcribed audio (the "ears" — Stage 2).
#[allow(dead_code)]
pub fn push_hear(words: String) {
    push(Kind::Hear, words);
}

/// Begin watching: clear the buffer and spawn the visual sampling loop.
/// Safe to call when already active (no-op). Returns a short status line.
pub fn start() -> String {
    {
        let mut s = cell().lock().unwrap();
        if s.active {
            return "Already watching your screen. Play the video and ask me anything.".into();
        }
        s.active = true;
        s.started = now();
        s.notes.clear();
    }
    spawn_visual_loop();
    let secs = env_u64("WATCH_INTERVAL_SECS", 6);
    format!(
        "Watching your screen now (a glance every {secs}s). Play the video, then ask me \
         anything about it — what was said, what's shown, or help with a step."
    )
}

/// Stop watching. The background loop notices `active=false` and exits.
pub fn stop() -> String {
    let mut s = cell().lock().unwrap();
    if !s.active {
        return "I wasn't watching.".into();
    }
    s.active = false;
    let n = s.notes.len();
    format!("Stopped watching. I had {n} observations buffered from this session.")
}

/// One-line status for the watch_status tool.
pub fn status() -> String {
    let s = match cell().lock() {
        Ok(s) => s,
        Err(_) => return "watch state unavailable".into(),
    };
    if !s.active {
        return "Not watching. Say 'watch my screen' to start.".into();
    }
    let secs = now().saturating_sub(s.started);
    let sees = s.notes.iter().filter(|n| n.kind == Kind::See).count();
    let hears = s.notes.iter().filter(|n| n.kind == Kind::Hear).count();
    format!(
        "Watching for {}m{:02}s — {sees} things seen, {hears} things heard.",
        secs / 60,
        secs % 60
    )
}

/// The fused, timestamped log of what Jarvis is currently seeing and hearing,
/// formatted for injection into the agent's context. Empty when not watching.
pub fn context_snapshot() -> String {
    let s = match cell().lock() {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    if !s.active {
        return String::new();
    }
    let cutoff = now().saturating_sub(KEEP_SECS);
    let mut lines = Vec::new();
    for n in s.notes.iter().filter(|n| n.ts >= cutoff) {
        let rel = n.ts.saturating_sub(s.started);
        let tag = match n.kind {
            Kind::See => "SEE",
            Kind::Hear => "HEAR",
        };
        lines.push(format!("[{:02}:{:02} {tag}] {}", rel / 60, rel % 60, n.text));
    }
    if lines.is_empty() {
        return "LIVE WATCH: you just started watching the user's screen; nothing captured yet \
                (give it a few seconds for the first frame)."
            .into();
    }
    format!(
        "LIVE WATCH CONTEXT — this is what you are CURRENTLY seeing (SEE) and hearing (HEAR) on \
         the user's screen as a video plays, oldest first, newest last. Timestamps are mm:ss \
         since watching began. Answer the user's questions about the video from THIS log. If \
         they ask about something not in the log yet, say it hasn't appeared on screen yet \
         rather than guessing.\n{}",
        lines.join("\n")
    )
}

// The visual sampling loop: every WATCH_INTERVAL_SECS, screenshot the screen and
// caption it with the vision model, until watch mode is turned off. Capture is
// done on a blocking thread because xcap's types are !Send.
fn spawn_visual_loop() {
    tokio::spawn(async move {
        let secs = env_u64("WATCH_INTERVAL_SECS", 6);
        loop {
            if !is_active() {
                break;
            }
            let shot = tokio::task::spawn_blocking(crate::tools::screenshot_data_url).await;
            if let Ok(Ok((data_url, _w, _h))) = shot {
                let caption = crate::tools::vision_ask(
                    &data_url,
                    "You are watching a video on the user's screen in real time. In ONE or TWO \
                     short sentences describe what is happening RIGHT NOW: the scene/action, and \
                     read out any important on-screen text, titles, captions, code, or slides \
                     verbatim. Be concrete and brief — this line is stitched into a running log. \
                     If the screen is not a video (just a desktop or static page), say so in a \
                     few words.",
                )
                .await;
                if !caption.starts_with("ERROR") {
                    push_see(caption);
                } else {
                    eprintln!("[watch] vision error: {caption}");
                }
            }
            tokio::time::sleep(Duration::from_secs(secs)).await;
        }
        eprintln!("[watch] visual loop stopped");
    });
}
