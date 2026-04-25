//! Minimal MCP-over-stdio JSON-RPC client.
//!
//! Davis spawns the MemPalace Python MCP server as a child process and
//! exchanges line-delimited JSON-RPC 2.0 messages over its stdio. This
//! module is deliberately narrow — it supports `initialize` and
//! `tools/call`, which is all the sink needs. Streaming, notifications,
//! and server→client requests are unused here.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// Hard cap on outstanding in-flight requests. The sink's bounded mpsc
/// upstream already limits queue depth; this is a belt-and-braces cap.
const MAX_INFLIGHT: usize = 256;

/// Default timeout for a single RPC. Generous because embedding loads and
/// ChromaDB writes can take a second or two on first call.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize)]
pub struct InitializeParams {
    pub client_name: String,
    pub client_version: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct InitializeResult {
    #[serde(default, rename = "protocolVersion")]
    pub protocol_version: Option<String>,
    #[serde(default, rename = "serverInfo")]
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ServerInfo {
    pub name: Option<String>,
    pub version: Option<String>,
}

type PendingMap = Mutex<HashMap<u64, oneshot::Sender<RpcOutcome>>>;

#[derive(Debug)]
enum RpcOutcome {
    Ok(Value),
    Err(String),
}

/// Long-running MCP-over-stdio client. Dropping an `McpStdioClient`
/// aborts the reader task and kills the child process.
pub struct McpStdioClient {
    stdin: Mutex<ChildStdin>,
    pending: Arc<PendingMap>,
    next_id: AtomicU64,
    child: Mutex<Option<Child>>,
    reader_task: Mutex<Option<JoinHandle<()>>>,
}

impl McpStdioClient {
    /// Spawn a child process given a fully-constructed `tokio::process::Command`.
    /// The command's stdin/stdout/stderr are reconfigured to piped by this call.
    pub async fn spawn(mut command: Command) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().context("spawn MCP stdio child")?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("child stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("child stdout was not piped"))?;
        let stderr = child.stderr.take();

        let pending: Arc<PendingMap> = Arc::new(Mutex::new(HashMap::new()));
        let reader_task = spawn_reader_task(BufReader::new(stdout), Arc::clone(&pending));
        if let Some(err_stream) = stderr {
            spawn_stderr_task(BufReader::new(err_stream));
        }

        Ok(Self {
            stdin: Mutex::new(stdin),
            pending,
            next_id: AtomicU64::new(1),
            child: Mutex::new(Some(child)),
            reader_task: Mutex::new(Some(reader_task)),
        })
    }

    /// Perform the MCP `initialize` handshake.
    pub async fn initialize(&self, params: &InitializeParams) -> Result<InitializeResult> {
        let body = json!({
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {
                "name": params.client_name,
                "version": params.client_version,
            }
        });
        let result = self.request("initialize", body).await?;
        // Also send the `notifications/initialized` notification as required by
        // spec — best effort, ignore failures.
        let _ = self.notify("notifications/initialized", json!({})).await;
        serde_json::from_value(result).context("parse initialize result")
    }

    /// Call a tool via `tools/call`. Returns the raw result value.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value> {
        self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
        .await
    }

    /// Issue a single JSON-RPC request and await its matched response.
    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        {
            let pending = self.pending.lock().await;
            if pending.len() >= MAX_INFLIGHT {
                bail!("MCP stdio client: too many in-flight requests");
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&payload).context("serialize request")?;
        line.push('\n');
        {
            let mut stdin = self.stdin.lock().await;
            if let Err(err) = stdin.write_all(line.as_bytes()).await {
                self.pending.lock().await.remove(&id);
                return Err(err).context("write request to MCP stdio");
            }
            if let Err(err) = stdin.flush().await {
                self.pending.lock().await.remove(&id);
                return Err(err).context("flush MCP stdio");
            }
        }

        match timeout(DEFAULT_RPC_TIMEOUT, rx).await {
            Ok(Ok(RpcOutcome::Ok(value))) => Ok(value),
            Ok(Ok(RpcOutcome::Err(msg))) => Err(anyhow!("MCP error: {msg}")),
            Ok(Err(_)) => Err(anyhow!("MCP reader task dropped response")),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(anyhow!(
                    "MCP request timed out after {:?}",
                    DEFAULT_RPC_TIMEOUT
                ))
            }
        }
    }

    /// Issue a JSON-RPC notification (no response expected).
    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&payload).context("serialize notify")?;
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Graceful shutdown: close stdin, await reader task, kill child if alive.
    pub async fn shutdown(&self) {
        // Drop stdin to signal EOF to the child.
        {
            let mut stdin = self.stdin.lock().await;
            let _ = stdin.shutdown().await;
        }
        if let Some(task) = self.reader_task.lock().await.take() {
            let _ = timeout(Duration::from_secs(2), task).await;
        }
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(2), child.wait()).await;
        }
    }
}

