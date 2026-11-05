//! JSON-RPC over the two MCP transports: a child process speaking on stdin/stdout, or
//! Streamable HTTP (one POST per message, answered with JSON or a server-sent event stream).

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

const SESSION_ID_HEADER: &str = "mcp-session-id";
const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

pub enum Transport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

impl Transport {
    /// Sends a request and waits for its result. A JSON-RPC error answer becomes `Err`.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        match self {
            Transport::Stdio(stdio) => stdio.request(method, params).await,
            Transport::Http(http) => http.request(method, params).await,
        }
    }

    pub async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        match self {
            Transport::Stdio(stdio) => stdio.write_message(&message).await,
            Transport::Http(http) => http.post(&message).await.map(|_| ()),
        }
    }

    /// Sent by the HTTP transport on every request after `initialize`, as the spec asks.
    pub fn set_protocol_version(&self, protocol_version: &str) {
        if let Transport::Http(http) = self {
            *http.protocol_version.lock().unwrap() = Some(protocol_version.to_string());
        }
    }
}

/// The `result` of a JSON-RPC response, or its `error` as a message.
fn response_result(response: &Value) -> Result<Value, String> {
    if let Some(error) = response.get("error") {
        let message = error.get("message").and_then(Value::as_str).unwrap_or("unknown error");
        return Err(format!("{} (code {})", message, error.get("code").unwrap_or(&Value::Null)));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| format!("response without result or error: {}", response))
}

type PendingRequests = HashMap<u64, oneshot::Sender<Result<Value, String>>>;

/// Requests waiting for their answer. Once the server's stdout closes, no answer can come:
/// waiting requests fail and new ones fail right away.
struct StdioPending {
    by_id: PendingRequests,
    closed: bool,
}

pub struct StdioTransport {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    pending: Arc<Mutex<StdioPending>>,
    next_id: AtomicU64,
    /// Kept so the process is killed when the transport is dropped.
    _child: Child,
}

impl StdioTransport {
    /// Starts the server process. Its stderr is dropped: inherited, it would draw over the TUI.
    pub fn spawn(command: &str, args: &[String], env: &BTreeMap<String, String>) -> Result<Self, String> {
        let mut child = Command::new(command)
            .args(args)
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| format!("could not start '{}': {}", command, err))?;
        let stdin = Arc::new(tokio::sync::Mutex::new(child.stdin.take().expect("piped stdin")));
        let stdout = child.stdout.take().expect("piped stdout");
        let pending = Arc::new(Mutex::new(StdioPending { by_id: HashMap::new(), closed: false }));

        tokio::spawn(read_stdout(BufReader::new(stdout), Arc::clone(&pending), Arc::clone(&stdin)));

        Ok(Self { stdin, pending, next_id: AtomicU64::new(1), _child: child })
    }

    async fn write_message(&self, message: &Value) -> Result<(), String> {
        write_line(&self.stdin, message).await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.closed {
                return Err(String::from("server process exited"));
            }
            pending.by_id.insert(id, sender);
        }
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        if let Err(err) = self.write_message(&message).await {
            self.pending.lock().unwrap().by_id.remove(&id);
            return Err(err);
        }
        receiver.await.map_err(|_| String::from("server process exited"))?
    }
}

async fn write_line(stdin: &tokio::sync::Mutex<ChildStdin>, message: &Value) -> Result<(), String> {
    let mut line = message.to_string();
    line.push('\n');
    let mut stdin = stdin.lock().await;
    stdin.write_all(line.as_bytes()).await.map_err(|err| err.to_string())?;
    stdin.flush().await.map_err(|err| err.to_string())
}

/// Routes each line of the server's stdout: answers go to the request waiting for them,
/// requests from the server get an answer, notifications are ignored.
async fn read_stdout(
    stdout: BufReader<tokio::process::ChildStdout>,
    pending: Arc<Mutex<StdioPending>>,
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
) {
    let mut lines = stdout.lines();
    while let Ok(Some(line)) = lines.next_line().await {
        // Servers sometimes log plain text on stdout; only JSON lines are messages.
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message.get("id").cloned();
        match (message.get("method").and_then(Value::as_str), id) {
            (Some(method), Some(id)) => {
                let _ = write_line(&stdin, &answer_server_request(method, id)).await;
            }
            (None, Some(id)) => {
                let Some(id) = id.as_u64() else { continue };
                let sender = pending.lock().unwrap().by_id.remove(&id);
                if let Some(sender) = sender {
                    let _ = sender.send(response_result(&message));
                }
            }
            (_, None) => {}
        }
    }
    let mut pending = pending.lock().unwrap();
    pending.closed = true;
    for (_, sender) in pending.by_id.drain() {
        let _ = sender.send(Err(String::from("server process exited")));
    }
}

