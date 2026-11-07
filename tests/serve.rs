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
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);

    let hello = read_event(&mut reader);
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["version"], 1);
    assert!(hello["features"].as_array().unwrap().iter().any(|feature| feature == "intro"));
    let first_session = wait_for(&mut reader, "session")["session_id"].as_str().unwrap().to_string();
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
    writer.write_all(b"{\"type\":\"mcp_remove\",\"name\":\"nope\"}\n").unwrap();
    assert!(wait_for(&mut reader, "notice")["notice"].as_str().unwrap().contains("no MCP server"));
    writer.write_all(b"{\"type\":\"message_apps_status\"}\n").unwrap();
    let apps = wait_for(&mut reader, "message_apps");
    assert_eq!(apps["slack"]["state"], "not_set_up");
    writer.write_all(b"{\"type\":\"list_sessions\"}\n").unwrap();
    assert!(wait_for(&mut reader, "sessions")["sessions"].is_array());
    writer.write_all(b"{\"type\":\"get_api_keys\"}\n").unwrap();
    let keys = wait_for(&mut reader, "api_keys");
    assert!(keys["openrouter"].is_boolean() && keys["openai"].is_boolean() && keys["anthropic"].is_boolean());

    writer.write_all(b"{\"type\":\"new_session\"}\n").unwrap();
    let second_session = wait_for(&mut reader, "session")["session_id"].as_str().unwrap().to_string();
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
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
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
    let saved = std::fs::read_dir(&sessions).map(|entries| entries.count()).unwrap_or(0);
    assert_eq!(saved, 0, "a chat with no messages must not leave a session file");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Reads events until a notice contains `text`.
fn wait_for_notice(reader: &mut impl BufRead, text: &str) {
    for _ in 0..20 {
        let event = read_event(reader);
        if event["type"] == "notice" && event["notice"].as_str().is_some_and(|notice| notice.contains(text)) {
            return;
        }
    }
    panic!("no notice containing {text:?}");
}

fn start(args: &[&str], dir: &std::path::Path) -> (std::process::Child, std::os::unix::net::UnixStream) {
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
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
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

    writer.write_all(b"{\"type\":\"message_apps_save\",\"bot_name\":\"Bad Name\"}\n").unwrap();
    wait_for_notice(&mut reader, "Bot name");
    writer
        .write_all(b"{\"type\":\"message_apps_save\",\"bot_name\":\"ok-bot\",\"app_token\":\"nope\",\"bot_token\":\"xoxb-1\"}\n")
        .unwrap();
    wait_for_notice(&mut reader, "xapp-");
    writer.write_all(b"{\"type\":\"message_apps_link\"}\n").unwrap();
    wait_for_notice(&mut reader, "Not running.");
    assert_eq!(wait_for(&mut reader, "message_apps")["slack"]["state"], "not_set_up");

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A server that takes connections and never answers, so a turn stays running.
fn hanging_server() -> (String, std::thread::JoinHandle<()>, std::sync::Arc<std::sync::atomic::AtomicBool>) {
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

fn start_with_hanging_model(dir: &std::path::Path, url: &str) -> (std::process::Child, std::os::unix::net::UnixStream) {
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
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    (child, stream)
}

#[test]
fn submit_runs_a_turn_and_cancel_ends_it() {
    let (url, server, stop) = hanging_server();
    let dir = std::env::temp_dir().join(format!("minerve-serve-cancel-{}", std::process::id()));
    let (mut child, stream) = start_with_hanging_model(&dir, &url);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");

    writer.write_all(b"{\"type\":\"submit\",\"text\":\"hello there\"}\n").unwrap();
    assert_eq!(wait_for(&mut reader, "state")["state"], "working");
    let transcript = wait_for(&mut reader, "transcript");
    assert!(transcript["messages"].to_string().contains("hello there"));

    // A second submit while the turn runs is refused, not queued.
    writer.write_all(b"{\"type\":\"submit\",\"text\":\"again\"}\n").unwrap();
    wait_for_notice(&mut reader, "busy");

    writer.write_all(b"{\"type\":\"cancel\"}\n").unwrap();
    assert_eq!(wait_for(&mut reader, "state")["state"], "waiting_for_input");

    let _ = child.kill();
    let _ = child.wait();
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = server.join();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn closing_the_socket_mid_turn_ends_minerve_and_saves_nothing() {
    let (url, server, stop) = hanging_server();
    let dir = std::env::temp_dir().join(format!("minerve-serve-midturn-{}", std::process::id()));
    let (mut child, stream) = start_with_hanging_model(&dir, &url);
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    wait_for(&mut reader, "usage");
    writer.write_all(b"{\"type\":\"submit\",\"text\":\"hello there\"}\n").unwrap();
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
    // The turn never finished, so no half-done session may be on disk to resume.
    let saved = std::fs::read_dir(dir.join(".minerve/sessions")).map(|entries| entries.count()).unwrap_or(0);
    assert_eq!(saved, 0, "closing mid-turn must not leave a session file");

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

    writer.write_all(b"{\"type\":\"message_apps_cancel_link\"}\n").unwrap();
    assert_eq!(wait_for(&mut reader, "message_apps")["slack"]["link"], "none");
    writer.write_all(b"{\"type\":\"message_apps_link\"}\n").unwrap();
    assert_eq!(wait_for(&mut reader, "message_apps")["slack"]["link"], "waiting");
    writer.write_all(b"{\"type\":\"message_apps_stop\"}\n").unwrap();
    wait_for_notice(&mut reader, "Disconnected.");
    assert_eq!(wait_for(&mut reader, "message_apps")["slack"]["state"], "disconnected");
    writer.write_all(b"{\"type\":\"message_apps_start\"}\n").unwrap();
    assert_ne!(wait_for(&mut reader, "message_apps")["slack"]["state"], "disconnected");

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
        event["servers"].as_array().unwrap().iter().find(|s| s["name"] == "tmp").map(|s| s["state"].as_str().unwrap().to_string())
    };
    writer.write_all(b"{\"type\":\"mcp_add\",\"name\":\"tmp\",\"command_or_url\":\"false\",\"env_or_headers\":\"\"}\n").unwrap();
    wait_for_notice(&mut reader, "Added 'tmp'.");
    let listed = wait_for(&mut reader, "mcp_servers");
    assert!(server_state(&listed).is_some(), "the new server is listed");
    let config = std::fs::read_to_string(dir.join(".minerve/mcp.json")).unwrap();
    assert!(config.contains("\"tmp\""), "saved to mcp.json: {config}");

    writer.write_all(b"{\"type\":\"mcp_disconnect\",\"name\":\"tmp\"}\n").unwrap();
    wait_for_notice(&mut reader, "Disconnected 'tmp'.");
    assert_eq!(server_state(&wait_for(&mut reader, "mcp_servers")).as_deref(), Some("disconnected"));
    writer.write_all(b"{\"type\":\"mcp_connect\",\"name\":\"tmp\"}\n").unwrap();
    wait_for_notice(&mut reader, "Connecting 'tmp'.");
    assert_ne!(server_state(&wait_for(&mut reader, "mcp_servers")).as_deref(), Some("disconnected"));

    writer.write_all(b"{\"type\":\"mcp_remove\",\"name\":\"tmp\"}\n").unwrap();
    wait_for_notice(&mut reader, "Removed 'tmp'.");
    assert_eq!(server_state(&wait_for(&mut reader, "mcp_servers")), None);
    let config = std::fs::read_to_string(dir.join(".minerve/mcp.json")).unwrap_or_default();
    assert!(!config.contains("\"tmp\""), "removed from mcp.json: {config}");

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}