fn spawn_reader_task(
    mut stdout: BufReader<tokio::process::ChildStdout>,
    pending: Arc<PendingMap>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match stdout.read_line(&mut line).await {
                Ok(0) => break, // EOF
                Ok(_) => {}
                Err(_) => break,
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            // Skip notifications and server→client requests (no id correlation).
            let Some(id) = value.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let outcome = if let Some(err) = value.get("error") {
                let msg = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
                    .to_string();
                RpcOutcome::Err(msg)
            } else {
                RpcOutcome::Ok(value.get("result").cloned().unwrap_or(Value::Null))
            };
            if let Some(tx) = pending.lock().await.remove(&id) {
                let _ = tx.send(outcome);
            }
        }
        // On EOF/error, fail every outstanding request.
        let mut pending = pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(RpcOutcome::Err("stdio reader closed".to_string()));
        }
    })
}

fn spawn_stderr_task(mut stderr: BufReader<tokio::process::ChildStderr>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut line = String::new();
        loop {
            line.clear();
            match stderr.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim_end();
                    if !trimmed.is_empty() {
                        tracing::warn!(target: "mempalace_sink", child_stderr = trimmed);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal echo server: reads JSON-RPC requests line-by-line and emits
    /// `{"jsonrpc":"2.0","id":<id>,"result":{"echo":<method>,"params":<params>}}`.
    /// For `initialize`, it returns a canned `serverInfo` so the handshake
    /// test has something to assert.
    fn fake_mcp_echo_command() -> Command {
        let script = r#"
import json
import sys

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    if "id" not in msg:
        continue
    method = msg.get("method", "")
    params = msg.get("params", {})
    if method == "initialize":
        result = {
            "protocolVersion": "2025-03-26",
            "serverInfo": {"name": "mempalace-mock", "version": "0.0.1"},
        }
    elif method == "tools/call":
        tool_name = params.get("name", "")
        if tool_name == "slow":
            # Intentionally never reply — used to test reader fail-on-shutdown.
            continue
        result = {"echo": method, "arguments": params.get("arguments", {})}
    else:
        result = {"echo": method, "params": params}
    response = {"jsonrpc": "2.0", "id": msg["id"], "result": result}
    sys.stdout.write(json.dumps(response) + "\n")
    sys.stdout.flush()
"#;
        let mut cmd = Command::new("python3");
        cmd.arg("-c").arg(script);
        cmd
    }

    #[tokio::test]
    async fn initialize_exchanges_handshake_over_stdio() {
        let client = McpStdioClient::spawn(fake_mcp_echo_command())
            .await
            .expect("spawn echo");
        let info = client
            .initialize(&InitializeParams {
                client_name: "davis".into(),
                client_version: "test".into(),
            })
            .await
            .expect("initialize");
        assert_eq!(info.protocol_version.as_deref(), Some("2025-03-26"));
        let server = info.server_info.expect("server info present");
        assert_eq!(server.name.as_deref(), Some("mempalace-mock"));
        assert_eq!(server.version.as_deref(), Some("0.0.1"));
        client.shutdown().await;
    }

    #[tokio::test]
    async fn call_tool_returns_arguments_via_echo() {
        let client = McpStdioClient::spawn(fake_mcp_echo_command())
            .await
            .expect("spawn echo");
        let _ = client
            .initialize(&InitializeParams {
                client_name: "davis".into(),
                client_version: "test".into(),
            })
            .await
            .unwrap();
        let result = client
            .call_tool("mempalace_add_drawer", json!({"wing": "davis:test"}))
            .await
            .expect("tool call");
        assert_eq!(result["arguments"]["wing"], "davis:test");
        client.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_calls_correlate_by_id() {
        let client = Arc::new(
            McpStdioClient::spawn(fake_mcp_echo_command())
                .await
                .expect("spawn echo"),
        );
        let _ = client
            .initialize(&InitializeParams {
                client_name: "davis".into(),
                client_version: "test".into(),
            })
            .await
            .unwrap();
        let mut handles = Vec::new();
        for i in 0..10 {
            let c = Arc::clone(&client);
            handles.push(tokio::spawn(async move {
                c.call_tool("m", json!({"n": i})).await
            }));
        }
        for (i, h) in handles.into_iter().enumerate() {
            let r = h.await.unwrap().expect("call ok");
            assert_eq!(r["arguments"]["n"], i as i64);
        }
        client.shutdown().await;
    }

    #[tokio::test]
    async fn timed_out_request_returns_error() {
        // Use a shorter timeout by calling a method the echo server ignores,
        // and cap the test wait with tokio::time::timeout on the outer future.
        let client = McpStdioClient::spawn(fake_mcp_echo_command())
            .await
            .expect("spawn echo");
        // Expect the default 10s RPC timeout — we don't want to wait that long
        // in a unit test, so we rely on the reader fail-all behavior by shutting
        // down the child mid-flight instead.
        let call = tokio::spawn({
            let fut = async move {
                let _ = client
                    .initialize(&InitializeParams {
                        client_name: "davis".into(),
                        client_version: "t".into(),
                    })
                    .await;
                let r = client.call_tool("slow", json!({})).await;
                client.shutdown().await;
                r
            };
            fut
        });
        // Give the call a moment to register.
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Wait for the outer task to return — when shutdown hits it will
        // close stdin/kill child; the reader drains pending with Err.
        let r = timeout(Duration::from_secs(15), call)
            .await
            .expect("outer timeout")
            .expect("join");
        assert!(r.is_err(), "slow call should fail, got {r:?}");
    }
}
