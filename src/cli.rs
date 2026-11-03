use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Prompt string to run headlessly
    pub prompt: Option<String>,

    /// Resume a saved session: `-r` for the latest one, or `-r <uuid>` / `-r=<uuid>` for a given one
    #[arg(short, long, num_args = 0..=1, default_missing_value = "latest", value_name = "UUID")]
    pub resume: Option<String>,

    /// Extra system prompt appended after the default one. Repeat the flag for several prompts.
    /// Only applies to new sessions: a resumed session keeps its own system messages.
    #[arg(short = 's', long = "append-system-prompt", value_name = "TEXT")]
    pub system_prompt: Vec<String>,
}