/// Minerve declares no client capabilities, so `ping` is the only server request it supports.
fn answer_server_request(method: &str, id: Value) -> Value {
    if method == "ping" {
        json!({ "jsonrpc": "2.0", "id": id, "result": {} })
    } else {
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } })
    }
}

pub struct HttpTransport {
    client: reqwest::Client,
    url: String,
    headers: HeaderMap,
    /// Given by the server in its `initialize` answer, sent back on every later message.
    session_id: Mutex<Option<String>>,
    protocol_version: Mutex<Option<String>>,
    next_id: AtomicU64,
}

impl HttpTransport {
    pub fn new(client: reqwest::Client, url: &str, headers: &BTreeMap<String, String>) -> Result<Self, String> {
        let mut header_map = HeaderMap::new();
        for (name, value) in headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|err| format!("header '{}': {}", name, err))?;
            let value = HeaderValue::from_str(value).map_err(|err| format!("header '{}': {}", name, err))?;
            header_map.insert(name, value);
        }
        Ok(Self {
            client,
            url: url.to_string(),
            headers: header_map,
            session_id: Mutex::new(None),
            protocol_version: Mutex::new(None),
            next_id: AtomicU64::new(1),
        })
    }

    async fn post(&self, message: &Value) -> Result<reqwest::Response, String> {
        let mut request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .json(message);
        if let Some(session_id) = self.session_id.lock().unwrap().as_ref() {
            request = request.header(SESSION_ID_HEADER, session_id);
        }
        if let Some(protocol_version) = self.protocol_version.lock().unwrap().as_ref() {
            request = request.header(PROTOCOL_VERSION_HEADER, protocol_version);
        }
        let response = request.send().await.map_err(|err| err.to_string())?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("{}: {}", status, body));
        }
        if let Some(session_id) = response.headers().get(SESSION_ID_HEADER) {
            let session_id = session_id.to_str().map_err(|err| err.to_string())?;
            *self.session_id.lock().unwrap() = Some(session_id.to_string());
        }
        Ok(response)
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let response = self.post(&message).await?;
        let is_event_stream = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        let body = response.text().await.map_err(|err| err.to_string())?;
        let answer = if is_event_stream {
            find_answer_in_event_stream(&body, id)?
        } else {
            serde_json::from_str(&body).map_err(|err| format!("{}: {}", err, body))?
        };
        response_result(&answer)
    }
}

/// The message answering request `id` among the `data:` events of a server-sent event stream.
fn find_answer_in_event_stream(body: &str, id: u64) -> Result<Value, String> {
    let mut data_lines: Vec<&str> = Vec::new();
    // A trailing blank line ends the last event even when the body does not.
    for line in body.lines().chain(std::iter::once("")) {
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.strip_prefix(' ').unwrap_or(data));
            continue;
        }
        if !line.is_empty() || data_lines.is_empty() {
            continue;
        }
        let data = data_lines.join("\n");
        data_lines.clear();
        let Ok(message) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if message.get("id").and_then(Value::as_u64) == Some(id) && message.get("method").is_none() {
            return Ok(message);
        }
    }
    Err(format!("event stream ended without an answer to request {}", id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_answer_among_events() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\n\
                    event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":3,\"result\":{\"ok\":true}}\n";
        let answer = find_answer_in_event_stream(body, 3).unwrap();
        assert_eq!(response_result(&answer).unwrap(), json!({ "ok": true }));
    }

    #[test]
    fn reports_missing_answer() {
        assert!(find_answer_in_event_stream("data: {\"id\":1,\"result\":{}}\n\n", 2).is_err());
    }

    #[test]
    fn error_answer_becomes_err() {
        let err = response_result(&json!({ "id": 1, "error": { "code": -32602, "message": "bad" } })).unwrap_err();
        assert_eq!(err, "bad (code -32602)");
    }
}
