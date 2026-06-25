// ── src/server.rs : the Jarvis web HUD (axum HTTP + WebSocket) ──────────────
//
// `jarvis serve` starts a local server and opens a futuristic browser UI.
// The browser talks to the Rust core over a WebSocket: it sends your text,
// the server runs the agent turn and streams back state + tool + answer events.
//
// Why a browser instead of a native window: it runs on ANY OS with zero extra
// install, and the binary serves the whole UI itself (HTML is embedded).

use crate::memory::MemoryHandle;
use crate::provider::{Message, Provider};
use crate::tools;
use anyhow::Result;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{Html, Response};
use axum::routing::get;
use axum::Router;

const MAX_STEPS: u32 = 8;

#[derive(Clone)]
struct AppState {
    provider: Provider,
    mem: MemoryHandle,
}

pub async fn serve(provider: Provider, mem: MemoryHandle) -> Result<()> {
    let state = AppState { provider, mem };
    let app = Router::new()
        .route("/", get(index))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr = "127.0.0.1:7878";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let url = format!("http://{addr}");
    println!("\n  Jarvis HUD online -> {url}\n  (opening your browser; Ctrl-C to stop)\n");
    open_browser(&url);
    axum::serve(listener, app).await?;
    Ok(())
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

    let mut messages = vec![Message::system(crate::PERSONA)];

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
        messages.push(Message::user(&user_text));
        st.mem.log("user", &user_text).await;

        let _ = send(&mut socket, serde_json::json!({"type":"state","state":"thinking"})).await;

        // In the web UI we don't offer run_shell (its approval gate is a console
        // prompt). Approval-over-WebSocket is a follow-up.
        let web_tools: Vec<_> = tools::definitions()
            .into_iter()
            .filter(|t| t.function.name != "run_shell")
            .collect();

        let mut answered = false;
        for _ in 0..MAX_STEPS {
            let reply = match st.provider.chat(&messages, Some(web_tools.clone())).await {
                Ok(r) => r,
                Err(e) => {
                    let _ = send(&mut socket, serde_json::json!({"type":"error","text":format!("{e}")})).await;
                    break;
                }
            };
            messages.push(reply.message.clone());

            if reply.finish_reason == "tool_calls" {
                for call in reply.message.tool_calls.clone().unwrap_or_default() {
                    let _ = send(&mut socket, serde_json::json!({"type":"tool","name":call.function.name})).await;
                    let _ = send(&mut socket, serde_json::json!({"type":"state","state":"working"})).await;
                    let outcome = tools::execute(&call.function.name, &call.function.arguments).await;
                    st.mem
                        .log_audit(&call.function.name, &call.function.arguments, &outcome.decision, outcome.ok)
                        .await;
                    st.mem.log("tool", &outcome.result).await;
                    messages.push(Message::tool_result(call.id, outcome.result));
                }
                continue;
            }

            let answer = reply.message.content.unwrap_or_else(|| "(no answer)".into());
            st.mem.log("assistant", &answer).await;
            let _ = send(&mut socket, serde_json::json!({"type":"answer","text":answer})).await;
            answered = true;
            break;
        }
        if !answered {
            let _ = send(&mut socket, serde_json::json!({"type":"error","text":"hit step limit"})).await;
        }
        let _ = send(&mut socket, serde_json::json!({"type":"state","state":"idle"})).await;
        crate::trim_messages(&mut messages, 16);
    }
}

async fn send(socket: &mut WebSocket, v: serde_json::Value) -> Result<()> {
    socket.send(WsMessage::Text(v.to_string().into())).await?;
    Ok(())
}

