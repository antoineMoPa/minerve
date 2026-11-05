//! The gateway minerve runs while its TUI is open, so `/message-apps` can show what it does.
//! It stops when minerve quits. Only one minerve runs it at a time (the gateway holds a lock);
//! the others show why.

use std::sync::Mutex;

use minerve_gateway::engine::config::{self, MessageAppsConfig};
use minerve_gateway::engine::link::PendingLink;
use minerve_gateway::engine::status::Status;
use minerve_gateway::service::{Gateway, GatewayOptions};

use crate::get_global_runtime;

pub enum Runner {
    /// Not set up, or disconnected with `d`.
    Stopped,
    Running(Gateway),
    /// Could not start: another minerve runs it, or the config is broken.
    Blocked(String),
}

static RUNNER: Mutex<Runner> = Mutex::new(Runner::Stopped);

/// Called before the TUI starts: runs the gateway when Slack is set up.
pub fn start_if_configured() {
    let ready = matches!(config::load(), Ok(MessageAppsConfig { slack: Some(_), .. }))
        && config::load_slack_tokens().is_some();
    if ready {
        start();
    }
}

/// (Re)starts from the saved config and tokens.
pub fn start() {
    let mut runner = RUNNER.lock().unwrap();
    // The old gateway must let go of its lock before the new one takes it.
    *runner = Runner::Stopped;
    *runner = match start_gateway() {
        Ok(gateway) => Runner::Running(gateway),
        Err(err) => Runner::Blocked(err),
    };
}

fn start_gateway() -> Result<Gateway, String> {
    let config = config::load()?;
    let minerve_bin = std::env::current_exe().map_err(|err| format!("cannot find the minerve binary: {}", err))?;
    let options = GatewayOptions { config, slack_tokens: config::load_slack_tokens(), minerve_bin, console: false };
    Gateway::start(get_global_runtime().handle(), options).map_err(|err| err.to_string())
}

pub fn stop() {
    let mut runner = RUNNER.lock().unwrap();
    if let Runner::Running(gateway) = std::mem::replace(&mut *runner, Runner::Stopped) {
        gateway.stop();
    }
}

/// What the modal draws from.
pub enum RunnerView {
    Stopped,
    Running { status: Status, allowed: Vec<String> },
    Blocked(String),
}

pub fn view() -> RunnerView {
    match &*RUNNER.lock().unwrap() {
        Runner::Stopped => RunnerView::Stopped,
        Runner::Running(gateway) => RunnerView::Running { status: gateway.status(), allowed: gateway.allowed() },
        Runner::Blocked(err) => RunnerView::Blocked(err.clone()),
    }
}

/// A new link code, or `None` when the gateway is not running.
pub fn start_link() -> Option<PendingLink> {
    match &*RUNNER.lock().unwrap() {
        Runner::Running(gateway) => Some(gateway.start_link()),
        _ => None,
    }
}

pub fn cancel_link() {
    if let Runner::Running(gateway) = &*RUNNER.lock().unwrap() {
        gateway.cancel_link();
    }
}
