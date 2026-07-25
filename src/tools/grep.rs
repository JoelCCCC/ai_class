use async_trait::async_trait;
use regex::Regex;
use serde_json::json;
use tokio::fs;

use super::glob_match::glob_match;
use super::{Tool, ToolSpec};

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
            description: "Search file contents by regex. Required: pattern (regex string). Optional: path, include (file filter e.g. '*.rs'). .gitignore-aware."
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
            if !entry.file_type().is_some_and(|f| f.is_file()) {
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
