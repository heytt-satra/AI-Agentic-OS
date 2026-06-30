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
            let text = handle.block_on(transcribe_wav(wav));
            let t = text.trim();
            if t.starts_with("ERROR") {
                eprintln!("[hearing] {t}");
            } else if !t.is_empty() {
                crate::watch::push_hear(t.to_string());
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

// POST a WAV to the OpenAI-compatible /audio/transcriptions endpoint (Groq by
// default) and return the plain transcript text, or an ERROR: string.
async fn transcribe_wav(wav: Vec<u8>) -> String {
    let key = transcribe_key();
    if key.is_empty() {
        return "ERROR: no transcription key".into();
    }
    let base = std::env::var("TRANSCRIBE_BASE_URL")
        .unwrap_or_else(|_| "https://api.groq.com/openai/v1".into());
    let model = std::env::var("TRANSCRIBE_MODEL")
        .unwrap_or_else(|_| "whisper-large-v3-turbo".into());
    let part = match reqwest::multipart::Part::bytes(wav).file_name("audio.wav").mime_str("audio/wav") {
        Ok(p) => p,
        Err(e) => return format!("ERROR transcribe part: {e}"),
    };
    let form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model)
        .text("response_format", "text");
    let client = reqwest::Client::new();
    match client
        .post(format!("{base}/audio/transcriptions"))
        .header("Authorization", format!("Bearer {key}"))
        .multipart(form)
        .send()
        .await
    {
        Ok(r) => {
            let s = r.status();
            let body = r.text().await.unwrap_or_default();
            if !s.is_success() {
                return format!("ERROR transcribe {s}: {}", body.chars().take(200).collect::<String>());
            }
            body.trim().to_string()
        }
        Err(e) => format!("ERROR transcribe request: {e}"),
    }
}
