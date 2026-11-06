//! Runs minerve for one turn and reads its answer back from the session file.
//!
//! minerve's headless stdout mixes the reply with tool-call and usage lines, so the reply is
//! taken from `~/.minerve/sessions/<id>.json`, which minerve saves after every turn.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;

use tokio::process::Command;

/// Tokens the agent has no use for; not passed to the minerve it runs.
const HIDDEN_ENV: [&str; 2] = ["SLACK_APP_TOKEN", "SLACK_BOT_TOKEN"];

pub struct Agent {
    pub minerve_bin: PathBuf,
    pub workdir: PathBuf,
    /// Process groups of the turns running now, so `kill_all` can end them and their shells.
    running: Mutex<HashSet<u32>>,
}

impl Agent {
    pub fn new(minerve_bin: PathBuf, workdir: PathBuf) -> Self {
        Self { minerve_bin, workdir, running: Mutex::new(HashSet::new()) }
    }

    /// Sends `prompt` in session `session_id` and returns what minerve answered to it.
    /// A session with no file yet starts with `--session-id`; minerve saves the file as it starts,
    /// so a turn that failed before that simply starts it again.
    pub async fn run_turn(&self, session_id: &str, system_prompts: &[String], prompt: &str) -> Result<String, String> {
        let path = session_file(session_id)?;
        let is_new = !path.exists();
        let mut command = Command::new(&self.minerve_bin);
        if is_new {
            command.arg(format!("--session-id={}", session_id));
            // minerve applies --append-system-prompt to new sessions only.
            for system_prompt in system_prompts {
                command.arg("--append-system-prompt").arg(system_prompt);
            }
        } else {
            command.arg(format!("--resume={}", session_id));
        }
        for name in HIDDEN_ENV {
            command.env_remove(name);
        }
        // `--`: a message like `-h` is a prompt, not a flag.
        let child = command
            .arg("--")
            .arg(prompt)
            .current_dir(&self.workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own process group, so stopping the gateway also ends the shells it started.
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(|err| format!("could not start {}: {}", self.minerve_bin.display(), err))?;
        let pid = child.id();
        if let Some(pid) = pid {
            self.running.lock().unwrap().insert(pid);
        }
        let output = child.wait_with_output().await;
        if let Some(pid) = pid {
            self.running.lock().unwrap().remove(&pid);
        }
        let output = output.map_err(|err| format!("minerve did not finish: {}", err))?;
        if !output.status.success() {
            return Err(format!("minerve failed ({}): {}", output.status, String::from_utf8_lossy(&output.stderr).trim()));
        }
        let text = std::fs::read_to_string(&path).map_err(|err| format!("could not read {}: {}", path.display(), err))?;
        last_assistant_reply(&text).ok_or_else(|| "minerve produced no reply".to_string())
    }

    /// Ends every running turn, with whatever it started.
    pub fn kill_all(&self) {
        for pid in self.running.lock().unwrap().drain() {
            // SAFETY: plain kill(2) on a process group this agent created.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
        }
    }
}

/// What a session has cost so far, from the usage minerve saves in its file. Zero before it exists.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SessionCost {
    pub usd: f64,
    /// Requests whose provider reported no cost (Anthropic and OpenAI do not).
    pub requests_without_cost: u64,
}

pub fn session_cost(session_id: &str) -> SessionCost {
    let usage = session_file(session_id)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .map(|session| session["usage"].clone())
        .unwrap_or_default();
    SessionCost {
        usd: usage["cost_usd"].as_f64().unwrap_or(0.0),
        requests_without_cost: usage["requests_without_cost"].as_u64().unwrap_or(0),
    }
}

/// Where minerve saves a session. Errors only when there is no home directory.
pub fn session_file(session_id: &str) -> Result<PathBuf, String> {
    Ok(dirs::home_dir().ok_or("no home directory")?.join(format!(".minerve/sessions/{}.json", session_id)))
}

/// The last assistant text after the last user message: this turn's answer. `None` when the turn
/// added none (an API error, say), rather than repeating the previous turn's answer.
pub fn last_assistant_reply(session_json: &str) -> Option<String> {
    let session: serde_json::Value = serde_json::from_str(session_json).ok()?;
    let messages = session["messages"].as_array()?;
    let turn_start = messages.iter().rposition(|message| message["role"] == "user").map_or(0, |i| i + 1);
    messages[turn_start..]
        .iter()
        .rev()
        .filter(|message| message["role"] == "assistant")
        .find_map(|message| message["content"].as_str().filter(|text| !text.trim().is_empty()))
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_last_assistant_text() {
        let json = r#"{"messages":[
            {"role":"user","content":"hi"},
            {"role":"assistant","content":"first"},
            {"role":"assistant","content":null,"tool_calls":[]},
            {"role":"tool","content":"x"},
            {"role":"assistant","content":"done"}]}"#;
        assert_eq!(last_assistant_reply(json).as_deref(), Some("done"));
    }

    #[test]
    fn never_repeats_the_previous_turns_answer() {
        let json = r#"{"messages":[
            {"role":"user","content":"one"},
            {"role":"assistant","content":"answer one"},
            {"role":"user","content":"two"}]}"#;
        assert_eq!(last_assistant_reply(json), None);
    }

    #[test]
    fn none_without_assistant_text() {
        assert_eq!(last_assistant_reply(r#"{"messages":[{"role":"user","content":"hi"}]}"#), None);
    }
}
