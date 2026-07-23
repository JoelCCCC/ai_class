use async_trait::async_trait;
use serde_json::json;
use tokio::fs;

use super::{Tool, ToolSpec};

pub struct ReadTool;

impl ReadTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "read".into(),
            description: "Read a file from the filesystem. Returns the file contents with line numbers.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line number to start reading from (1-indexed)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of lines to read"
                    }
                },
                "required": ["file_path"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let path = args["file_path"].as_str().unwrap_or("");
        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = args["limit"].as_u64().map(|l| l as usize);

        if path.is_empty() {
            return Err(anyhow::anyhow!("file_path is required"));
        }

        let content = fs::read_to_string(path).await?;
        let lines: Vec<&str> = content.lines().collect();

        let start = offset.saturating_sub(1);
        let end = limit.map_or(lines.len(), |l| (start + l).min(lines.len()));

        let mut result = String::new();
        for (i, line) in lines[start..end].iter().enumerate() {
            result.push_str(&format!("{:6}: {}\n", start + i + 1, line));
        }

        Ok(result)
    }
}