fn open_browser(url: &str) {
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
  :root{
    --bg:#05080b; --panel:#0a0f14; --amber:#ffb000; --amber-dim:#a9791f;
    --ink:#cfd8df; --dim:#5b6b76; --line:rgba(255,176,0,.16);
    --mono:'SF Mono','JetBrains Mono','Cascadia Code',Consolas,ui-monospace,monospace;
  }
  *{box-sizing:border-box}
  html,body{height:100%;margin:0}
  body{
    background:
      repeating-linear-gradient(0deg, transparent 0 38px, rgba(255,176,0,.025) 38px 39px),
      repeating-linear-gradient(90deg, transparent 0 38px, rgba(255,176,0,.025) 38px 39px),
      var(--bg);
    color:var(--ink); font-family:var(--mono); display:flex; flex-direction:column;
    overflow:hidden;
  }
  header{
    display:flex; align-items:center; justify-content:space-between;
    padding:14px 22px; border-bottom:1px solid var(--line); letter-spacing:.5em;
    font-size:13px; color:var(--amber); text-transform:uppercase;
  }
  header .stat{letter-spacing:.08em; font-size:11px; color:var(--dim); display:flex; gap:18px}
  header .dot{color:var(--amber)}
  main{flex:1; display:flex; flex-direction:column; align-items:center; min-height:0}
  #orbWrap{padding:18px 0 6px; position:relative}
  canvas{display:block}
  #tool{height:16px; font-size:11px; letter-spacing:.18em; color:var(--amber-dim);
    text-transform:uppercase; opacity:0; transition:opacity .25s}
  #log{
    width:min(880px,92vw); flex:1; overflow-y:auto; padding:10px 4px 8px;
    display:flex; flex-direction:column; gap:14px; scrollbar-width:thin;
  }
  .msg{font-size:14px; line-height:1.55; white-space:pre-wrap; word-wrap:break-word}
  .msg .who{font-size:10px; letter-spacing:.2em; text-transform:uppercase; color:var(--dim); display:block; margin-bottom:3px}
  .user .who{color:var(--dim)}
  .user .body{color:var(--ink)}
  .jarvis .who{color:var(--amber)}
  .jarvis .body{color:#ffd98a}
  .err .body{color:#ff6b6b}
  footer{width:min(880px,92vw); padding:12px 0 22px}
  .inbar{
    display:flex; align-items:center; gap:10px; border:1px solid var(--line);
    background:var(--panel); padding:12px 14px; transition:border-color .2s, box-shadow .2s;
  }
  .inbar:focus-within{border-color:var(--amber); box-shadow:0 0 0 1px rgba(255,176,0,.25)}
  .inbar .chev{color:var(--amber)}
  input{
    flex:1; background:transparent; border:0; outline:0; color:var(--ink);
    font-family:var(--mono); font-size:14px; caret-color:var(--amber);
  }
  input::placeholder{color:var(--dim)}
</style>
</head>
<body>
  <header>
    <span>J&nbsp;A&nbsp;R&nbsp;V&nbsp;I&nbsp;S</span>
    <span class="stat">
      <span id="model">model: …</span>
      <span id="conn"><span class="dot">●</span> connecting</span>
    </span>
  </header>
  <main>
    <div id="orbWrap"><canvas id="orb" width="260" height="260"></canvas></div>
    <div id="tool"></div>
    <div id="log"></div>
    <footer>
      <div class="inbar">
        <span class="chev">&gt;</span>
        <input id="in" autocomplete="off" placeholder="Speak to Jarvis…" autofocus/>
      </div>
    </footer>
  </main>
<script>
const log=document.getElementById('log'), input=document.getElementById('in'),
      toolEl=document.getElementById('tool'), connEl=document.getElementById('conn'),
      modelEl=document.getElementById('model');
let state='idle';

// ── reactive orb: thin amber arcs, geometry encodes state (no glowing sphere)
const cv=document.getElementById('orb'), ctx=cv.getContext('2d');
const cx=130, cy=130; let t=0;
function speed(){return state==='thinking'?3.2:state==='working'?2.2:state==='speaking'?1.4:0.5;}
function draw(){
  t+=0.016*speed();
  ctx.clearRect(0,0,260,260);
  const rings=[ {r:46,seg:0.7,dir:1,w:2}, {r:66,seg:0.45,dir:-1,w:1.5},
                {r:88,seg:0.6,dir:1,w:1}, {r:108,seg:0.3,dir:-1,w:1} ];
  const pulse = state==='idle' ? 0.6+0.18*Math.sin(t*1.6)
              : state==='speaking' ? 0.7+0.3*Math.abs(Math.sin(t*5))
              : 0.85;
  rings.forEach((rg,i)=>{
    const a0=t*rg.dir + i*1.1, a1=a0+Math.PI*2*rg.seg;
    ctx.beginPath(); ctx.arc(cx,cy,rg.r,a0,a1);
    ctx.strokeStyle='rgba(255,176,0,'+(0.25+0.6*pulse*(1-i*0.18))+')';
    ctx.lineWidth=rg.w; ctx.stroke();
  });
  // core aperture
  ctx.beginPath(); ctx.arc(cx,cy,14+ (state!=='idle'?2*Math.sin(t*6):0),0,Math.PI*2);
  ctx.strokeStyle='rgba(255,176,0,'+(0.5+0.5*pulse)+')'; ctx.lineWidth=2; ctx.stroke();
  requestAnimationFrame(draw);
}
draw();

function addMsg(cls, who){
  const d=document.createElement('div'); d.className='msg '+cls;
  d.innerHTML='<span class="who">'+who+'</span><span class="body"></span>';
  log.appendChild(d); log.scrollTop=log.scrollHeight;
  return d.querySelector('.body');
}
function typewriter(el, text){
  let i=0; state='speaking';
  (function step(){
    if(i<=text.length){ el.textContent=text.slice(0,i); i+=Math.max(1,Math.round(text.length/220));
      log.scrollTop=log.scrollHeight; setTimeout(step,12);}
    else { state='idle'; }
  })();
}
function flashTool(name){ toolEl.textContent='◢ '+name; toolEl.style.opacity=1;
  clearTimeout(flashTool._t); flashTool._t=setTimeout(()=>toolEl.style.opacity=0,1600); }

// ── websocket
let ws;
function connect(){
  ws=new WebSocket((location.protocol==='https:'?'wss':'ws')+'://'+location.host+'/ws');
  ws.onopen=()=>{connEl.innerHTML='<span class="dot">●</span> online';};
  ws.onclose=()=>{connEl.innerHTML='<span class="dot" style="color:#ff6b6b">●</span> offline'; setTimeout(connect,1500);};
  ws.onmessage=(e)=>{
    const m=JSON.parse(e.data);
    if(m.type==='hello'){ modelEl.textContent='model: '+m.model; }
    else if(m.type==='state'){ if(m.state!=='speaking') state=m.state; }
    else if(m.type==='tool'){ flashTool(m.name); }
    else if(m.type==='answer'){ typewriter(addMsg('jarvis','Jarvis'), m.text); }
    else if(m.type==='error'){ addMsg('err','Error').textContent=m.text; state='idle'; }
  };
}
connect();

input.addEventListener('keydown',(e)=>{
  if(e.key==='Enter' && input.value.trim()){
    const text=input.value.trim();
    addMsg('user','You').textContent=text;
    ws.send(JSON.stringify({type:'user',text}));
    input.value='';
  }
});
</script>
</body>
</html>"##;
