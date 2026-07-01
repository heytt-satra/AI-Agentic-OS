// ── src/server.rs : the Jarvis web HUD (axum HTTP + WebSocket) ──────────────
//
// `jarvis serve` starts a local server and opens a futuristic browser UI.
// The browser talks to the Rust core over a WebSocket: it sends your text,
// the server runs the agent turn and streams back state + tool + answer events.
//
// Why a browser instead of a native window: it runs on ANY OS with zero extra
// install, and the binary serves the whole UI itself (HTML is embedded).

use crate::memory::MemoryHandle;
use crate::policy;
use crate::provider::{Message, Provider};
use crate::tools;
use anyhow::Result;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{Html, Response};
use axum::routing::get;
use axum::Router;

// Per-turn tool-call budget for the HUD path. Generous for code-building;
// overridable via JARVIS_MAX_STEPS. Only a backstop — the model stops when done.
fn max_steps() -> u32 {
    std::env::var("JARVIS_MAX_STEPS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(40)
}

#[derive(Clone)]
struct AppState {
    provider: Provider,
    mem: MemoryHandle,
}

pub async fn serve(provider: Provider, mem: MemoryHandle) -> Result<()> {
    // Scheduler: while the HUD is up, run any due scheduled agents (Phase 3).
    spawn_scheduler(provider.clone(), mem.clone());
    // Proactive sensing loop: periodically review recent activity + learnings and
    // queue a nudge if something is worth raising. Off with JARVIS_PROACT=off.
    if std::env::var("JARVIS_PROACT").unwrap_or_default() != "off" {
        let p = provider.clone();
        let m = mem.clone();
        tokio::spawn(async move {
            let secs: u64 = std::env::var("PROACT_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(900);
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(secs.max(60)));
            tick.tick().await; // skip the immediate first tick
            loop {
                tick.tick().await;
                crate::run_proact(&p, &m).await;
            }
        });
    }
    let state = AppState { provider, mem };
    let app = Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr = "127.0.0.1:7878";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let url = format!("http://{addr}");
    println!("\n  Jarvis HUD online -> {url}\n  (opening your browser; Ctrl-C to stop)\n");
    // Global summon hotkey (Ctrl+Alt+J) opens/focuses the HUD from anywhere.
    #[cfg(windows)]
    crate::hotkey::spawn(url.clone());
    open_browser(&url);
    axum::serve(listener, app).await?;
    Ok(())
}

// Background scheduler: every minute, run any saved agents whose schedule is due.
fn spawn_scheduler(provider: Provider, mem: MemoryHandle) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            ticker.tick().await;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            for (id, agent, every) in mem.schedule_due(now).await {
                if let Some(instr) = mem.agent_get(&agent).await {
                    let result = crate::run_subagent(&provider, &mem, &format!("scheduled agent '{agent}'"), &instr, 0).await;
                    mem.log("assistant", &format!("[scheduled run: {agent}] {}", result.chars().take(600).collect::<String>())).await;
                    eprintln!("[scheduler] ran '{agent}'");
                }
                mem.schedule_mark_run(id, now + every.max(60)).await;
            }
        }
    });
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn ws_handler(ws: WebSocketUpgrade, State(st): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, st))
}

