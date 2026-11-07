use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Prompt string to run headlessly
    pub prompt: Option<String>,

    /// Resume a saved session: `-r` for the latest one, or `-r <uuid>` / `-r=<uuid>` for a given one
    #[arg(short, long, num_args = 0..=1, default_missing_value = "latest", value_name = "UUID")]
    pub resume: Option<String>,

    /// Start a new session under this id (a lowercase hyphenated UUID), so a launcher can
    /// resume exactly this session later with `--resume=<uuid>`.
    #[arg(long, value_name = "UUID", conflicts_with = "resume")]
    pub session_id: Option<String>,

    /// Extra system prompt appended after the default one. Repeat the flag for several prompts.
    /// Only applies to new sessions: a resumed session keeps its own system messages.
    #[arg(short = 's', long = "append-system-prompt", value_name = "TEXT")]
    pub system_prompt: Vec<String>,

    /// Print the chat-app threads (Slack and others): cost, messages, tool calls, and which gateway
    /// runs them now. Read-only; prints and exits. Add `--json` for machine-readable output.
    #[arg(long, conflicts_with_all = ["prompt", "resume", "session_id", "setup"])]
    pub threads: bool,

    /// With `--threads`: print JSON instead of text.
    #[arg(long, requires = "threads")]
    pub json: bool,

    /// Open the API key setup modal, even when a key is already set. Same as `/apikeys`.
    /// Saving writes ~/.minerve/keys.json; a blank field keeps the stored key.
    #[arg(long)]
    pub setup: bool,

    /// Run without a terminal UI, driven over MINERVE_CONTROL_SOCKET. Used by minerve-gui.
    #[arg(long, conflicts_with_all = ["prompt", "threads", "setup"])]
    pub serve: bool,
}
