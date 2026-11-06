//! `minerve-gateway`: runs the gateway without minerve's TUI.
//!
//! `minerve-gateway manifest <bot name>` prints the Slack app manifest.
//! `minerve-gateway [--link] [--console]` reads `~/.minerve/message-apps.toml` and the Slack
//! tokens, then serves until killed. `--link` prints a one-time code to link your account.

use minerve_gateway::engine::config;
use minerve_gateway::engine::log::log_path;
use minerve_gateway::service::{Gateway, GatewayOptions};
use minerve_gateway::transport::slack;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("manifest") {
        let Some(name) = args.get(1) else {
            eprintln!("usage: minerve-gateway manifest <bot name>");
            std::process::exit(2);
        };
        print!("{}", slack::manifest(name));
        return;
    }
    let config = config::load().unwrap_or_else(|err| {
        eprintln!("minerve-gateway: {}", err);
        std::process::exit(1);
    });
    let options = GatewayOptions {
        config,
        slack_tokens: config::load_slack_tokens(),
        minerve_bin: std::env::var_os("MINERVE_BIN").map(Into::into).unwrap_or_else(|| "minerve".into()),
        console: args.iter().any(|arg| arg == "--console"),
        workdir: std::env::current_dir().unwrap_or_else(|_| ".".into()),
    };
    let gateway = Gateway::start(&tokio::runtime::Handle::current(), options).unwrap_or_else(|err| {
        eprintln!("minerve-gateway: {}", err);
        std::process::exit(1);
    });
    if args.iter().any(|arg| arg == "--link") {
        let link = gateway.start_link();
        eprintln!("Send this to the bot in a DM within 5 minutes: {}", link.code);
    }
    eprintln!("minerve-gateway: running, log in {}", log_path().display());
    std::future::pending::<()>().await;
}
