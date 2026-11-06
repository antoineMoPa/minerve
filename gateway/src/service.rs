//! The gateway as one handle: start it from a config, read its status, show a link code, stop it.
//!
//! Only one process may run it: a second Socket Mode connection for the same Slack app would get
//! half the events. `~/.minerve/message-apps.lock` holds the running process's pid.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::engine::agent::Agent;
use crate::engine::auth::Allowlist;
use crate::engine::config::{self, MessageAppsConfig, SlackTokens};
use crate::engine::inspect;
use crate::engine::link::PendingLink;
use crate::engine::log::log;
use crate::engine::router::Router;
use crate::engine::sessions::SessionStore;
use crate::engine::status::{ConnectionReporter, LinkState, SharedStatus, Status};
use crate::transport::console::Console;
use crate::transport::slack::Slack;
use crate::transport::Transport;

pub struct GatewayOptions {
    pub config: MessageAppsConfig,
    pub slack_tokens: Option<SlackTokens>,
    /// The minerve binary each turn runs, headless.
    pub minerve_bin: PathBuf,
    /// Where those turns run: the folder of the minerve serving the apps.
    pub workdir: PathBuf,
    /// Also read prompts from stdin (the standalone binary's `--console`).
    pub console: bool,
}

#[derive(Debug, PartialEq)]
pub enum StartError {
    /// Another process runs the gateway; its pid when the lock file names one.
    AlreadyRunning(Option<u32>),
    Other(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::AlreadyRunning(Some(pid)) => write!(f, "already running in another minerve (pid {})", pid),
            StartError::AlreadyRunning(None) => write!(f, "already running in another minerve"),
            StartError::Other(err) => f.write_str(err),
        }
    }
}

pub struct Gateway {
    router: Arc<Router>,
    status: SharedStatus,
    tasks: Vec<JoinHandle<()>>,
    /// Held while running; dropping it releases the lock.
    _lock: File,
}

impl Gateway {
    pub fn start(runtime: &Handle, options: GatewayOptions) -> Result<Gateway, StartError> {
        let lock = acquire_lock()?;
        let status = SharedStatus::default();
        let router = Arc::new(Router {
            allowlist: Allowlist::new(options.config.allowed.clone()),
            sessions: SessionStore::open(SessionStore::default_path()),
            agent: Agent::new(options.minerve_bin, options.workdir),
            status: status.clone(),
            link: Mutex::new(None),
            config_path: Some(config::config_path()),
            snapshot_path: Some(inspect::default_path()),
        });
        router.publish_snapshot();

        let mut transports: Vec<Arc<dyn Transport>> = Vec::new();
        if let (Some(settings), Some(tokens)) = (&options.config.slack, options.slack_tokens) {
            transports.push(Arc::new(Slack::new(tokens, settings.icon_url().to_string())));
        }
        if options.console {
            transports.push(Arc::new(Console));
        }

        let (tx, rx) = mpsc::channel(64);
        let mut tasks = Vec::new();
        for transport in &transports {
            let (transport, tx) = (transport.clone(), tx.clone());
            let reporter = ConnectionReporter::new(status.clone(), transport.name());
            tasks.push(runtime.spawn(async move {
                if let Err(err) = transport.run(tx, reporter).await {
                    log(format!("{} stopped: {}", transport.name(), err));
                }
            }));
        }
        drop(tx);
        let by_name: HashMap<_, _> = transports.into_iter().map(|t| (t.name(), t)).collect();
        tasks.push(runtime.spawn(router.clone().serve(by_name, rx)));
        // Connection changes reach no event of their own, so the snapshot is also rewritten on a beat.
        let heartbeat = router.clone();
        tasks.push(runtime.spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                tick.tick().await;
                heartbeat.publish_snapshot();
            }
        }));
        log("started");
        Ok(Gateway { router, status, tasks, _lock: lock })
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Shows a new code, replacing any earlier one.
    pub fn start_link(&self) -> PendingLink {
        let pending = PendingLink::new(Instant::now());
        *self.router.link.lock().unwrap() = Some(pending.clone());
        self.status.lock().unwrap().link = Some(LinkState::Waiting { code: pending.code.clone(), expires_at: pending.expires_at });
        pending
    }

    pub fn cancel_link(&self) {
        *self.router.link.lock().unwrap() = None;
        self.status.lock().unwrap().link = None;
    }

    pub fn workdir(&self) -> PathBuf {
        self.router.agent.workdir.clone()
    }

    pub fn allowed(&self) -> Vec<String> {
        self.router.allowlist.entries()
    }

    /// Ends the connections and any running turn.
    pub fn stop(self) {
        log("stopped");
    }
}

impl Drop for Gateway {
    /// Kills running turns right away: minerve may exit before aborted tasks get to drop them.
    fn drop(&mut self) {
        self.router.agent.kill_all();
        for task in &self.tasks {
            task.abort();
        }
        inspect::remove(&inspect::default_path());
    }
}

fn lock_path() -> PathBuf {
    dirs::home_dir().expect("no home directory").join(".minerve/message-apps.lock")
}

fn acquire_lock() -> Result<File, StartError> {
    let path = lock_path();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|err| StartError::Other(format!("{}: {}", path.display(), err)))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            let mut pid = String::new();
            let _ = file.read_to_string(&mut pid);
            return Err(StartError::AlreadyRunning(pid.trim().parse().ok()));
        }
        Err(std::fs::TryLockError::Error(err)) => return Err(StartError::Other(format!("{}: {}", path.display(), err))),
    }
    let _ = file.set_len(0);
    let _ = file.rewind();
    let _ = write!(file, "{}", std::process::id());
    Ok(file)
}