async fn handle_socket(mut socket: WebSocket, st: AppState) {
    let _ = send(&mut socket, serde_json::json!({"type":"state","state":"idle"})).await;
    let _ = send(
        &mut socket,
        serde_json::json!({"type":"hello","model": st.provider.model()}),
    )
    .await;

    let mut messages = vec![Message::system(crate::system_prompt())];

    // Continuous-learning spine: load the stable profile so the HUD session, like
    // the REPL, starts already knowing the user.
    let profile = st.mem.top_learnings(6).await;
    if !profile.is_empty() {
        let p = profile.iter().map(|(k, t, _)| format!("- [{k}] {t}")).collect::<Vec<_>>().join("\n");
        messages.push(Message::system(format!(
            "What you have LEARNED about this user across past sessions (persisted; act consistently with it):\n{p}"
        )));
    }
    // Self-direction: Jarvis's own active hypotheses/goals (resolve via goal_update).
    let active_goals: Vec<_> = st.mem.goals_list().await.into_iter()
        .filter(|(_, _, _, s)| s == "open" || s == "testing").take(6).collect();
    if !active_goals.is_empty() {
        let gl = active_goals.iter().map(|(id, k, t, s)| format!("#{id} [{k}/{s}] {t}")).collect::<Vec<_>>().join("\n");
        messages.push(Message::system(format!(
            "Your OWN current hypotheses/goals (self-direction). If the user's message confirms, answers, or relates to one, resolve it with goal_update (and learn any confirmed fact). Otherwise ignore:\n{gl}"
        )));
    }

    while let Some(Ok(msg)) = socket.recv().await {
        let text = match msg {
            WsMessage::Text(t) => t.as_str().to_owned(),
            WsMessage::Close(_) => break,
            _ => continue,
        };
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        let user_text = parsed
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if user_text.is_empty() {
            continue;
        }

        // Semantic relevance recall (same as the REPL).
        let relevant = st.mem.search(&user_text, 3).await;
        if !relevant.is_empty() {
            let ctx = relevant
                .iter()
                .map(|(r, c)| format!("- ({r}) {c}"))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(Message::system(format!("Possibly relevant memory:\n{ctx}")));
        }
        // Continuous-learning spine: durable learnings relevant to THIS question.
        let learned = st.mem.recall_learnings(&user_text, 5).await;
        if !learned.is_empty() {
            let l = learned.iter().map(|(k, t, _)| format!("- [{k}] {t}")).collect::<Vec<_>>().join("\n");
            messages.push(Message::system(format!("Relevant things you've learned about this user:\n{l}")));
        }
        // Proactive: surface a queued background nudge as gentle context (not an
        // imperative, which derails weaker models into just acknowledging it).
        if let Some(nudge) = st.mem.nudge_take().await {
            messages.push(Message::system(format!(
                "(Background observation from your own sensing - mention it to the user only if it is relevant or helpful right now, otherwise ignore it: {nudge})"
            )));
        }
        // Live watch-along: hand the agent everything it is currently seeing/
        // hearing on screen, so the user can ask about a playing video (same as
        // the REPL path).
        if crate::watch::is_active() {
            let live = crate::watch::context_snapshot();
            if !live.is_empty() {
                messages.push(Message::system(live));
            }
        }
        messages.push(Message::user(&user_text));
        st.mem.log("user", &user_text).await;

        let _ = send(&mut socket, serde_json::json!({"type":"state","state":"thinking"})).await;

        let mut tainted = false;
        let mut answered = false;
        let mut seen: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        'turn: for _ in 0..max_steps() {
            // Streamed model call: content tokens go to the browser live.
            let (dtx, mut drx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let reply = {
                let fut = st.provider.chat_stream(&messages, Some(tools::all_definitions().await), dtx);
                tokio::pin!(fut);
                loop {
                    tokio::select! {
                        Some(piece) = drx.recv() => {
                            let _ = send(&mut socket, serde_json::json!({"type":"delta","text":piece})).await;
                        }
                        r = &mut fut => {
                            while let Ok(piece) = drx.try_recv() {
                                let _ = send(&mut socket, serde_json::json!({"type":"delta","text":piece})).await;
                            }
                            break r;
                        }
                    }
                }
            };
            let reply = match reply {
                Ok(r) => r,
                Err(e) => {
                    let _ = send(&mut socket, serde_json::json!({"type":"error","text":format!("{e}")})).await;
                    break;
                }
            };
            messages.push(reply.message.clone());

            if reply.finish_reason == "tool_calls" {
                for call in reply.message.tool_calls.clone().unwrap_or_default() {
                    let name = call.function.name.clone();
                    let args = call.function.arguments.clone();

                    // Runaway-loop guard (gap 7): stop repeating the same call.
                    let sig = format!("{name}|{args}");
                    let c = seen.entry(sig).or_insert(0);
                    *c += 1;
                    if *c >= 4 {
                        let _ = send(&mut socket, serde_json::json!({"type":"answer","text":"I caught myself repeating the same action and stopped to avoid a loop, sir. Could you rephrase or give me more to go on?"})).await;
                        answered = true;
                        break 'turn;
                    }

                    let risk = policy::assess(&name, &args);

                    // Approval (over the WebSocket) if the policy demands it.
                    let (decision, run) = decide_hud(&mut socket, &st.mem, &name, &risk, tainted).await;

                    let _ = send(&mut socket, serde_json::json!({"type":"tool","name":name})).await;
                    let _ = send(&mut socket, serde_json::json!({"type":"state","state":"working"})).await;

                    let result = if run {
                        tools::execute(&name, &args, &st.mem, &st.provider, 0).await
                    } else {
                        "DENIED by user".to_string()
                    };
                    let ok = tools::result_ok(&result);
                    st.mem.log_audit(&name, &args, &decision, ok).await;
                    if matches!(name.as_str(), "fetch_url" | "news_search" | "web_search" | "extract_contacts" | "browse_url" | "browse_js") {
                        tainted = true;
                    }
                    st.mem.log("tool", &result).await;
                    messages.push(Message::tool_result(call.id, result));
                }
                continue;
            }

            let answer = reply.message.content.unwrap_or_else(|| "(no answer)".into());
            st.mem.log("assistant", &answer).await;
            // Content was already streamed as deltas; just finalize the bubble.
            let _ = send(&mut socket, serde_json::json!({"type":"done"})).await;
            answered = true;
            break;
        }
        if !answered {
            // Out of tool-call budget. Ask for a short status (no tools) instead
            // of erroring; the conversation persists so the user can say "continue".
            messages.push(Message::user(
                "You have reached the step limit for this turn. Stop calling tools. In two \
                 or three sentences, tell me what you accomplished, what still remains, and \
                 the project path if relevant. Be honest about what is not finished.",
            ));
            match st.provider.chat(&messages, None).await {
                Ok(r) => {
                    let answer = r.message.content.unwrap_or_else(|| {
                        "Hit the step limit before finishing, sir. Say 'continue' and I'll resume.".into()
                    });
                    st.mem.log("assistant", &answer).await;
                    let _ = send(&mut socket, serde_json::json!({"type":"answer","text":answer})).await;
                }
                Err(_) => {
                    let _ = send(&mut socket, serde_json::json!({"type":"answer","text":"Hit the step limit before finishing, sir. Say 'continue' and I'll resume where I left off."})).await;
                }
            }
        }
        let _ = send(&mut socket, serde_json::json!({"type":"state","state":"idle"})).await;
        crate::trim_messages(&mut messages, 16);
    }
}

