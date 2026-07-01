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
    // Optional window to watch (title/app substring). None = whole screen.
    target: Option<String>,
    notes: VecDeque<Note>,
}

impl WatchState {
    fn new() -> Self {
        Self { active: false, started: 0, target: None, notes: VecDeque::new() }
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
pub fn start(target: Option<String>) -> String {
    let target = target.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    {
        let mut s = cell().lock().unwrap();
        if s.active {
            return "Already watching. Play the video and ask me anything about it.".into();
        }
        s.active = true;
        s.started = now();
        s.target = target.clone();
        s.notes.clear();
    }
    spawn_visual_loop(target.clone());
    // The ears (Windows): capture + transcribe system audio into the same buffer.
    // Audio is system-wide (no focus needed), so it works regardless of target.
    // No-ops with a hint if no transcription key is set.
    #[cfg(windows)]
    crate::hearing::spawn_audio_loop();
    let secs = env_u64("WATCH_INTERVAL_SECS", 3);
    match &target {
        Some(t) => format!(
            "Watching the '{t}' window now - I can see it even while you keep this HUD in front \
             (checking every {secs}s, captioning on change), and I hear its audio. Play the video \
             and ask me anything about it."
        ),
        None => format!(
            "Watching now - I'll auto-detect whichever window is playing the video (by its audio + \
             title, across any browser) and watch that, even while you keep this HUD in front. \
             Checking every {secs}s, captioning on change, and I hear the audio too. Just play the \
             video and ask me about it. (If I pick the wrong window, name it: \"watch the vlc window\".)"
        ),
    }
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
    let tgt = match &s.target {
        Some(t) => format!(" the '{t}' window"),
        None => " the auto-detected playing window".to_string(),
    };
    format!(
        "Watching{tgt} for {}m{:02}s — {sees} things seen, {hears} things heard.",
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
        "LIVE WATCH CONTEXT — what you are CURRENTLY seeing (SEE) and hearing (HEAR) on the \
         user's screen as a video plays, oldest first, newest last (mm:ss since watching began). \
         ACCURACY RULES: answer ONLY from this log. Quote the HEAR (spoken) lines close to \
         VERBATIM - do NOT paraphrase dramatically, embellish, add backstory, or invent any \
         detail. Preserve names/numbers/spellings exactly as logged. If the log is unclear, \
         partial, or does not contain the answer, say so plainly instead of guessing or \
         filling gaps. Better to say 'I only caught part of that' than to make it up.\n{}",
        lines.join("\n")
    )
}

const CAPTION_PROMPT: &str =
    "You are watching a video on the user's screen, frame by frame, to build an ACCURATE log. \
     Rules: (1) Transcribe any on-screen text EXACTLY as written, character for character - do \
     NOT fix, guess, or normalise spellings, names, or numbers (e.g. write 'Lensr' if that is \
     what it says, never 'Lenser'). (2) Describe ONLY what is literally visible right now - no \
     interpretation, no backstory, no guessing what it means. (3) If text is too small or blurry \
     to read with confidence, say '(text unclear)' rather than inventing it. Keep it to one or \
     two short factual lines. If the screen is a static desktop or page, say so briefly.";

// Mean absolute difference between two equal-length grayscale fingerprints, in
// 0..=255 units. Large for full-motion video / scene cuts, ~0 for a static or
// paused screen. A mismatched/empty pair counts as "fully changed".
fn fp_diff(a: &[u8], b: &[u8]) -> f64 {
    if a.is_empty() || a.len() != b.len() {
        return 255.0;
    }
    let sum: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs() as u64)
        .sum();
    sum as f64 / a.len() as f64
}

fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key).ok().and_then(|s| s.parse().ok()).filter(|n: &f64| *n >= 0.0).unwrap_or(default)
}

// The visual loop: sample the screen cheaply every WATCH_INTERVAL_SECS, but only
// pay for a vision caption when the frame has CHANGED from the last captioned one
// (>= WATCH_CHANGE_THRESHOLD mean-pixel diff) AND at least WATCH_MIN_CAPTION_SECS
// have passed — so a static slide/paused video costs nothing and a fast-cut video
// is rate-limited instead of blindly captioned. Capture runs on a blocking thread
// (xcap types are !Send). Loop exits when watch mode is turned off.
// Route one capture: an explicit window hint, else auto-detect the playing video
// window (Windows), else the whole screen. On Windows, if auto-detection finds
// nothing playing, fall back to the full screen so watching still works.
fn capture_for(target: &Option<String>) -> Result<(String, Vec<u8>, u32, u32), String> {
    match target {
        Some(hint) => crate::tools::screenshot_window_with_fingerprint(hint),
        None => {
            #[cfg(windows)]
            {
                crate::tools::screenshot_auto_window_with_fingerprint()
                    .or_else(|_| crate::tools::screenshot_with_fingerprint())
            }
            #[cfg(not(windows))]
            {
                crate::tools::screenshot_with_fingerprint()
            }
        }
    }
}

fn spawn_visual_loop(target: Option<String>) {
    tokio::spawn(async move {
        let sample_secs = env_u64("WATCH_INTERVAL_SECS", 3);
        let min_caption_secs = env_u64("WATCH_MIN_CAPTION_SECS", 5);
        let threshold = env_f64("WATCH_CHANGE_THRESHOLD", 6.0);
        let mut last_fp: Vec<u8> = Vec::new();
        let mut last_caption_ts: u64 = 0;
        let mut first = true;
        loop {
            if !is_active() {
                break;
            }
            // Capture the target window (visible even behind the HUD), or, with no
            // explicit target, auto-detect the window that is currently playing a
            // video and capture that. Capture is !Send (xcap) -> blocking thread.
            let t = target.clone();
            let shot = tokio::task::spawn_blocking(move || capture_for(&t)).await;
            match shot {
                Ok(Ok((data_url, fp, _w, _h))) => {
                    let changed = first || fp_diff(&last_fp, &fp) >= threshold;
                    let cooled = now().saturating_sub(last_caption_ts) >= min_caption_secs;
                    if changed && cooled {
                        let caption = crate::tools::vision_ask(&data_url, CAPTION_PROMPT).await;
                        if !caption.starts_with("ERROR") {
                            push_see(caption);
                            last_fp = fp;
                            last_caption_ts = now();
                            first = false;
                        } else {
                            eprintln!("[watch] vision error: {caption}");
                        }
                    }
                }
                Ok(Err(e)) => eprintln!("[watch] {e}"), // e.g. window not found (yet)
                Err(_) => {}
            }
            tokio::time::sleep(Duration::from_secs(sample_secs)).await;
        }
        eprintln!("[watch] visual loop stopped");
    });
}

#[cfg(test)]
mod tests {
    use super::fp_diff;

    #[test]
    fn fp_diff_detects_change() {
        let a = vec![100u8; 4096];
        // identical frames -> no change
        assert_eq!(fp_diff(&a, &a), 0.0);
        // a uniformly brighter frame -> diff equals the brightness delta
        let b = vec![140u8; 4096];
        assert!((fp_diff(&a, &b) - 40.0).abs() < 1e-9);
        // length mismatch or empty -> treated as fully changed
        assert_eq!(fp_diff(&a, &[1, 2, 3]), 255.0);
        assert_eq!(fp_diff(&[], &[]), 255.0);
    }
}
