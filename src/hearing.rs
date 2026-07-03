// ── src/hearing.rs : the ears — capture system audio and transcribe it ─────────
//
// Windows-only (gated cfg(windows) in main.rs, like uiautomation). Captures the
// audio playing through the speakers via WASAPI loopback, chunks it, and sends
// each chunk to a cloud transcription API (Groq whisper by default) behind a
// provider-style env seam. Transcribed lines feed watch::push_hear, so the live
// watch buffer carries BOTH what's on screen (SEE) and what's said (HEAR).
//
// Env seam:
//   TRANSCRIBE_API_KEY (or GROQ_API_KEY) — required; without it audio is disabled
//   TRANSCRIBE_BASE_URL  — default https://api.groq.com/openai/v1
//   TRANSCRIBE_MODEL     — default whisper-large-v3-turbo
//   HEAR_CHUNK_SECS      — seconds of audio per request (default 12)

use std::collections::VecDeque;
use wasapi::{Direction, SampleType, StreamMode, WaveFormat};

const SR: u32 = 16_000; // 16 kHz mono is what whisper wants

fn chunk_secs() -> usize {
    std::env::var("HEAR_CHUNK_SECS").ok().and_then(|s| s.parse().ok()).filter(|n| *n > 0).unwrap_or(12)
}

fn transcribe_key() -> String {
    std::env::var("TRANSCRIBE_API_KEY")
        .or_else(|_| std::env::var("GROQ_API_KEY"))
        .unwrap_or_default()
}

/// Process IDs that are CURRENTLY playing audio (active render sessions on the
/// default output device). This is the truest "what is playing" signal - whatever
/// app is making sound is almost certainly the video - and it works across any
/// browser or player. Returns empty if nothing is playing or on any error.
pub fn active_audio_pids() -> Vec<u32> {
    let _ = wasapi::initialize_mta();
    let mut pids = Vec::new();
    let en = match wasapi::DeviceEnumerator::new() {
        Ok(e) => e,
        Err(_) => return pids,
    };
    let dev = match en.get_default_device(&Direction::Render) {
        Ok(d) => d,
        Err(_) => return pids,
    };
    let mgr = match dev.get_iaudiosessionmanager() {
        Ok(m) => m,
        Err(_) => return pids,
    };
    let sessions = match mgr.get_audiosessionenumerator() {
        Ok(s) => s,
        Err(_) => return pids,
    };
    for i in 0..sessions.get_count().unwrap_or(0) {
        if let Ok(ctrl) = sessions.get_session(i) {
            if matches!(ctrl.get_state(), Ok(wasapi::SessionState::Active)) {
                if let Ok(pid) = ctrl.get_process_id() {
                    if pid != 0 && !pids.contains(&pid) {
                        pids.push(pid);
                    }
                }
            }
        }
    }
    pids
}

/// Start the background audio loop. No-op (with a hint) if no transcription key
/// is set, so watching still works visually. Spawns a dedicated OS thread because
/// WASAPI wants its own COM-initialized thread with blocking reads.
pub fn spawn_audio_loop() {
    if transcribe_key().is_empty() {
        eprintln!(
            "[hearing] no transcription key (set GROQ_API_KEY or TRANSCRIBE_API_KEY) - \
             watching is visual-only. Free key: https://console.groq.com/keys"
        );
        return;
    }
    let handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        if let Err(e) = capture_and_transcribe(handle) {
            eprintln!("[hearing] audio loop stopped: {e}");
        }
    });
}

// Open the default render (output) device for LOOPBACK capture, asking WASAPI to
// auto-convert to 16 kHz mono i16 so we can ship chunks straight to whisper.
fn open_loopback() -> Result<(wasapi::AudioClient, wasapi::AudioCaptureClient, wasapi::Handle), String> {
    // COM init for this thread (S_FALSE if already initialized — both fine).
    let _ = wasapi::initialize_mta();
    // Loopback = the default RENDER (output) device opened for CAPTURE; the crate
    // sets AUDCLNT_STREAMFLAGS_LOOPBACK automatically for that (Render, Capture,
    // Shared) combination.
    let enumerator = wasapi::DeviceEnumerator::new().map_err(|e| format!("device enumerator: {e}"))?;
    let device = enumerator
        .get_default_device(&Direction::Render)
        .map_err(|e| format!("default output device: {e}"))?;
    let mut client = device.get_iaudioclient().map_err(|e| format!("audio client: {e}"))?;
    let format = WaveFormat::new(16, 16, &SampleType::Int, SR as usize, 1, None);
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 };
    client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|e| format!("initialize loopback: {e}"))?;
    let capture = client.get_audiocaptureclient().map_err(|e| format!("capture client: {e}"))?;
    let h_event = client.set_get_eventhandle().map_err(|e| format!("event handle: {e}"))?;
    client.start_stream().map_err(|e| format!("start stream: {e}"))?;
    Ok((client, capture, h_event))
}