// Ask the browser to approve a risky action; block this turn until it replies.
async fn decide_hud(
    socket: &mut WebSocket,
    mem: &MemoryHandle,
    tool: &str,
    risk: &policy::Risk,
    tainted: bool,
) -> (String, bool) {
    if !risk.needs_approval {
        return ("auto".to_string(), true);
    }
    if !tainted {
        match mem.check_permission(tool, &risk.key).await {
            Some(true) => return ("auto-allowed".to_string(), true),
            Some(false) => return ("auto-denied".to_string(), false),
            None => {}
        }
    }
    let _ = send(socket, serde_json::json!({"type":"approval","label":risk.label,"tainted":tainted})).await;
    // Wait for the user's click (an approval_response message).
    while let Some(Ok(msg)) = socket.recv().await {
        if let WsMessage::Text(t) = msg {
            let v: serde_json::Value = serde_json::from_str(t.as_str()).unwrap_or_default();
            if v.get("type").and_then(|x| x.as_str()) == Some("approval_response") {
                return match v.get("decision").and_then(|x| x.as_str()).unwrap_or("deny") {
                    "once" => ("approved".to_string(), true),
                    "always" => {
                        mem.remember_permission(tool, &risk.key, true).await;
                        ("approved-always".to_string(), true)
                    }
                    _ => ("denied".to_string(), false),
                };
            }
        }
    }
    ("denied".to_string(), false)
}

