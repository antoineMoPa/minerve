use clap::Parser;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Prompt string to run headlessly
    pub prompt: Option<String>,
}

