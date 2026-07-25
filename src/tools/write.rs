use anyhow::Context;
use async_trait::async_trait;
use serde_json::json;
use tokio::fs;

use super::{Tool, ToolSpec};

pub struct WriteTool;

impl WriteTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "write".into(),
            description: "Write content to a file. Required: file_path (absolute path), content (string to write). Creates parent dirs.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["file_path", "content"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let path = args["file_path"].as_str().unwrap_or("");
        let content = args["content"].as_str().unwrap_or("");

        if path.is_empty() {
            return Err(anyhow::anyhow!("file_path is required"));
        }

        if let Some(parent) = std::path::Path::new(path).parent() {
            fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Failed to create directory for {}", path))?;
        }

        fs::write(path, content)
            .await
            .with_context(|| format!("Failed to write {}", path))?;

        Ok(format!("Successfully wrote file: {}", path))
    }
}