async fn send(socket: &mut WebSocket, v: serde_json::Value) -> Result<()> {
    socket.send(WsMessage::Text(v.to_string().into())).await?;
    Ok(())
}

fn open_browser(url: &str) {
    // Under the supervisor (`jarvis daemon`), don't reopen the browser on every
    // restart - the user summons the HUD with the hotkey or the URL instead.
    if std::env::var("JARVIS_NO_BROWSER").is_ok() {
        return;
    }
    let _ = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/c", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
}

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8"/>
<meta name="viewport" content="width=device-width, initial-scale=1"/>
<title>JARVIS</title>
<style>
  /* JARVIS-OS HUD - instrument-panel redesign (see DESIGN.md)
     amber = system, cyan = live data only, red = danger only.
     Two zones: a left status rail + a focused conversation column. */
  :root{
    --bg:#04060a; --surface:#0a0f15; --surface2:#0c131c;
    --amber:#ffb000; --amber-soft:#ffd98a; --amber-dim:#bb8c30;
    --cyan:#3ad8c4; --red:#ff5c5c;
    --ink:#dbe3ea; --muted:#93a1b1; --faint:#727f8e;
    --line:rgba(255,176,0,.16); --line2:rgba(255,255,255,.07);
    --mono:'SF Mono','JetBrains Mono','Cascadia Code','Cascadia Mono',Consolas,ui-monospace,monospace;
  }
  *{box-sizing:border-box}
  html,body{height:100%;margin:0}
  body{
    background:radial-gradient(130% 100% at 18% -10%, #0a121c 0%, var(--bg) 55%);
    color:var(--ink); font-family:var(--mono); overflow:hidden; -webkit-font-smoothing:antialiased;
  }
  .app{display:grid; grid-template-columns:300px 1fr; height:100vh}

  /* ── left instrument rail ─────────────────────────────────── */
  .rail{
    display:flex; flex-direction:column; align-items:center; gap:18px;
    padding:26px 22px;
    background:linear-gradient(180deg, var(--surface) 0%, var(--bg) 100%);
    border-right:1px solid var(--line);
  }
  .brand{align-self:stretch; display:flex; align-items:baseline; gap:9px;
    font-size:17px; letter-spacing:.42em; color:var(--amber); font-weight:600}
  .brand .ver{font-size:9px; letter-spacing:.3em; color:var(--faint); font-weight:400}
  .orbWrap{display:flex; flex-direction:column; align-items:center; gap:11px; margin-top:6px}
  #orb{display:block; width:188px; height:188px}
  #state{font-size:11px; letter-spacing:.34em; color:var(--cyan); text-transform:uppercase}
  #tool{height:13px; font-size:9.5px; letter-spacing:.2em; color:var(--amber-dim);
    text-transform:uppercase; opacity:0; transition:opacity .3s; text-align:center}

  .panel{align-self:stretch; display:flex; flex-direction:column; gap:1px; margin-top:4px;
    border:1px solid var(--line2); border-radius:3px; overflow:hidden}
  .row{display:flex; justify-content:space-between; align-items:center; gap:10px;
    padding:11px 13px; background:var(--surface2); font-size:11.5px}
  .row .k{color:var(--faint); letter-spacing:.18em; text-transform:uppercase; font-size:9.5px}
  .row .v{color:var(--ink); text-align:right; word-break:break-word}
  #model{color:var(--amber-soft)}
  #conn{color:var(--cyan)} #conn .dot{margin-right:6px}
  .toggle{cursor:pointer; user-select:none; color:var(--muted)} .toggle.on{color:var(--cyan)}
  .hint{margin-top:auto; align-self:stretch; font-size:9.5px; letter-spacing:.16em;
    text-transform:uppercase; color:var(--faint); line-height:1.8;
    border-top:1px solid var(--line2); padding-top:14px}
  .hint b{color:var(--amber-dim); font-weight:400}

  /* ── main conversation column ─────────────────────────────── */
  .main{display:flex; flex-direction:column; min-height:0}
  .topline{height:2px; flex:0 0 auto;
    background:linear-gradient(90deg, var(--amber) 0%, transparent 42%); opacity:.5}
  #log{
    flex:1; overflow-y:auto; padding:32px clamp(20px,6vw,90px);
    display:flex; flex-direction:column; align-items:center; gap:22px;
    scrollbar-width:thin; scrollbar-color:var(--line) transparent;
  }
  #log::-webkit-scrollbar{width:7px}
  #log::-webkit-scrollbar-thumb{background:var(--line); border-radius:4px}
  #empty{margin:auto; max-width:420px; text-align:center}
  #empty .big{font-size:13px; letter-spacing:.2em; text-transform:uppercase;
    color:var(--amber-soft); margin-bottom:10px}
  #empty .sub{font-size:12.5px; line-height:1.85; color:var(--faint)}
  .msg{width:100%; max-width:760px; font-size:14.5px; line-height:1.62;
    white-space:pre-wrap; word-wrap:break-word; padding-left:16px;
    border-left:2px solid var(--line2); animation:rise .4s ease-out both}
  @keyframes rise{from{opacity:0; transform:translateY(6px)} to{opacity:1; transform:none}}
  .msg .who{font-size:10px; letter-spacing:.22em; text-transform:uppercase;
    color:var(--muted); display:block; margin-bottom:7px}
  /* speaker distinction: a colored left rule per role, so the dialogue scans */
  .user{border-left-color:rgba(219,227,234,.22)}
  .user .body{color:var(--ink)}
  .jarvis{border-left-color:var(--amber)}
  .jarvis .who{color:var(--amber)}
  .jarvis .body{color:var(--amber-soft)}
  .err{border-left-color:var(--red)}
  .err .who{color:var(--red)}
  .err .body{color:var(--red)}

  .composer{padding:16px clamp(20px,6vw,90px) 24px; border-top:1px solid var(--line2)}
  .inbar{
    display:flex; align-items:center; gap:12px; max-width:760px; margin:0 auto;
    border:1px solid var(--line); border-radius:3px; background:var(--surface);
    padding:14px 16px; transition:border-color .2s, box-shadow .2s;
  }
  .inbar:focus-within{border-color:var(--amber); box-shadow:0 0 0 1px rgba(255,176,0,.18)}
  .inbar .chev{color:var(--amber)}
  #in{flex:1; background:transparent; border:0; outline:0; color:var(--ink);
    font-family:var(--mono); font-size:14.5px; caret-color:var(--amber)}
  #in::placeholder{color:var(--muted)}
  #mic{font-family:var(--mono); font-size:10px; letter-spacing:.12em; text-transform:uppercase;
    color:var(--muted); background:transparent; border:1px solid var(--line); border-radius:3px;
    padding:8px 12px; cursor:pointer; transition:.15s; white-space:nowrap}
  #mic:hover{border-color:var(--amber); color:var(--amber)}
  #mic.live{border-color:var(--cyan); color:var(--cyan); animation:micpulse 1s ease-in-out infinite}
  @keyframes micpulse{0%,100%{opacity:1}50%{opacity:.5}}

  /* ── approval modal ───────────────────────────────────────── */
  #approvalWrap{position:fixed;inset:0;background:rgba(2,4,7,.82);display:none;
    align-items:center;justify-content:center;z-index:20;backdrop-filter:blur(3px)}
  #approvalWrap.show{display:flex}
  .approval{width:min(520px,90vw);border:1px solid var(--red);border-radius:3px;
    background:var(--surface);padding:24px}
  .approval .h{font-size:10px;letter-spacing:.24em;text-transform:uppercase;
    color:var(--red);margin-bottom:12px}
  .approval .lbl{font-size:14px;color:var(--amber-soft);word-break:break-word;margin-bottom:6px}
  .approval .warn{font-size:11px;color:var(--red);margin-bottom:18px;min-height:14px}
  .approval .btns{display:flex;gap:10px}
  .approval button{flex:1;font-family:var(--mono);font-size:10.5px;letter-spacing:.1em;
    text-transform:uppercase;padding:12px;background:transparent;color:var(--ink);
    border:1px solid var(--line);border-radius:3px;cursor:pointer;transition:.15s}
  .approval button:hover{border-color:var(--amber);color:var(--amber)}
  .approval button.deny:hover{border-color:var(--red);color:var(--red)}

  /* ── responsive: rail collapses to a top strip ────────────── */
  @media(max-width:760px){
    .app{grid-template-columns:1fr; grid-template-rows:auto 1fr}
    .rail{flex-direction:row; flex-wrap:wrap; align-items:center; gap:12px;
      padding:13px 16px; border-right:0; border-bottom:1px solid var(--line)}
    .brand{align-self:auto; font-size:14px; letter-spacing:.3em}
    .orbWrap{flex-direction:row; gap:9px; margin:0} #orb{width:38px; height:38px}
    #tool{display:none}
    .panel{flex:1 1 100%; flex-direction:row; flex-wrap:wrap; gap:8px; margin:0; border:0}
    .row{flex:1 1 auto; border:1px solid var(--line2); border-radius:3px; padding:7px 11px}
    .hint{display:none}
  }
