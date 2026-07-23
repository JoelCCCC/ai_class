use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use tokio::fs;

use super::{Tool, ToolSpec};

fn glob_match(pattern: &str, path: &str) -> bool {
    let mut pi = 0;
    let mut si = 0;
    let mut star_idx = None;
    let mut match_idx = 0;
    let pattern_bytes = pattern.as_bytes();
    let path_bytes = path.as_bytes();

    while si < path_bytes.len() {
        if pi < pattern_bytes.len() && pattern_bytes[pi] == b'*' {
            star_idx = Some(pi);
            match_idx = si;
            pi += 1;
        } else if pi < pattern_bytes.len() && pattern_bytes[pi] == b'?' {
            pi += 1;
            si += 1;
        } else if pi < pattern_bytes.len() && pattern_bytes[pi] == path_bytes[si] {
            pi += 1;
            si += 1;
        } else if let Some(si_val) = star_idx {
            pi = si_val + 1;
            match_idx += 1;
            si = match_idx;
        } else {
            return false;
        }
    }

    while pi < pattern_bytes.len() && pattern_bytes[pi] == b'*' {
        pi += 1;
    }

    pi == pattern_bytes.len()
}

pub struct GrepTool;

impl GrepTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "grep".into(),
            description: "Search for a regex pattern in files. Uses .gitignore-aware file search."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in"
                    },
                    "include": {
                        "type": "string",
                        "description": "File pattern to filter (e.g. '*.rs')"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let pattern = args["pattern"].as_str().unwrap_or("");
        let search_path = args["path"].as_str().unwrap_or(".");
        let include = args["include"].as_str();

        if pattern.is_empty() {
            return Err(anyhow::anyhow!("pattern is required"));
        }

        let regex = Regex::new(pattern)?;
        let mut results = Vec::new();

        let walker = ignore::WalkBuilder::new(search_path)
            .standard_filters(true)
            .build();

        for entry in walker {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type().map_or(false, |f| f.is_file()) {
                continue;
            }
            if let Some(include_pat) = include {
                if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
                    if !glob_match(include_pat, fname) {
                        continue;
                    }
                }
            }

            match fs::read_to_string(path).await {
                Ok(content) => {
                    for (line_num, line) in content.lines().enumerate() {
                        if regex.is_match(line) {
                            results.push(format!(
                                "{}:{}: {}",
                                path.display(),
                                line_num + 1,
                                line.trim()
                            ));
                        }
                    }
                }
                Err(_) => continue,
            }
        }

        if results.is_empty() {
            Ok("No matches found.".into())
        } else {
            Ok(results.join("\n"))
        }
    }
}
