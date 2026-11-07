//! `/message-apps` for `--serve`: Slack setup, state and linking, as commands. Unlike the TUI,
//! the gateway is not started at launch: the window starts it when you ask.

use std::time::Instant;

use minerve_gateway::engine::config::{self, SlackSettings};
use minerve_gateway::engine::status::{Connection, LinkState};
use minerve_gateway::transport::slack;

use crate::control_socket::{ControlSocket, MessageAppsOut, MinerveEvent, ParentCommand};
use crate::message_apps::{check_bot_name, check_form, SetupForm, MANIFEST_FILE, SLACK_APPS_URL};
use crate::message_apps_runner::{self as runner, RunnerView};

/// True when `command` was a message-apps one.
pub fn handle(command: &ParentCommand, control: &ControlSocket) -> bool {
    let notice: Option<String> = match command {
        ParentCommand::MessageAppsStatus {} => None,
        ParentCommand::MessageAppsStart {} => {
            runner::start();
            None
        }
        ParentCommand::MessageAppsStop {} => {
            runner::stop();
            Some("Disconnected.".to_string())
        }
        ParentCommand::MessageAppsLink {} => match runner::start_link() {
            Some(_) => None,
            None => Some("Not running.".to_string()),
        },
        ParentCommand::MessageAppsCancelLink {} => {
            runner::cancel_link();
            None
        }
        ParentCommand::MessageAppsSave { bot_name, app_token, bot_token } => Some(save(bot_name, app_token, bot_token)),
        ParentCommand::MessageAppsManifest { bot_name } => {
            send_manifest(bot_name.trim(), control);
            return true;
        }
        _ => return false,
    };
    if let Some(notice) = notice {
        control.send(&MinerveEvent::Notice { notice: &notice });
    }
    control.send(&MinerveEvent::MessageApps { slack: &describe() });
    true
}

fn save(bot_name: &str, app_token: &Option<String>, bot_token: &Option<String>) -> String {
    let tokens = config::load_slack_tokens();
    let form = SetupForm {
        bot_name: bot_name.to_string(),
        app_token: app_token.clone().unwrap_or_default(),
        bot_token: bot_token.clone().unwrap_or_default(),
        has_app_token: tokens.is_some(),
        has_bot_token: tokens.is_some(),
        ..SetupForm::default()
    };
    let result = check_form(&form).and_then(|checked| {
        // From disk: someone may have linked since the window last looked.
        let mut saved = config::load()?;
        let icon_url = saved.slack.as_ref().and_then(|slack| slack.icon_url.clone());
        saved.slack = Some(SlackSettings { icon_url, ..checked.settings });
        config::save(&saved)?;
        config::save_slack_tokens(checked.app_token.as_deref(), checked.bot_token.as_deref())?;
        Ok(saved)
    });
    match result {
        Ok(saved) => {
            runner::start();
            if !saved.allowed.iter().any(|entry| entry.starts_with("slack:")) {
                runner::start_link();
            }
            "Saved.".to_string()
        }
        Err(err) => err,
    }
}

fn send_manifest(bot_name: &str, control: &ControlSocket) {
    if let Err(err) = check_bot_name(bot_name) {
        control.send(&MinerveEvent::Notice { notice: &err });
        return;
    }
    let manifest = slack::manifest(bot_name);
    if let Some(path) = dirs::home_dir().map(|home| home.join(MANIFEST_FILE)) {
        let _ = std::fs::write(path, &manifest);
    }
    crate::mcp::oauth::open_in_browser(SLACK_APPS_URL);
    control.send(&MinerveEvent::MessageAppsManifest { manifest: &manifest, url: SLACK_APPS_URL });
}

fn describe() -> MessageAppsOut {
    let loaded = config::load();
    let has_tokens = config::load_slack_tokens().is_some();
    let (bot_name, members) = match &loaded {
        Ok(config) => (
            config.slack.as_ref().map(|slack| slack.bot_name.clone()).unwrap_or_default(),
            config.allowed.iter().filter_map(|entry| entry.strip_prefix("slack:")).map(String::from).collect(),
        ),
        Err(_) => (String::new(), Vec::new()),
    };
    let set_up = matches!(&loaded, Ok(config) if config.slack.is_some()) && has_tokens;
    let (mut state, mut detail, mut link, mut link_detail, mut link_seconds_left) =
        ("not_set_up", String::new(), "none", String::new(), 0);
    if let Err(err) = &loaded {
        (state, detail) = ("failed", err.clone());
    } else if set_up {
        match runner::view() {
            RunnerView::Stopped => state = "disconnected",
            RunnerView::Blocked(err) => (state, detail) = ("failed", err),
            RunnerView::Running { status, .. } => {
                match status.connections.get("slack") {
                    None | Some(Connection::Connecting) => state = "connecting",
                    Some(Connection::Connected { workspace, bot }) => {
                        (state, detail) = ("connected", format!("{workspace} · @{bot}"));
                    }
                    Some(Connection::Failed(err)) => (state, detail) = ("failed", err.clone()),
                }
                match status.link {
                    Some(LinkState::Waiting { code, expires_at }) => {
                        link = "waiting";
                        link_detail = code;
                        link_seconds_left = expires_at.saturating_duration_since(Instant::now()).as_secs();
                    }
                    Some(LinkState::Linked { member }) => (link, link_detail) = ("linked", member),
                    None => {}
                }
            }
        }
    }
    MessageAppsOut { state, detail, bot_name, has_tokens, members, link, link_detail, link_seconds_left }
}
