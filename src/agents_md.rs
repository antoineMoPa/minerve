//! Loads AGENTS.md instructions into a new session's system prompt.
//!
//! Read in this order, skipping files that are missing, empty, or already read:
//! 1. `~/AGENTS.md` (personal instructions for every project)
//! 2. `AGENTS.md` in each directory from the filesystem root down to the current directory,
//!    so the project root's file comes before the nearest directory's file.

use std::path::{Path, PathBuf};

const AGENTS_FILE: &str = "AGENTS.md";

/// One system prompt per AGENTS.md file that has content, each tagged with its path.
pub fn load_agents_md() -> Vec<String> {
    let Ok(cwd) = std::env::current_dir() else {
        return Vec::new();
    };

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(AGENTS_FILE));
    }
    let mut ancestors: Vec<&Path> = cwd.ancestors().collect();
    ancestors.reverse();
    candidates.extend(ancestors.into_iter().map(|dir| dir.join(AGENTS_FILE)));

    let mut seen: Vec<PathBuf> = Vec::new();
    let mut prompts = Vec::new();
    for path in candidates.into_iter().filter(|path| path.is_file()) {
        // The home directory can be the same folder as the current one; read each file once.
        let Ok(canonical) = path.canonicalize() else {
            continue;
        };
        if seen.contains(&canonical) {
            continue;
        }
        seen.push(canonical);

        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let text = text.trim();
        if !text.is_empty() {
            prompts.push(format!("Instructions from {}:\n{}", path.display(), text));
        }
    }
    prompts
}
