use anyhow::Context;
use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolSpec};

pub struct FetchTool;

impl FetchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for FetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "fetch".into(),
            description:
                "Fetch a URL via HTTP GET. Returns status code, content type, and response body. Use to look up documentation, API responses, or any web content."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch (must be absolute, e.g. https://example.com/api)"
                    }
                },
                "required": ["url"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let url = args["url"].as_str().unwrap_or("");

        if url.is_empty() {
            return Err(anyhow::anyhow!("url is required"));
        }

        let response = reqwest::get(url)
            .await
            .with_context(|| format!("Failed to fetch {}", url))?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        let body = response
            .text()
            .await
            .with_context(|| format!("Failed to read response body from {}", url))?;

        let truncated = if body.len() > 8000 {
            format!(
                "{}...\n[truncated {} total bytes]",
                &body[..8000],
                body.len()
            )
        } else {
            body
        };

        Ok(format!(
            "Status: {}\nContent-Type: {}\n\n{}",
            status, content_type, truncated
        ))
    }
}
