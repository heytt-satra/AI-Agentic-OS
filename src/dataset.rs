// ── src/dataset.rs : the training-dataset exporter (own-model Stage 1) ───────
//
// This is the foundation of the Cursor playbook: turn the history Jarvis is
// ALREADY collecting (messages + audit) into clean, labeled training examples.
// Data is the moat. Models come later (local inference, then fine-tuning, then
// preference tuning on these very labels).
//
// We segment the message log into EPISODES. An episode is one user request plus
// everything Jarvis did until the next user request. Each episode becomes one
// example: (instruction -> response + the tool steps taken) with a REWARD we
// derive from implicit signals:
//   - did it produce a final answer?
//   - did any tool error along the way?
//   - did the user correct it on the very next turn (negative) or move on
//     (positive)?
//
// The reward is a heuristic, not ground truth. It is the starting label for
// later preference tuning (DPO), where good vs bad pairs teach the model YOUR
// taste specifically.

use serde::Serialize;

#[derive(Serialize)]
pub struct Step {
    pub tool: String,
    pub ok: bool,
}

#[derive(Serialize)]
pub struct Example {
    pub instruction: String,
    pub response: String,
    pub steps: Vec<Step>,
    pub tool_errors: usize,
    pub reward: f32,
    pub label: String, // "good" | "neutral" | "bad"
    pub reward_reasons: Vec<String>,
    pub ts: i64,
}

#[derive(Default)]
pub struct Stats {
    pub total: usize,
    pub good: usize,
    pub neutral: usize,
    pub bad: usize,
    pub skipped: usize,
}

// Build labeled examples from the full message + audit history.
// messages: (ts, role, content) chronological. audit: (ts, tool, args, ok).
pub fn build(
    messages: &[(i64, String, String)],
    audit: &[(i64, String, String, bool)],
) -> (Vec<Example>, Stats) {
    let mut stats = Stats::default();

    // Find the index of every user message; each starts an episode.
    let user_idx: Vec<usize> = messages
        .iter()
        .enumerate()
        .filter(|(_, (_, role, _))| role == "user")
        .map(|(i, _)| i)
        .collect();

    // Attribute every audit (tool) row to exactly one episode: the most recent
    // user turn at or before the tool's timestamp. This avoids the boundary
    // problem of second-granular timestamps (a whole turn can share one second).
    // steps_by_ep[k] holds the tool steps for the k-th user turn.
    let ep_start_ts: Vec<i64> = user_idx.iter().map(|&i| messages[i].0).collect();
    let mut steps_by_ep: Vec<Vec<Step>> = (0..user_idx.len()).map(|_| Vec::new()).collect();
    for (ts, tool, _args, ok) in audit {
        // partition_point gives the count of starts <= ts; minus 1 is that episode.
        let k = ep_start_ts.partition_point(|&s| s <= *ts);
        if k > 0 {
            steps_by_ep[k - 1].push(Step { tool: tool.clone(), ok: *ok });
        }
    }

    let mut examples = Vec::new();

    for (k, &start) in user_idx.iter().enumerate() {
        let (start_ts, _, instruction) = &messages[start];
        // Strip a stray UTF-8 BOM (leaks in from piped input) and surrounding space.
        let instruction = instruction.trim_start_matches('\u{feff}').trim().to_string();

        // The next user message bounds this episode (in index) and is the
        // correction signal.
        let next_start = user_idx.get(k + 1).copied().unwrap_or(messages.len());
        let next_user_text = user_idx
            .get(k + 1)
            .map(|&i| messages[i].2.trim().to_string());

        if is_noise(&instruction) {
            stats.skipped += 1;
            continue;
        }

        // The final assistant message inside this episode is the response.
        let response = messages[start + 1..next_start]
            .iter()
            .filter(|(_, role, _)| role == "assistant")
            .next_back()
            .map(|(_, _, c)| c.trim().to_string())
            .unwrap_or_default();

        let steps = std::mem::take(&mut steps_by_ep[k]);
        let tool_errors = steps.iter().filter(|s| !s.ok).count();

        let (reward, label, reward_reasons) =
            score(&response, tool_errors, next_user_text.as_deref(), &instruction);

        match label.as_str() {
            "good" => stats.good += 1,
            "bad" => stats.bad += 1,
            _ => stats.neutral += 1,
        }
        stats.total += 1;

        examples.push(Example {
            instruction,
            response,
            steps,
            tool_errors,
            reward,
            label,
            reward_reasons,
            ts: *start_ts,
        });
    }

    (examples, stats)
}

// Compute the reward, a label, and the human-readable reasons behind it.
fn score(
    response: &str,
    tool_errors: usize,
    next_user: Option<&str>,
    instruction: &str,
) -> (f32, String, Vec<String>) {
    let mut r: f32 = 0.0;
    let mut reasons = Vec::new();

    if response.is_empty() {
        r -= 1.0;
        reasons.push("no final answer".to_string());
    } else {
        r += 1.0;
        reasons.push("produced an answer".to_string());
    }

    if tool_errors > 0 {
        let pen = (0.5 * tool_errors as f32).min(1.5);
        r -= pen;
        reasons.push(format!("{tool_errors} tool error(s)"));
    }

    match next_user {
        Some(n) if is_correction(n, instruction) => {
            r -= 1.0;
            reasons.push("user corrected on the next turn".to_string());
        }
        Some(_) => {
            r += 0.3;
            reasons.push("user moved on".to_string());
        }
        None => {}
    }

    r = r.clamp(-2.0, 2.0);
    let label = if r >= 1.2 {
        "good"
    } else if r < 0.0 {
        "bad"
    } else {
        "neutral"
    };
    (r, label.to_string(), reasons)
}

// Messages we never want as training instructions.
fn is_noise(user: &str) -> bool {
    let u = user.trim().to_lowercase();
    u.is_empty()
        || u == "[heartbeat tick]"
        || u.starts_with("you have reached the step limit") // our own injected nudge
        || u == "exit"
        || u == "quit"
}

// Heuristic: does the user's NEXT message look like they were correcting Jarvis?
// A correction is a strong negative training signal.
fn is_correction(next_user: &str, instruction: &str) -> bool {
    let n = next_user.trim().to_lowercase();
    if n.is_empty() {
        return false;
    }
    const CUES: &[&str] = &[
        "no ", "no,", "nope", "not ", "that's not", "thats not", "that is not",
        "i said", "i didn't", "i did not", "actually", "stop", "don't", "dont",
        "incorrect", "wrong", "you didn't", "you did not", "redo", "that's wrong",
        "not what i", "fix it", "still ", "that's incorrect",
    ];
    if CUES.iter().any(|c| n.starts_with(c) || n.contains(c)) {
        return true;
    }
    // A short, near-identical re-ask usually means the first try missed.
    if n.len() < 80 {
        let overlap = token_overlap(&n, &instruction.to_lowercase());
        if overlap > 0.6 {
            return true;
        }
    }
    false
}

// Fraction of the smaller token set that appears in the larger one.
fn token_overlap(a: &str, b: &str) -> f32 {
    let at: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let bt: std::collections::HashSet<&str> = b.split_whitespace().collect();
    if at.is_empty() || bt.is_empty() {
        return 0.0;
    }
    let inter = at.intersection(&bt).count();
    inter as f32 / at.len().min(bt.len()) as f32
}

// Serialize examples to JSONL (one JSON object per line) - the standard shape
// for fine-tuning and preference-tuning pipelines.
pub fn to_jsonl(examples: &[Example]) -> String {
    let mut out = String::new();
    for ex in examples {
        if let Ok(line) = serde_json::to_string(ex) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}
