use async_trait::async_trait;
use serde_json::json;

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
                "Find files matching a glob pattern. Uses .gitignore-aware search.".into(),
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
        let walker = ignore::WalkBuilder::new(search_path).standard_filters(true).build();

        for entry in walker {
            let entry = entry?;
            let path = entry.path();
            if !entry.file_type().map_or(false, |f| f.is_file()) {
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

pub(crate) fn glob_match(pattern: &str, path: &str) -> bool {
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

#[cfg(test)]
#[test]
fn test_glob_match() {
    assert!(glob_match("*.rs", "main.rs"));
    assert!(!glob_match("*.rs", "main.py"));
    assert!(glob_match("src/**/*.rs", "src/foo/bar.rs"));
    assert!(!glob_match("src/**/*.rs", "tests/foo.rs"));
}
