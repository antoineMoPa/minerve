mod core;
mod transport;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::core::agent::Agent;
use crate::core::auth::Allowlist;
use crate::core::router::Router;
use crate::core::sessions::SessionStore;
use crate::transport::console::Console;
use crate::transport::Transport;

#[tokio::main]
async fn main() {
    let allowlist = Allowlist::from_csv(&std::env::var("GATEWAY_ALLOWED_USERS").unwrap_or_default());
    if allowlist.is_empty() {
        eprintln!("gateway: GATEWAY_ALLOWED_USERS is empty, every message will be ignored (the console user is `me`)");
    }
    let router = Arc::new(Router {
        allowlist,
        sessions: SessionStore::open(SessionStore::default_path()),
        agent: Agent {
            minerve_bin: std::env::var_os("MINERVE_BIN").map(PathBuf::from).unwrap_or_else(|| "minerve".into()),
            workdir: std::env::var_os("GATEWAY_WORKDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().expect("no working directory")),
        },
    });

    let transports: Vec<Arc<dyn Transport>> = vec![Arc::new(Console)];
    let (tx, rx) = mpsc::channel(64);
    for transport in &transports {
        let (transport, tx) = (transport.clone(), tx.clone());
        tokio::spawn(async move {
            if let Err(err) = transport.run(tx).await {
                eprintln!("gateway: {} stopped: {}", transport.name(), err);
            }
        });
    }
    drop(tx);
    let by_name = transports.into_iter().map(|t| (t.name(), t)).collect::<HashMap<_, _>>();
    router.serve(by_name, rx).await;
}