fn capture_and_transcribe(handle: tokio::runtime::Handle) -> Result<(), String> {
    let (client, capture, h_event) = open_loopback()?;
    let bytes_per_chunk = SR as usize * 2 * chunk_secs(); // i16 mono
    let mut buf: VecDeque<u8> = VecDeque::new();
    eprintln!("[hearing] listening to system audio ({}s chunks)", chunk_secs());
    while crate::watch::is_active() {
        capture
            .read_from_device_to_deque(&mut buf)
            .map_err(|e| format!("read: {e}"))?;
        while buf.len() >= bytes_per_chunk {
            let chunk: Vec<u8> = buf.drain(..bytes_per_chunk).collect();
            // skip near-silent chunks (no point paying to transcribe silence)
            if rms_i16(&chunk) < 60.0 {
                continue;
            }
            let wav = wav_pcm16_mono(&chunk, SR);
            match handle.block_on(transcribe_wav(wav)) {
                Err(e) => eprintln!("[hearing] {e}"),
                Ok(h) if !h.text.is_empty() => crate::watch::push_hear(h.text, h.low_conf),
                Ok(_) => {} // empty transcript (silence/non-speech) - drop
            }
        }
        let _ = h_event.wait_for_event(500); // short so we notice watch-off fast
    }
    let _ = client.stop_stream();
    eprintln!("[hearing] audio loop stopped (watch off)");
    Ok(())
}

/// Capture `secs` seconds of loopback audio and report (sample_count, rms). Used
/// by the `hear-test` subcommand to prove capture works without needing a key.
pub fn selftest_capture(secs: usize) -> Result<(usize, f64), String> {
    let (client, capture, h_event) = open_loopback()?;
    let want = SR as usize * 2 * secs;
    let mut buf: VecDeque<u8> = VecDeque::new();
    // Hard wall-clock deadline so a SILENT stream (which never fires the event)
    // can't hang the test; a short event timeout keeps each spin cheap.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs as u64 + 2);
    while buf.len() < want && std::time::Instant::now() < deadline {
        capture.read_from_device_to_deque(&mut buf).map_err(|e| format!("read: {e}"))?;
        let _ = h_event.wait_for_event(200);
    }
    let _ = client.stop_stream();
    let data: Vec<u8> = buf.into_iter().collect();
    Ok((data.len() / 2, rms_i16(&data)))
}

// Root-mean-square amplitude of little-endian i16 PCM, in 0..=32767 units. ~0 for
// silence; meaningfully positive when audio is playing.
fn rms_i16(pcm: &[u8]) -> f64 {
    let n = pcm.len() / 2;
    if n == 0 {
        return 0.0;
    }
    let mut sum_sq = 0.0f64;
    for s in pcm.chunks_exact(2) {
        let v = i16::from_le_bytes([s[0], s[1]]) as f64;
        sum_sq += v * v;
    }
    (sum_sq / n as f64).sqrt()
}

// Wrap raw i16 mono PCM in a minimal WAV container (44-byte header) so the
// transcription API accepts it. No external crate - keeps zero-install.
fn wav_pcm16_mono(pcm: &[u8], sr: u32) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = sr * 2;
    let mut v = Vec::with_capacity(44 + pcm.len());
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes()); // PCM fmt chunk size
    v.extend_from_slice(&1u16.to_le_bytes()); // format = PCM
    v.extend_from_slice(&1u16.to_le_bytes()); // channels = mono
    v.extend_from_slice(&sr.to_le_bytes());
    v.extend_from_slice(&byte_rate.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes()); // block align
    v.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    v.extend_from_slice(pcm);
    v
}

// One transcribed audio chunk: the (possibly multi-turn) text plus a flag for
// when the model's own confidence was low, so the watch log can mark it.
pub struct Heard {
    pub text: String,
    pub low_conf: bool,
}

// Whisper's avg_logprob is ~-0.1..-0.3 for clean speech and drops well below
// this when it is unsure or hallucinating over noise/music.
fn conf_floor() -> f64 {
    std::env::var("HEAR_CONF_FLOOR").ok().and_then(|s| s.parse().ok()).unwrap_or(-0.7)
}
// A gap between spoken segments longer than this (seconds) is treated as a new
// turn - usually a change of speaker - and marked with a divider.
fn turn_gap() -> f64 {
    std::env::var("HEAR_TURN_GAP_SECS").ok().and_then(|s| s.parse().ok()).filter(|n: &f64| *n > 0.0).unwrap_or(1.4)
}

