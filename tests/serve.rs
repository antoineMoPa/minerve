//! `minerve --serve` against a fake parent on a Unix socket.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn read_event(reader: &mut impl BufRead) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read from minerve");
    assert!(!line.is_empty(), "minerve closed the socket");
    serde_json::from_str(&line).unwrap()
}

/// Reads events until one has `type`; fails after 20 events.
fn wait_for(reader: &mut impl BufRead, kind: &str) -> serde_json::Value {
    for _ in 0..20 {
        let event = read_event(reader);
        if event["type"] == kind {
            return event;
        }
    }
    panic!("no {} event", kind);
}

#[test]
fn serve_talks_over_the_control_socket() {
    let dir = std::env::temp_dir().join(format!("minerve-serve-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket_path = dir.join("s.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_minerve"))
        .arg("--serve")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", &dir)
        .env("MINERVE_CONTROL_SOCKET", &socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);

    let hello = read_event(&mut reader);
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["version"], 1);
    assert!(hello["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|feature| feature == "intro"));
    let first_session = wait_for(&mut reader, "session")["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(wait_for(&mut reader, "state")["state"], "waiting_for_input");
    assert!(wait_for(&mut reader, "transcript")["messages"].is_array());
    assert!(wait_for(&mut reader, "usage")["summary"].is_string());

    writer.write_all(b"{\"type\":\"get_intro\"}\n").unwrap();
    let intro = wait_for(&mut reader, "intro");
    assert_eq!(intro["fps"], 24);
    assert!(intro["frames"].as_array().unwrap().len() >= 10);
    assert!(!intro["tips"].as_array().unwrap().is_empty());

    writer.write_all(b"{\"type\":\"list_models\"}\n").unwrap();
    let models = wait_for(&mut reader, "models");
    assert!(models["current"].is_string());
    assert!(models["lists"].is_array());
    writer.write_all(b"{\"type\":\"list_mcp\"}\n").unwrap();
    assert!(wait_for(&mut reader, "mcp_servers")["servers"].is_array());
    writer
        .write_all(b"{\"type\":\"mcp_remove\",\"name\":\"nope\"}\n")
        .unwrap();
    assert!(wait_for(&mut reader, "notice")["notice"]
        .as_str()
        .unwrap()
        .contains("no MCP server"));
    writer
        .write_all(b"{\"type\":\"message_apps_status\"}\n")
        .unwrap();
    let apps = wait_for(&mut reader, "message_apps");
    assert_eq!(apps["slack"]["state"], "not_set_up");
    writer.write_all(b"{\"type\":\"list_sessions\"}\n").unwrap();
    assert!(wait_for(&mut reader, "sessions")["sessions"].is_array());
    writer.write_all(b"{\"type\":\"get_api_keys\"}\n").unwrap();
    let keys = wait_for(&mut reader, "api_keys");
    assert!(
        keys["openrouter"].is_boolean()
            && keys["openai"].is_boolean()
            && keys["anthropic"].is_boolean()
    );

    writer.write_all(b"{\"type\":\"new_session\"}\n").unwrap();
    let second_session = wait_for(&mut reader, "session")["session_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(first_session, second_session);

    drop(writer);
    drop(reader);
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("minerve --serve did not exit after the parent closed the socket");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn serve_exits_and_saves_nothing_after_a_bad_command() {
    let dir = std::env::temp_dir().join(format!("minerve-serve-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket_path = dir.join("s.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_minerve"))
        .arg("--serve")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", &dir)
        .env("MINERVE_CONTROL_SOCKET", &socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");

    // The parent stays connected: only the bad line should end minerve.
    writer.write_all(b"{\"type\":\"bogus\"}\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("minerve --serve kept running after a bad command");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let sessions = dir.join(".minerve/sessions");
    let saved = std::fs::read_dir(&sessions)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        saved, 0,
        "a chat with no messages must not leave a session file"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reads events until a notice contains `text`.
fn wait_for_notice(reader: &mut impl BufRead, text: &str) {
    for _ in 0..20 {
        let event = read_event(reader);
        if event["type"] == "notice"
            && event["notice"]
                .as_str()
                .is_some_and(|notice| notice.contains(text))
        {
            return;
        }
    }
    panic!("no notice containing {text:?}");
}

fn start(
    args: &[&str],
    dir: &std::path::Path,
) -> (std::process::Child, std::os::unix::net::UnixStream) {
    std::fs::create_dir_all(dir).unwrap();
    let socket_path = dir.join("s.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_minerve"))
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", dir)
        .env("MINERVE_CONTROL_SOCKET", &socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    (child, stream)
}

/// What moon meets: the terminal app's `hello` offers no optional commands.
#[test]
fn the_terminal_app_advertises_no_features() {
    let dir = std::env::temp_dir().join(format!("minerve-tui-hello-{}", std::process::id()));
    let (mut child, stream) = start(&[], &dir);
    let hello = read_event(&mut BufReader::new(stream));
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["version"], 1);
    assert_eq!(hello["features"], serde_json::json!([]));
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn message_apps_refuses_bad_input_without_touching_slack() {
    let dir = std::env::temp_dir().join(format!("minerve-apps-bad-{}", std::process::id()));
    let (mut child, stream) = start(&["--serve"], &dir);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");

    writer
        .write_all(b"{\"type\":\"message_apps_save\",\"bot_name\":\"Bad Name\"}\n")
        .unwrap();
    wait_for_notice(&mut reader, "Bot name");
    writer
        .write_all(b"{\"type\":\"message_apps_save\",\"bot_name\":\"ok-bot\",\"app_token\":\"nope\",\"bot_token\":\"xoxb-1\"}\n")
        .unwrap();
    wait_for_notice(&mut reader, "xapp-");
    writer
        .write_all(b"{\"type\":\"message_apps_link\"}\n")
        .unwrap();
    wait_for_notice(&mut reader, "Not running.");
    assert_eq!(
        wait_for(&mut reader, "message_apps")["slack"]["state"],
        "not_set_up"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A server that takes connections and never answers, so a turn stays running.
fn hanging_server() -> (
    String,
    std::thread::JoinHandle<()>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::{AtomicBool, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let handle = std::thread::spawn(move || {
        let mut held = Vec::new();
        while !flag.load(Ordering::SeqCst) {
            if let Ok((stream, _)) = listener.accept() {
                held.push(stream);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    (url, handle, stop)
}

fn start_with_model_at(
    dir: &std::path::Path,
    url: &str,
) -> (std::process::Child, std::os::unix::net::UnixStream) {
    std::fs::create_dir_all(dir).unwrap();
    let socket_path = dir.join("s.sock");
    let listener = UnixListener::bind(&socket_path).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_minerve"))
        .arg("--serve")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", dir)
        .env("OPENROUTER_API_KEY", "sk-test")
        .env("MINERVE_OPENROUTER_BASE_URL", url)
        .env("MINERVE_CONTROL_SOCKET", &socket_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    (child, stream)
}

/// Submits a prompt, then waits until the turn has ended by itself and the chat shows every
/// text of `expected`. The last transcript can come after the state: they take different paths.
fn submit_and_wait_for_ended_turn_showing(
    writer: &mut impl Write,
    reader: &mut impl BufRead,
    expected: &[&str],
) {
    writer
        .write_all(b"{\"type\":\"submit\",\"text\":\"hello there\"}\n")
        .unwrap();
    assert_eq!(wait_for(reader, "state")["state"], "working");
    // The chat and the notices shown with it: a failed request is told as a notice.
    let mut chat = String::new();
    let mut notices = String::new();
    let mut ended = false;
    while !(ended
        && expected
            .iter()
            .all(|text| chat.contains(text) || notices.contains(text)))
    {
        let event = read_event(reader);
        match event["type"].as_str() {
            Some("transcript") => chat = event["messages"].to_string(),
            Some("notice") => notices.push_str(event["notice"].as_str().unwrap_or_default()),
            Some("state") => {
                assert_eq!(event["state"], "waiting_for_input");
                ended = true;
            }
            _ => {}
        }
    }
}

/// The headers of a request and all of its body: a server that answers before it has read
/// everything resets the connection under the client.
fn read_whole_http_request(stream: &mut std::net::TcpStream) -> String {
    use std::io::Read;
    let mut request = Vec::new();
    let mut buffer = [0u8; 65536];
    while let Ok(read) = stream.read(&mut buffer) {
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let text = String::from_utf8_lossy(&request);
        let Some(headers_end) = text.find("\r\n\r\n") else {
            continue;
        };
        let body_length = text[..headers_end]
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        if request.len() >= headers_end + 4 + body_length {
            break;
        }
    }
    String::from_utf8_lossy(&request).to_string()
}

/// A server that answers every request with `status_line` (like "200 OK") and `body`.
fn start_server_answering(status_line: &'static str, body: &'static str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            read_whole_http_request(&mut stream);
            let _ = write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status_line,
                body.len(),
                body
            );
        }
    });
    url
}

fn build_shell_tool_call_answer(id: &str, command: &str) -> String {
    serde_json::json!({"choices": [{"message": {
        "role": "assistant",
        "content": null,
        "tool_calls": [{"id": id, "type": "function", "function": {
            "name": "run_shell_command",
            "arguments": serde_json::json!({ "command": command }).to_string(),
        }}],
    }}]})
    .to_string()
}

/// A model that asks for `command` in the shell, reads its result, asks for `echo recovered`,
/// then answers "all done". The requests it got are kept, to read what the results looked like.
fn start_model_running_shell_command(
    command: &str,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let first_call = build_shell_tool_call_answer("call_1", command);
    let second_call = build_shell_tool_call_answer("call_2", "echo recovered");
    let done = r#"{"choices":[{"message":{"role":"assistant","content":"all done"}}]}"#.to_string();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let request = read_whole_http_request(&mut stream);
            let body = match request.matches("\"role\":\"tool\"").count() {
                0 => &first_call,
                1 => &second_call,
                _ => &done,
            };
            seen.lock().unwrap().push(request);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });
    (url, requests)
}

#[test]
fn a_shell_command_that_fails_is_told_to_the_model_which_keeps_working() {
    // Each command, with what the model must read of its failure.
    let commands = [
        // Unterminated substitute: sed fails at once.
        ("sed 's/a/b' /etc/hosts", "[Error] sed"),
        // No file: sed reads stdin, which must not be a terminal or an open pipe.
        ("sed 's/a/b/'", ""),
        ("echo oops >&2; exit 3", "[Error] oops"),
        // Not ASCII, in the command and in sed's error.
        ("sed 's/é→/e' /etc/hosts", "[Error] sed"),
        // The pipeline succeeds with no output: the result is empty.
        ("sed 's/a/b' /etc/hosts | head -1", ""),
        // Fails with nothing on stderr.
        ("false", "[Error]"),
    ];
    for (index, (command, failure)) in commands.iter().enumerate() {
        let (url, requests) = start_model_running_shell_command(command);
        let dir = std::env::temp_dir().join(format!(
            "minerve-serve-badshell-{index}-{}",
            std::process::id()
        ));
        let (mut child, stream) = start_with_model_at(&dir, &url);
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        wait_for(&mut reader, "usage");

        submit_and_wait_for_ended_turn_showing(&mut writer, &mut reader, &["all done"]);
        let requests = requests.lock().unwrap();
        let posts: Vec<&String> = requests
            .iter()
            .filter(|request| request.starts_with("POST"))
            .collect();
        assert_eq!(posts.len(), 3, "{command}");
        // The model read the failure, then its next command ran.
        assert!(posts[1].contains(failure), "{command}: {}", posts[1]);
        assert!(posts[2].contains("recovered"), "{command}: {}", posts[2]);

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A message sent while a turn runs does not wait for the turn to end: the model reads it with
/// the result of the tool call that was running.
#[test]
fn a_message_sent_during_a_tool_call_goes_to_the_model_with_its_result() {
    let (url, requests) = start_model_running_shell_command("sleep 1");
    let dir = std::env::temp_dir().join(format!("minerve-serve-queue-{}", std::process::id()));
    let (mut child, stream) = start_with_model_at(&dir, &url);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let hello = wait_for(&mut reader, "hello");
    assert!(hello["features"].to_string().contains("\"queue\""));
    wait_for(&mut reader, "usage");

    writer
        .write_all(b"{\"type\":\"submit\",\"text\":\"hello there\"}\n")
        .unwrap();
    assert_eq!(wait_for(&mut reader, "state")["state"], "working");
    for text in ["use tabs", "and add tests", "taken back"] {
        writeln!(writer, r#"{{"type":"submit","text":"{text}"}}"#).unwrap();
    }
    writer
        .write_all(b"{\"type\":\"unqueue_newest\"}\n")
        .unwrap();

    let mut sent_counts = Vec::new();
    loop {
        let event = read_event(&mut reader);
        match event["type"].as_str() {
            Some("queued_sent") => sent_counts.push(event["count"].as_u64().unwrap()),
            Some("state") => break,
            Some("notice") => panic!("unexpected notice: {event}"),
            _ => {}
        }
    }
    // Both went together, once, and the turn ended by itself after them.
    assert_eq!(sent_counts, [2]);

    let requests = requests.lock().unwrap();
    let posts: Vec<&String> = requests
        .iter()
        .filter(|request| request.starts_with("POST"))
        .collect();
    assert_eq!(posts.len(), 3);
    assert!(!posts[0].contains("use tabs"));
    // One user message, after the result of the call that was running.
    let result_at = posts[1].find("\"role\":\"tool\"").unwrap();
    let message_at = posts[1].find(r"use tabs\n\nand add tests").unwrap();
    assert!(result_at < message_at, "{}", posts[1]);
    assert!(!posts[2].contains("taken back"));

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A turn ends by itself and its chat shows `expected` when the model's server answers
/// `status_line` and `body`.
fn assert_turn_answered_with_ends_showing(
    name: &str,
    status_line: &'static str,
    body: &'static str,
    expected: &[&str],
) {
    let url = start_server_answering(status_line, body);
    let dir = std::env::temp_dir().join(format!("minerve-serve-{name}-{}", std::process::id()));
    let (mut child, stream) = start_with_model_at(&dir, &url);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");

    submit_and_wait_for_ended_turn_showing(&mut writer, &mut reader, expected);

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_answer_without_choices_ends_the_turn_with_an_error() {
    assert_turn_answered_with_ends_showing(
        "nochoice",
        "200 OK",
        r#"{"choices":[]}"#,
        &["no choices"],
    );
}

#[test]
fn an_http_error_ends_the_turn_and_shows_its_status_and_body() {
    assert_turn_answered_with_ends_showing(
        "http-error",
        "429 Too Many Requests",
        r#"{"error":{"message":"rate limited upstream"}}"#,
        &["429 Too Many Requests", "rate limited upstream"],
    );
}

#[test]
fn an_error_sent_with_status_200_ends_the_turn_and_shows_its_body() {
    assert_turn_answered_with_ends_showing(
        "error-200",
        "200 OK",
        r#"{"error":{"message":"provider returned error","code":502}}"#,
        &["provider returned error"],
    );
}

#[test]
fn submit_runs_a_turn_and_cancel_ends_it() {
    let (url, server, stop) = hanging_server();
    let dir = std::env::temp_dir().join(format!("minerve-serve-cancel-{}", std::process::id()));
    let (mut child, stream) = start_with_model_at(&dir, &url);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");

    writer
        .write_all(b"{\"type\":\"submit\",\"text\":\"hello there\"}\n")
        .unwrap();
    assert_eq!(wait_for(&mut reader, "state")["state"], "working");
    let transcript = wait_for(&mut reader, "transcript");
    assert!(transcript["messages"].to_string().contains("hello there"));

    // A second submit while the turn runs waits for it. Taken back, as minerve-gui does before
    // a stop, it starts no turn once the first one is stopped.
    writer
        .write_all(b"{\"type\":\"submit\",\"text\":\"again\"}\n")
        .unwrap();
    writer
        .write_all(b"{\"type\":\"unqueue_newest\"}\n")
        .unwrap();

    writer.write_all(b"{\"type\":\"cancel\"}\n").unwrap();
    assert_eq!(wait_for(&mut reader, "state")["state"], "waiting_for_input");

    let _ = child.kill();
    let _ = child.wait();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = server.join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_the_socket_mid_turn_ends_minerve_and_keeps_the_prompt_saved() {
    let (url, server, stop) = hanging_server();
    let dir = std::env::temp_dir().join(format!("minerve-serve-midturn-{}", std::process::id()));
    let (mut child, stream) = start_with_model_at(&dir, &url);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");
    writer
        .write_all(b"{\"type\":\"submit\",\"text\":\"hello there\"}\n")
        .unwrap();
    assert_eq!(wait_for(&mut reader, "state")["state"], "working");

    drop(writer);
    drop(reader);
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("minerve --serve did not exit after the socket closed mid-turn");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Every message is saved as it comes, so a turn that broke can be read back and resumed.
    let saved: Vec<_> = std::fs::read_dir(dir.join(".minerve/sessions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert!(std::fs::read_to_string(&saved[0])
        .unwrap()
        .contains("hello there"));

    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = server.join();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Save, connect and link, with made-up tokens in a temp HOME: Slack refuses them, but the
/// config is written, the gateway starts and a link code is issued.
#[test]
fn message_apps_saves_connects_and_issues_a_link_code() {
    let dir = std::env::temp_dir().join(format!("minerve-apps-save-{}", std::process::id()));
    let (mut child, stream) = start(&["--serve"], &dir);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");

    writer
        .write_all(b"{\"type\":\"message_apps_save\",\"bot_name\":\"test-bot\",\"app_token\":\"xapp-test\",\"bot_token\":\"xoxb-test\"}\n")
        .unwrap();
    wait_for_notice(&mut reader, "Saved.");
    let apps = wait_for(&mut reader, "message_apps");
    assert_ne!(apps["slack"]["state"], "not_set_up");
    assert_eq!(apps["slack"]["bot_name"], "test-bot");
    assert_eq!(apps["slack"]["has_tokens"], true);
    // Not linked yet, so saving asked the gateway for a code.
    assert_eq!(apps["slack"]["link"], "waiting");
    assert!(!apps["slack"]["link_detail"].as_str().unwrap().is_empty());

    writer
        .write_all(b"{\"type\":\"message_apps_cancel_link\"}\n")
        .unwrap();
    assert_eq!(
        wait_for(&mut reader, "message_apps")["slack"]["link"],
        "none"
    );
    writer
        .write_all(b"{\"type\":\"message_apps_link\"}\n")
        .unwrap();
    assert_eq!(
        wait_for(&mut reader, "message_apps")["slack"]["link"],
        "waiting"
    );
    writer
        .write_all(b"{\"type\":\"message_apps_stop\"}\n")
        .unwrap();
    wait_for_notice(&mut reader, "Disconnected.");
    assert_eq!(
        wait_for(&mut reader, "message_apps")["slack"]["state"],
        "disconnected"
    );
    writer
        .write_all(b"{\"type\":\"message_apps_start\"}\n")
        .unwrap();
    assert_ne!(
        wait_for(&mut reader, "message_apps")["slack"]["state"],
        "disconnected"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Add, connect, disconnect and remove an MCP server in a temp HOME. The server is `false`, which
/// exits at once, so it never connects, but the config file and the registry are the real ones.
#[test]
fn mcp_servers_can_be_added_changed_and_removed() {
    let dir = std::env::temp_dir().join(format!("minerve-mcp-{}", std::process::id()));
    let (mut child, stream) = start(&["--serve"], &dir);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");

    let server_state = |event: &serde_json::Value| {
        event["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "tmp")
            .map(|s| s["state"].as_str().unwrap().to_string())
    };
    writer.write_all(b"{\"type\":\"mcp_add\",\"name\":\"tmp\",\"command_or_url\":\"false\",\"env_or_headers\":\"\"}\n").unwrap();
    wait_for_notice(&mut reader, "Added 'tmp'.");
    let listed = wait_for(&mut reader, "mcp_servers");
    assert!(server_state(&listed).is_some(), "the new server is listed");
    let config = std::fs::read_to_string(dir.join(".minerve/mcp.json")).unwrap();
    assert!(config.contains("\"tmp\""), "saved to mcp.json: {config}");

    writer
        .write_all(b"{\"type\":\"mcp_disconnect\",\"name\":\"tmp\"}\n")
        .unwrap();
    wait_for_notice(&mut reader, "Disconnected 'tmp'.");
    assert_eq!(
        server_state(&wait_for(&mut reader, "mcp_servers")).as_deref(),
        Some("disconnected")
    );
    writer
        .write_all(b"{\"type\":\"mcp_connect\",\"name\":\"tmp\"}\n")
        .unwrap();
    wait_for_notice(&mut reader, "Connecting 'tmp'.");
    assert_ne!(
        server_state(&wait_for(&mut reader, "mcp_servers")).as_deref(),
        Some("disconnected")
    );

    writer
        .write_all(b"{\"type\":\"mcp_remove\",\"name\":\"tmp\"}\n")
        .unwrap();
    wait_for_notice(&mut reader, "Removed 'tmp'.");
    assert_eq!(server_state(&wait_for(&mut reader, "mcp_servers")), None);
    let config = std::fs::read_to_string(dir.join(".minerve/mcp.json")).unwrap_or_default();
    assert!(
        !config.contains("\"tmp\""),
        "removed from mcp.json: {config}"
    );

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The bug from a real session: the agent viewed a screenshot, then deleted it while cleaning
/// up. Every later prompt failed with "cannot read image" before reaching the model.
#[test]
fn deleting_a_viewed_image_does_not_block_the_next_prompt() {
    let dir = std::env::temp_dir().join(format!("minerve-serve-image-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let shot = dir.join("end-check.png");
    // The smallest valid PNG: 1x1, under the downscale size, so it used to be sent by path.
    std::fs::write(
        &shot,
        [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ],
    )
    .unwrap();

    let view = serde_json::json!({"choices": [{"message": {
        "role": "assistant",
        "content": null,
        "tool_calls": [{"id": "call_1", "type": "function", "function": {
            "name": "view_image",
            "arguments": serde_json::json!({ "path": shot.display().to_string() }).to_string(),
        }}],
    }}]})
    .to_string();
    let delete = build_shell_tool_call_answer("call_2", &format!("rm -f {}", shot.display()));
    let done = r#"{"choices":[{"message":{"role":"assistant","content":"all done"}}]}"#.to_string();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let request = read_whole_http_request(&mut stream);
            let body = match request.matches("\"role\":\"tool\"").count() {
                0 => &view,
                1 => &delete,
                _ => &done,
            };
            seen.lock().unwrap().push(request);
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
        }
    });

    let (mut child, stream) = start_with_model_at(&dir.join("home"), &url);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");

    submit_and_wait_for_ended_turn_showing(&mut writer, &mut reader, &["all done"]);
    assert!(!shot.exists(), "the model's rm ran");
    // The next prompt reaches the model, still with the image it viewed.
    submit_and_wait_for_ended_turn_showing(&mut writer, &mut reader, &["all done"]);

    let requests = requests.lock().unwrap();
    let posts: Vec<&String> = requests
        .iter()
        .filter(|request| request.starts_with("POST"))
        .collect();
    assert_eq!(posts.len(), 4, "every prompt reached the model");
    for post in &posts[2..] {
        assert!(post.contains("data:image/png;base64,"), "{post}");
        assert!(!post.contains("Request Error"), "{post}");
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A session that outgrew the model's context window: the request is refused until the history
/// is compacted, then the prompt goes through instead of failing on every later message.
#[test]
fn a_conversation_too_long_for_the_model_is_compacted_and_answered() {
    let call = build_shell_tool_call_answer("call_1", "printf 'x%.0s' $(seq 1 500)");
    let done = r#"{"choices":[{"message":{"role":"assistant","content":"all done"}}]}"#.to_string();
    let too_long = r#"{"type":"error","error":{"type":"invalid_request_error","message":"prompt is too long: 210000 tokens > 200000 maximum"}}"#;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let request = read_whole_http_request(&mut stream);
            let prompts = request.matches("hello there").count();
            let (status, body) = match request.matches("\"role\":\"tool\"").count() {
                0 => ("200 OK", call.as_str()),
                // The second prompt is refused while the long output is still in the history.
                _ if prompts > 1 && request.contains("xxxxxxxxxx") => ("400 Bad Request", too_long),
                _ => ("200 OK", done.as_str()),
            };
            seen.lock().unwrap().push(request);
            let _ = write!(
                stream,
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                body.len(),
                body
            );
        }
    });

    let dir = std::env::temp_dir().join(format!("minerve-serve-compact-{}", std::process::id()));
    let (mut child, stream) = start_with_model_at(&dir, &url);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");

    submit_and_wait_for_ended_turn_showing(&mut writer, &mut reader, &["all done"]);
    submit_and_wait_for_ended_turn_showing(
        &mut writer,
        &mut reader,
        &["outgrew the model's context window", "all done"],
    );

    let requests = requests.lock().unwrap();
    let last = requests.last().unwrap();
    assert!(last.contains("Tool output dropped"), "{last}");
    assert!(!last.contains("prompt is too long"), "{last}");

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
