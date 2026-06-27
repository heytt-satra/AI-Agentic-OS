// ── src/mcp.rs : Model Context Protocol client (gap 5) ──────────────────────
//
// MCP is the emerging open standard (Anthropic + the Agentic AI Foundation) for
// connecting an agent to external TOOL SERVERS. Speaking it means Jarvis can use
// the whole ecosystem of MCP servers (filesystem, git, Slack, search, ...) on
// top of its own built-in tools, instead of a closed, fixed tool set.
//
// This is a minimal stdio JSON-RPC client:
//   - read mcp.json (the same { "mcpServers": { name: {command, args} } } shape
//     Claude Desktop uses),
//   - spawn each server, do the initialize handshake, list its tools,
//   - expose them to the model as tools named mcp__<server>__<tool>,
//   - route those tool calls to the right server via tools/call.
//
// Servers run on a dedicated thread (blocking child-process I/O), reached from
// the async agent loop through a channel - same actor pattern as memory.rs.

use crate::provider::{FunctionDef, Tool};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::OnceLock;
use tokio::sync::{mpsc, oneshot};

enum Cmd {
    Tools { reply: oneshot::Sender<Vec<Tool>> },
    Call { name: String, args: String, reply: oneshot::Sender<String> },
}

#[derive(Clone)]
pub struct McpHandle {
    tx: mpsc::Sender<Cmd>,
}

static HUB: OnceLock<McpHandle> = OnceLock::new();

// The connected MCP hub, if any servers are configured. None when there is no
// mcp.json (the common case), so everything else is a no-op.
pub fn handle() -> Option<&'static McpHandle> {
    HUB.get()
}

impl McpHandle {
    pub async fn tools(&self) -> Vec<Tool> {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Cmd::Tools { reply }).await.is_err() {
            return Vec::new();
        }
        rx.await.unwrap_or_default()
    }

    pub async fn call(&self, name: &str, args: &str) -> String {
        let (reply, rx) = oneshot::channel();
        if self.tx.send(Cmd::Call { name: name.to_string(), args: args.to_string(), reply }).await.is_err() {
            return "ERROR: MCP hub is gone".to_string();
        }
        rx.await.unwrap_or_else(|_| "ERROR: MCP call failed".to_string())
    }
}

struct ServerCfg {
    command: String,
    args: Vec<String>,
}

// Read mcp.json from the working directory. Returns the configured servers.
fn load_config() -> Vec<(String, ServerCfg)> {
    let Ok(text) = std::fs::read_to_string("mcp.json") else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    let Some(map) = v.get("mcpServers").and_then(|m| m.as_object()) else { return Vec::new() };
    let mut out = Vec::new();
    for (name, sc) in map {
        let Some(command) = sc.get("command").and_then(|c| c.as_str()) else { continue };
        let args = sc
            .get("args")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        out.push((name.clone(), ServerCfg { command: command.to_string(), args }));
    }
    out
}

// Start the MCP hub if mcp.json configures any servers. Call once at startup.
pub fn init() {
    let cfg = load_config();
    if cfg.is_empty() {
        return;
    }
    let (tx, mut rx) = mpsc::channel::<Cmd>(32);
    let spawned = std::thread::Builder::new()
        .name("jarvis-mcp".into())
        .spawn(move || {
            let mut servers: Vec<Server> = Vec::new();
            for (name, sc) in cfg {
                match Server::connect(&name, &sc) {
                    Ok(s) => {
                        eprintln!("[mcp] connected '{}' ({} tools)", s.name, s.tools.len());
                        servers.push(s);
                    }
                    Err(e) => eprintln!("[mcp] server '{name}' failed: {e}"),
                }
            }
            while let Some(cmd) = rx.blocking_recv() {
                match cmd {
                    Cmd::Tools { reply } => {
                        let mut all = Vec::new();
                        for s in &servers {
                            all.extend(s.tool_defs());
                        }
                        let _ = reply.send(all);
                    }
                    Cmd::Call { name, args, reply } => {
                        let _ = reply.send(route_call(&mut servers, &name, &args));
                    }
                }
            }
        });
    if spawned.is_ok() {
        let _ = HUB.set(McpHandle { tx });
    }
}