// POST a WAV to the OpenAI-compatible /audio/transcriptions endpoint (Groq by
// default). We ask for verbose_json so we get per-segment timings and
// confidence: that lets us (1) flag shaky transcripts and (2) split distinct
// spoken turns on audio pauses. Returns Err(msg) on any failure.
async fn transcribe_wav(wav: Vec<u8>) -> Result<Heard, String> {
    let key = transcribe_key();
    if key.is_empty() {
        return Err("no transcription key".into());
    }
    let base = std::env::var("TRANSCRIBE_BASE_URL")
        .unwrap_or_else(|_| "https://api.groq.com/openai/v1".into());
    let model = std::env::var("TRANSCRIBE_MODEL")
        .unwrap_or_else(|_| "whisper-large-v3-turbo".into());
    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("transcribe part: {e}"))?;
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model)
        .text("response_format", "verbose_json");
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{base}/audio/transcriptions"))
        .header("Authorization", format!("Bearer {key}"))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("transcribe request: {e}"))?;
    let s = r.status();
    let body = r.text().await.unwrap_or_default();
    if !s.is_success() {
        return Err(format!("transcribe {s}: {}", body.chars().take(200).collect::<String>()));
    }
    Ok(parse_verbose(&body))
}

// Turn a verbose_json transcription body into a Heard. Falls back gracefully to
// the flat "text" field if a provider doesn't return segments.
fn parse_verbose(body: &str) -> Heard {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        // Not JSON (e.g. a plain-text provider) - use it verbatim.
        Err(_) => return Heard { text: body.trim().to_string(), low_conf: false },
    };
    let segs = v["segments"].as_array();
    let Some(segs) = segs else {
        return Heard { text: v["text"].as_str().unwrap_or("").trim().to_string(), low_conf: false };
    };

    let gap = turn_gap();
    let mut out = String::new();
    let mut logprob_sum = 0.0;
    let mut logprob_n = 0.0;
    let mut prev_end: Option<f64> = None;
    for seg in segs {
        let txt = seg["text"].as_str().unwrap_or("").trim();
        if txt.is_empty() {
            continue;
        }
        // Drop segments the model itself thinks are probably not speech - this is
        // what kills whisper's "thanks for watching" hallucinations over music.
        if seg["no_speech_prob"].as_f64().unwrap_or(0.0) > 0.6 {
            continue;
        }
        if let Some(lp) = seg["avg_logprob"].as_f64() {
            logprob_sum += lp;
            logprob_n += 1.0;
        }
        let start = seg["start"].as_f64().unwrap_or(0.0);
        if let Some(pe) = prev_end {
            out.push_str(if start - pe > gap { " | " } else { " " });
        }
        out.push_str(txt);
        prev_end = Some(seg["end"].as_f64().unwrap_or(start));
    }
    let mean_lp = if logprob_n > 0.0 { logprob_sum / logprob_n } else { 0.0 };
    Heard { text: out.trim().to_string(), low_conf: logprob_n > 0.0 && mean_lp < conf_floor() }
}

#[cfg(test)]
mod tests {
    use super::parse_verbose;

    #[test]
    fn verbose_json_splits_turns_and_flags_low_confidence() {
        // two segments with a 2s gap between them -> a turn divider; clean logprob
        let body = r#"{"text":"hello there general","segments":[
            {"start":0.0,"end":1.0,"text":"hello","avg_logprob":-0.2,"no_speech_prob":0.01},
            {"start":3.0,"end":4.0,"text":"there general","avg_logprob":-0.25,"no_speech_prob":0.02}
        ]}"#;
        let h = parse_verbose(body);
        assert_eq!(h.text, "hello | there general");
        assert!(!h.low_conf);
    }

    #[test]
    fn verbose_json_drops_nonspeech_and_flags_shaky() {
        // one real segment (shaky logprob) + one non-speech hallucination dropped
        let body = r#"{"text":"x","segments":[
            {"start":0.0,"end":1.0,"text":"muffled words","avg_logprob":-1.1,"no_speech_prob":0.1},
            {"start":1.0,"end":2.0,"text":"thanks for watching","avg_logprob":-0.3,"no_speech_prob":0.9}
        ]}"#;
        let h = parse_verbose(body);
        assert_eq!(h.text, "muffled words");
        assert!(h.low_conf);
    }

    #[test]
    fn falls_back_to_flat_text_without_segments() {
        let h = parse_verbose(r#"{"text":"just a sentence"}"#);
        assert_eq!(h.text, "just a sentence");
        assert!(!h.low_conf);
    }
}
