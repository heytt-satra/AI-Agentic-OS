use std::time::Instant;
use tokio::time::{sleep, Duration};

// `#[tokio::main]` is a macro that rewrites this async function into a normal
// `fn main` that boots the tokio runtime, then runs our async code on it.
// Without it, you cannot write `async fn main` — `main` must be sync.
#[tokio::main]
async fn main() {
    println!("Jarvis async core: online.\n");

    let start = Instant::now();

    // Pretend each of these is a slow thing an agent waits on (an API call,
    // a web fetch). Each "takes" 1 second. Synchronously that's 2 seconds.
    let task_a = fake_slow_work("calling Claude", 1000);
    let task_b = fake_slow_work("fetching the news", 1000);

    // `tokio::join!` runs BOTH at the same time and waits for both to finish.
    // Because they overlap, total time is ~1s, not ~2s. That overlap is the
    // entire reason an agent uses async.
    tokio::join!(task_a, task_b);

    println!("\nBoth finished in {} ms (not 2000).", start.elapsed().as_millis());
}

// `async fn` returns a "future": a value that represents work not done yet.
// Nothing runs until something `.await`s it (here, tokio::join! does).
async fn fake_slow_work(label: &str, millis: u64) {
    sleep(Duration::from_millis(millis)).await; // .await = "pause here until done, let others run"
    println!("  done: {label}");
}