fn route_call(servers: &mut [Server], full_name: &str, args: &str) -> String {
    // full_name = mcp__<server>__<tool>
    let rest = full_name.strip_prefix("mcp__").unwrap_or(full_name);
    let mut parts = rest.splitn(2, "__");
    let server = parts.next().unwrap_or("");
    let tool = parts.next().unwrap_or("");
    for s in servers.iter_mut() {
        if s.name == server {
            return s.call(tool, args);
        }
    }
    format!("ERROR: no MCP server named '{server}'")
}

struct McpTool {
    name: String,
    description: String,
    schema: Value,
}

struct Server {
    name: String, // sanitized (alphanumeric) for the tool-name prefix
    _child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: i64,
    tools: Vec<McpTool>,
}

impl Server {
    fn connect(raw_name: &str, sc: &ServerCfg) -> Result<Server, String> {
        let name: String = raw_name.chars().filter(|c| c.is_alphanumeric()).collect();
        // On Windows, npx/npm are .cmd/.ps1 shims, so go through cmd /c.
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/c").arg(&sc.command).args(&sc.args);
            c
        } else {
            let mut c = Command::new(&sc.command);
            c.args(&sc.args);
            c
        };
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let mut s = Server {
            name,
            _child: child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 0,
            tools: Vec::new(),
        };
        s.initialize()?;
        s.tools = s.list_tools()?;
        Ok(s)
    }

    fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        writeln!(self.stdin, "{req}").map_err(|e| format!("write: {e}"))?;
        self.stdin.flush().ok();
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).map_err(|e| format!("read: {e}"))?;
            if n == 0 {
                return Err("server closed the connection".to_string());
            }
            let Ok(v) = serde_json::from_str::<Value>(line.trim()) else { continue };
            if v.get("id").and_then(|x| x.as_i64()) == Some(id) {
                if let Some(err) = v.get("error") {
                    return Err(err.to_string());
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
            // otherwise a notification/log line - ignore and keep reading.
        }
    }

    fn notify(&mut self, method: &str) {
        let n = json!({"jsonrpc":"2.0","method":method});
        let _ = writeln!(self.stdin, "{n}");
        let _ = self.stdin.flush();
    }

    fn initialize(&mut self) -> Result<(), String> {
        self.rpc(
            "initialize",
            json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"jarvis","version":"0.1"}}),
        )?;
        self.notify("notifications/initialized");
        Ok(())
    }

    fn list_tools(&mut self) -> Result<Vec<McpTool>, String> {
        let res = self.rpc("tools/list", json!({}))?;
        let arr = res.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        Ok(arr
            .iter()
            .map(|t| McpTool {
                name: t.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                description: t.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type":"object"})),
            })
            .filter(|t| !t.name.is_empty())
            .collect())
    }

    fn tool_defs(&self) -> Vec<Tool> {
        self.tools
            .iter()
            .map(|t| Tool {
                kind: "function".to_string(),
                function: FunctionDef {
                    name: format!("mcp__{}__{}", self.name, t.name),
                    description: t.description.clone(),
                    parameters: t.schema.clone(),
                },
            })
            .collect()
    }

    fn call(&mut self, tool: &str, args: &str) -> String {
        let arguments: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
        match self.rpc("tools/call", json!({"name":tool,"arguments":arguments})) {
            Ok(res) => {
                if let Some(content) = res.get("content").and_then(|c| c.as_array()) {
                    let text: String = content
                        .iter()
                        .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        return text.chars().take(6000).collect();
                    }
                }
                res.to_string().chars().take(6000).collect()
            }
            Err(e) => format!("ERROR (mcp {tool}): {e}"),
        }
    }
}