</style>
</head>
<body>
  <div class="app">
    <aside class="rail">
      <div class="brand">JARVIS <span class="ver">OS</span></div>
      <div class="orbWrap">
        <canvas id="orb" width="260" height="260"></canvas>
        <div id="state">STANDBY</div>
        <div id="tool"></div>
      </div>
      <div class="panel">
        <div class="row"><span class="k">Model</span><span class="v" id="model">…</span></div>
        <div class="row"><span class="k">Link</span><span class="v" id="conn"><span class="dot">&#9679;</span>CONNECTING</span></div>
        <div class="row"><span class="k">Voice</span><span class="v toggle" id="voice">OFF</span></div>
      </div>
      <div class="hint">On your machine. <b>Private by default.</b><br/>Ask me to do anything.</div>
    </aside>
    <section class="main">
      <div class="topline"></div>
      <div id="log">
        <div id="empty">
          <div class="big">Standby</div>
          <div class="sub">Type below or hit Talk. I can run apps, write code, search the web, find leads, and act on your machine.</div>
        </div>
      </div>
      <footer class="composer">
        <div class="inbar">
          <span class="chev">&gt;</span>
          <input id="in" autocomplete="off" placeholder="Speak to Jarvis" autofocus/>
          <button id="mic" title="Push to talk">&#9679; Talk</button>
        </div>
      </footer>
    </section>
  </div>
  <div id="approvalWrap"><div class="approval">
    <div class="h">&#9888; Approval required</div>
    <div class="lbl" id="apLbl"></div>
    <div class="warn" id="apWarn"></div>
    <div class="btns">
      <button id="apOnce">Allow once</button>
      <button id="apAlways">Always allow</button>
      <button class="deny" id="apDeny">Deny</button>
    </div>
  </div></div>
