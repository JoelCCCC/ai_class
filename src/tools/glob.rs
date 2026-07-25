use async_trait::async_trait;
use serde_json::json;

use super::glob_match::glob_match;
use super::{Tool, ToolSpec};

pub struct GlobTool;

impl GlobTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "glob".into(),
            description:
                "Find files by glob pattern. Required: pattern (e.g. '**/*.rs'). Optional: path (directory). .gitignore-aware."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match (e.g. '**/*.rs')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in"
                    }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let pattern = args["pattern"].as_str().unwrap_or("");
        let search_path = args["path"].as_str().unwrap_or(".");

        if pattern.is_empty() {
            return Err(anyhow::anyhow!("pattern is required"));
        }

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
            if let Some(path_str) = path.to_str() {
                if glob_match(pattern, path_str) {
                    results.push(path_str.to_string());
                }
            }
        }

        if results.is_empty() {
            Ok("No matching files found.".into())
        } else {
            Ok(results.join("\n"))
        }
    }
}

#[cfg(test)]
#[test]
fn test_glob_match() {
    assert!(glob_match("*.rs", "main.rs"));
    assert!(!glob_match("*.rs", "main.py"));
    assert!(glob_match("src/**/*.rs", "src/foo/bar.rs"));
    assert!(!glob_match("src/**/*.rs", "tests/foo.rs"));
}
