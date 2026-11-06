//! `minerve --threads`: what the Slack (and other chat app) threads cost, how many messages and
//! tool calls they hold, and which gateway runs them now.
//!
//! Stored data comes from `~/.minerve/message-apps-sessions.json` (conversation to session) and
//! each `~/.minerve/sessions/<id>.json`. Live data comes from the gateway's status snapshot
//! `~/.minerve/message-apps-status.json`, trusted only while its pid is alive.
//!
//! Read-only: it never starts, stops or writes anything.

use std::collections::BTreeMap;
use std::path::Path;

use minerve_gateway::engine::inspect::{self, Snapshot};
use serde::Serialize;

/// One conversation's thread, joined from the stored session and the live snapshot.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct ThreadRow {
    pub conversation: String,
    pub session_id: String,
    /// Turn running now, per the live snapshot.
    pub running: bool,
    /// The session file exists; false for a thread never answered.
    pub has_session: bool,
    pub cost_usd: f64,
    pub requests_without_cost: u64,
    pub user_messages: usize,
    pub assistant_messages: usize,
    pub tool_calls: usize,
    /// Tool calls by name, most used first.
    pub tools: Vec<(String, usize)>,
    pub updated_at: String,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct GatewayRow {
    pub pid: u32,
    /// Alive now. A snapshot whose pid is gone is a gateway that stopped without cleaning up.
    pub alive: bool,
    pub workdir: String,
    pub connections: BTreeMap<String, String>,
    pub working: usize,
    pub running: Vec<String>,
    pub cost_usd: f64,
    pub cost_incomplete: bool,
    pub updated_at_unix: u64,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Report {
    pub gateway: Option<GatewayRow>,
    pub threads: Vec<ThreadRow>,
    pub total_cost_usd: f64,
}

/// Whether a process with this pid exists (signal 0 checks without sending anything).
pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: kill(2) with signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Builds the report from the files in `dir` (`~/.minerve`).
pub fn build(dir: &Path) -> Report {
    let snapshot = inspect::read(&dir.join("message-apps-status.json"));
    let running: Vec<String> = snapshot.as_ref().map(|s| s.running.clone()).unwrap_or_default();
    let threads = read_threads(dir, &running);
    let total_cost_usd = threads.iter().map(|t| t.cost_usd).sum();
    Report {
        gateway: snapshot.map(gateway_row),
        threads,
        total_cost_usd,
    }
}

fn gateway_row(snapshot: Snapshot) -> GatewayRow {
    GatewayRow {
        alive: pid_alive(snapshot.pid),
        pid: snapshot.pid,
        workdir: snapshot.workdir,
        connections: snapshot.connections,
        working: snapshot.working,
        running: snapshot.running,
        cost_usd: snapshot.cost_usd,
        cost_incomplete: snapshot.cost_incomplete,
        updated_at_unix: snapshot.updated_at_unix,
    }
}

fn read_threads(dir: &Path, running: &[String]) -> Vec<ThreadRow> {
    let map: BTreeMap<String, String> = std::fs::read_to_string(dir.join("message-apps-sessions.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let mut rows: Vec<ThreadRow> = map
        .into_iter()
        .map(|(conversation, session_id)| {
            let is_running = running.contains(&conversation);
            session_row(dir, conversation, session_id, is_running)
        })
        .collect();
    rows.sort_by(|a, b| b.cost_usd.total_cmp(&a.cost_usd).then(a.conversation.cmp(&b.conversation)));
    rows
}

fn session_row(dir: &Path, conversation: String, session_id: String, running: bool) -> ThreadRow {
    let path = dir.join("sessions").join(format!("{}.json", session_id));
    let mut row = ThreadRow {
        conversation,
        session_id,
        running,
        has_session: path.exists(),
        cost_usd: 0.0,
        requests_without_cost: 0,
        user_messages: 0,
        assistant_messages: 0,
        tool_calls: 0,
        tools: Vec::new(),
        updated_at: String::new(),
    };
    let Some(session) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else {
        return row;
    };
    let usage = &session["usage"];
    row.cost_usd = usage["cost_usd"].as_f64().unwrap_or(0.0);
    row.requests_without_cost = usage["requests_without_cost"].as_u64().unwrap_or(0);
    row.updated_at = session["updated_at"].as_str().unwrap_or_default().to_string();
    let mut tools: BTreeMap<String, usize> = BTreeMap::new();
    for message in session["messages"].as_array().into_iter().flatten() {
        match message["role"].as_str() {
            Some("user") => row.user_messages += 1,
            Some("assistant") => row.assistant_messages += 1,
            _ => {}
        }
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            row.tool_calls += 1;
            let name = call["function"]["name"].as_str().unwrap_or("?").to_string();
            *tools.entry(name).or_default() += 1;
        }
    }
    let mut tools: Vec<(String, usize)> = tools.into_iter().collect();
    tools.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    row.tools = tools;
    row
}

/// Prints the report for `~/.minerve` to stdout, as text or JSON.
pub fn print(json: bool) {
    let Some(dir) = dirs::home_dir().map(|home| home.join(".minerve")) else {
        eprintln!("minerve: no home directory");
        std::process::exit(1);
    };
    let report = build(&dir);
    if json {
        println!("{}", serde_json::to_string_pretty(&report).expect("report serializes"));
    } else {
        print!("{}", render_text(&report));
    }
}

/// The plain-text report.
pub fn render_text(report: &Report) -> String {
    let mut out = String::new();
    match &report.gateway {
        None => out.push_str("gateway: not running (no status snapshot)\n"),
        Some(g) => {
            let state = if g.alive { "running" } else { "STOPPED (stale snapshot)" };
            out.push_str(&format!("gateway: {} pid {} in {}\n", state, g.pid, g.workdir));
            for (name, connection) in &g.connections {
                out.push_str(&format!("  {}: {}\n", name, connection));
            }
            out.push_str(&format!(
                "  working now: {}  cost since start: ${:.4}{}\n",
                g.working,
                g.cost_usd,
                if g.cost_incomplete { " (floor: some cost unreported)" } else { "" }
            ));
        }
    }
    out.push_str(&format!("\nthreads: {}  total stored cost: ${:.4}\n", report.threads.len(), report.total_cost_usd));
    for t in &report.threads {
        let state = if t.running {
            "RUNNING"
        } else if !t.has_session {
            "no session"
        } else {
            "idle"
        };
        out.push_str(&format!("\n[{}] {}\n", state, t.conversation));
        out.push_str(&format!("  session {}  updated {}\n", t.session_id, if t.updated_at.is_empty() { "-" } else { &t.updated_at }));
        out.push_str(&format!(
            "  cost ${:.4}{}  messages: {} user / {} assistant  tool calls: {}\n",
            t.cost_usd,
            if t.requests_without_cost > 0 { format!(" (+{} unreported)", t.requests_without_cost) } else { String::new() },
            t.user_messages,
            t.assistant_messages,
            t.tool_calls
        ));
        if !t.tools.is_empty() {
            let tools: Vec<String> = t.tools.iter().map(|(name, n)| format!("{} x{}", name, n)).collect();
            out.push_str(&format!("  tools: {}\n", tools.join(", ")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("threads-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        dir
    }

    #[test]
    fn joins_stored_threads_with_the_live_snapshot() {
        let dir = temp_dir();
        std::fs::write(
            dir.join("message-apps-sessions.json"),
            r#"{"slack:T1:C1:1.0":"s1","slack:T1:C2:2.0":"s2"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("sessions/s1.json"),
            r#"{"updated_at":"now","usage":{"cost_usd":0.25,"requests_without_cost":0},"messages":[
                {"role":"system","content":"x"},
                {"role":"user","content":"hi"},
                {"role":"assistant","content":null,"tool_calls":[
                    {"id":"a","type":"function","function":{"name":"run_shell_command","arguments":"{}"}},
                    {"id":"b","type":"function","function":{"name":"run_shell_command","arguments":"{}"}}]},
                {"role":"tool","content":"ok"},
                {"role":"assistant","content":"done"}]}"#,
        )
        .unwrap();
        let snapshot = Snapshot { pid: std::process::id(), running: vec!["slack:T1:C1:1.0".into()], working: 1, ..Default::default() };
        inspect::write(&dir.join("message-apps-status.json"), &snapshot);

        let report = build(&dir);
        let g = report.gateway.clone().unwrap();
        assert!(g.alive, "our own pid is alive");
        assert_eq!(report.threads.len(), 2);
        let s1 = report.threads.iter().find(|t| t.session_id == "s1").unwrap();
        assert!(s1.running);
        assert!(s1.has_session);
        assert_eq!(s1.user_messages, 1);
        assert_eq!(s1.assistant_messages, 2);
        assert_eq!(s1.tool_calls, 2);
        assert_eq!(s1.tools, vec![("run_shell_command".to_string(), 2)]);
        let s2 = report.threads.iter().find(|t| t.session_id == "s2").unwrap();
        assert!(!s2.running);
        assert!(!s2.has_session);
        assert!((report.total_cost_usd - 0.25).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn no_snapshot_means_the_gateway_is_not_running() {
        let dir = temp_dir();
        let report = build(&dir);
        assert!(report.gateway.is_none());
        assert!(render_text(&report).contains("not running"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
