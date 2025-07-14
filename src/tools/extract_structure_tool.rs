use async_trait::async_trait;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead};
use std::path::Path;
use crate::tools::{ExecuteCommandSettings, Tool};

pub struct ExtractStructureTool;

#[async_trait]
impl Tool for ExtractStructureTool {
    fn name(&self) -> &'static str {
        "extract_structure"
    }

    fn description(&self) -> &'static str {
        "Extracts structure of a file, showing nested blocks. Use this to get an overview of a code file."
    }

    fn parameters(&self) -> HashMap<&'static str, &'static str> {
        let mut params = HashMap::new();
        params.insert("filepath", "string");
        params
    }

    async fn run(&self, args: HashMap<String, String>, _settings: ExecuteCommandSettings) -> String {
        let filepath = match args.get("filepath") {
            Some(f) => f,
            None => return String::from("[Error] Missing 'filepath' parameter."),
        };

        match extract_structure(filepath) {
            Ok(output) => output,
            Err(e) => format!("[Error] Failed to extract structure: {}", e),
        }
    }
}

fn extract_structure<P: AsRef<Path>>(path: P) -> io::Result<String> {
    let file = File::open(path)?;
    let reader = io::BufReader::new(file);

    let mut depth = 0;
    let mut output = String::new();

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();

        // Skip empty or comment lines
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with("*")
        {
            continue;
        }

        // If line contains opening block char, treat it as a structure header
        if trimmed.contains('{') || trimmed.ends_with(':') {
            output.push_str(&format!(
                "{}{}\n",
                "    ".repeat(depth),
                trimmed.replace('{', "").trim()
            ));
            depth += 1;

            // optionally emit a placeholder for content
            output.push_str(&format!("{}// [...]\n", "    ".repeat(depth)));
        } else if trimmed.contains('}') {
            if depth > 0 {
                depth -= 1;
            }
        }
    }

    Ok(output)
}