<script>
const log=document.getElementById('log'), input=document.getElementById('in'),
      toolEl=document.getElementById('tool'), connEl=document.getElementById('conn'),
      modelEl=document.getElementById('model'), stateEl=document.getElementById('state');
let state='idle';
let cur=null, curRaw=''; // the live answer bubble + its raw accumulated text
function plainify(s){return s.replace(/\*\*/g,'').replace(/__/g,'').replace(/—/g,' - ').replace(/–/g,'-').replace(/^#{1,6}\s*/gm,'').replace(/^\s*[\*\-]\s+/gm,'- ');}
const STATE_LABEL={idle:'STANDBY',thinking:'THINKING',working:'WORKING',speaking:'RESPONDING'};
function setState(s){ state=s; stateEl.textContent=STATE_LABEL[s]||'STANDBY'; }

// ── arc-reactor orb: calm by default, geometry encodes state.
//    amber structure + a single cyan live-ring. Restrained motion.
const cv=document.getElementById('orb'), ctx=cv.getContext('2d');
const cx=130, cy=130; let t=0;
function speed(){return state==='thinking'?1.9:state==='working'?1.4:state==='speaking'?1.0:0.35;}
function amber(a){return 'rgba(255,176,0,'+a+')';}
function cyan(a){return 'rgba(57,211,192,'+a+')';}
function draw(){
  t+=0.016*speed();
  ctx.clearRect(0,0,260,260);
  const active = state!=='idle';
  const breathe = 0.5+0.5*Math.sin(t*(active?2.4:1.0)); // 0..1

  // faint tick ring (the reactor bezel) - static, structural
  const ticks=60;
  for(let i=0;i<ticks;i++){
    const a=i/ticks*Math.PI*2, big=(i%5===0);
    const r0=big?100:104, r1=110;
    ctx.beginPath();
    ctx.moveTo(cx+Math.cos(a)*r0, cy+Math.sin(a)*r0);
    ctx.lineTo(cx+Math.cos(a)*r1, cy+Math.sin(a)*r1);
    ctx.strokeStyle=amber(big?0.22:0.10); ctx.lineWidth=1; ctx.stroke();
  }
  // two thin amber arcs, slow counter-rotation
  const arcs=[{r:84,seg:0.62,dir:1,w:1.5},{r:64,seg:0.4,dir:-1,w:1}];
  arcs.forEach((rg,i)=>{
    const a0=t*rg.dir+i*1.4, a1=a0+Math.PI*2*rg.seg;
    ctx.beginPath(); ctx.arc(cx,cy,rg.r,a0,a1);
    ctx.strokeStyle=amber(0.4+0.35*breathe); ctx.lineWidth=rg.w; ctx.stroke();
  });
  // cyan live-ring: the "alive" signal. dim at idle, present when active.
  const live = active ? 0.55+0.4*breathe : 0.16+0.08*breathe;
  ctx.beginPath(); ctx.arc(cx,cy,46,0,Math.PI*2);
  ctx.strokeStyle=cyan(live); ctx.lineWidth=1.5; ctx.stroke();
  // core aperture (amber), gentle breath
  const cr=15+(active?2.5*breathe:1.2*breathe);
  ctx.beginPath(); ctx.arc(cx,cy,cr,0,Math.PI*2);
  ctx.strokeStyle=amber(0.55+0.4*breathe); ctx.lineWidth=2; ctx.stroke();
  ctx.beginPath(); ctx.arc(cx,cy,cr*0.42,0,Math.PI*2);
  ctx.fillStyle=cyan(active?0.5*breathe:0.12); ctx.fill();
  requestAnimationFrame(draw);
}
draw();

function addMsg(cls, who){
  const e=document.getElementById('empty'); if(e) e.style.display='none';
  const d=document.createElement('div'); d.className='msg '+cls;
  d.innerHTML='<span class="who">'+who+'</span><span class="body"></span>';
  log.appendChild(d); log.scrollTop=log.scrollHeight;
  return d.querySelector('.body');
}
function typewriter(el, text){
  let i=0; setState('speaking');
  (function step(){
    if(i<=text.length){ el.textContent=text.slice(0,i); i+=Math.max(1,Math.round(text.length/220));
      log.scrollTop=log.scrollHeight; setTimeout(step,12);}
    else { setState('idle'); }
  })();
}
function flashTool(name){ toolEl.textContent='◢ '+name; toolEl.style.opacity=1;
  clearTimeout(flashTool._t); flashTool._t=setTimeout(()=>toolEl.style.opacity=0,1800); }

// ── websocket
let ws;
function connect(){
  ws=new WebSocket((location.protocol==='https:'?'wss':'ws')+'://'+location.host+'/ws');
  ws.onopen=()=>{connEl.innerHTML='<span class="dot">●</span> ONLINE';};
  ws.onclose=()=>{connEl.innerHTML='<span class="dot" style="color:var(--red)">●</span> OFFLINE'; setTimeout(connect,1500);};
  ws.onmessage=(e)=>{
    const m=JSON.parse(e.data);
    if(m.type==='hello'){ modelEl.textContent=m.model; }
    else if(m.type==='state'){ if(m.state!=='speaking') setState(m.state); }
    else if(m.type==='tool'){ flashTool(m.name); }
    else if(m.type==='delta'){ if(!cur){cur=addMsg('jarvis','Jarvis');curRaw='';} curRaw+=m.text; cur.textContent=plainify(curRaw); setState('speaking'); log.scrollTop=log.scrollHeight; }
    else if(m.type==='done'){ speak(plainify(curRaw)); cur=null; curRaw=''; setState('idle'); }
    else if(m.type==='answer'){ const txt=plainify(m.text); typewriter(addMsg('jarvis','Jarvis'), txt); speak(txt); }
    else if(m.type==='error'){ addMsg('err','Error').textContent=m.text; cur=null; setState('idle'); }
    else if(m.type==='approval'){ showApproval(m); }
  };
}
connect();

// ── approval modal
const apWrap=document.getElementById('approvalWrap'),
      apLbl=document.getElementById('apLbl'), apWarn=document.getElementById('apWarn');
function showApproval(m){
  apLbl.textContent=m.label||'(action)';
  apWarn.textContent=m.tainted ? 'This turn read web content - approve carefully.' : '';
  apWrap.classList.add('show');
}
function respond(decision){
  apWrap.classList.remove('show');
  ws.send(JSON.stringify({type:'approval_response',decision}));
}
document.getElementById('apOnce').onclick=()=>respond('once');
document.getElementById('apAlways').onclick=()=>respond('always');
document.getElementById('apDeny').onclick=()=>respond('deny');

function sendText(text){
  text=(text||'').trim(); if(!text) return;
  addMsg('user','You').textContent=text;
  ws.send(JSON.stringify({type:'user',text}));
  input.value=''; cur=null; curRaw='';
}
input.addEventListener('keydown',(e)=>{ if(e.key==='Enter') sendText(input.value); });

// ── voice OUT: speak Jarvis's replies with the browser's built-in TTS (no deps)
let voiceOn=false;
const voiceBtn=document.getElementById('voice');
voiceBtn.onclick=()=>{
  voiceOn=!voiceOn;
  voiceBtn.textContent=(voiceOn?'ON':'OFF');
  voiceBtn.classList.toggle('on',voiceOn);
  if(!voiceOn && window.speechSynthesis) speechSynthesis.cancel();
};
function speak(text){
  if(!voiceOn || !window.speechSynthesis || !text) return;
  speechSynthesis.cancel();
  const u=new SpeechSynthesisUtterance(text);
  u.rate=1.04; u.pitch=0.9;
  speechSynthesis.speak(u);
}

// ── voice IN: push-to-talk via the browser Web Speech API (no deps, Chromium)
const micBtn=document.getElementById('mic');
const SR=window.SpeechRecognition||window.webkitSpeechRecognition;
let rec=null, listening=false, finalText='';
if(SR){
  rec=new SR(); rec.lang='en-US'; rec.interimResults=true; rec.continuous=false;
  rec.onstart=()=>{ listening=true; finalText=''; micBtn.classList.add('live'); micBtn.innerHTML='&#9679; Listening'; flashTool('listening'); };
  rec.onresult=(e)=>{
    let interim='';
    for(let i=e.resultIndex;i<e.results.length;i++){
      const t=e.results[i][0].transcript;
      if(e.results[i].isFinal) finalText+=t; else interim+=t;
    }
    input.value=(finalText+interim).trim();
  };
  rec.onerror=()=>{ listening=false; micBtn.classList.remove('live'); micBtn.innerHTML='&#9679; Talk'; };
  rec.onend=()=>{ listening=false; micBtn.classList.remove('live'); micBtn.innerHTML='&#9679; Talk';
    if(input.value.trim()) sendText(input.value); };
  micBtn.onclick=()=>{ if(listening){ try{rec.stop();}catch(_){} } else { try{rec.start();}catch(_){} } };
} else {
  micBtn.title='Voice input needs a Chromium browser (Chrome/Edge)';
  micBtn.onclick=()=>alert('Voice input needs a Chromium browser (Chrome or Edge).');
}
</script>
</body>
</html>"##;
