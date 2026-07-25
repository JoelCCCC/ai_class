use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;
use tokio::process::Command;

use super::{Tool, ToolSpec};

const BASH_TIMEOUT_SECS: u64 = 120;

pub struct BashTool;

impl BashTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "bash".into(),
            description:
                "Execute a shell command. Required: command (string). Optional: workdir (directory). Non-interactive, no stdin."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The bash command to execute"
                    },
                    "workdir": {
                        "type": "string",
                        "description": "Optional working directory for the command"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let cmd = args["command"].as_str().unwrap_or("");
        let workdir = args["workdir"].as_str().unwrap_or(".");

        if cmd.is_empty() {
            return Err(anyhow::anyhow!("command is required"));
        }

        let mut child = if cfg!(target_os = "windows") {
            let mut c = Command::new("cmd");
            c.args(["/C", cmd]);
            c
        } else {
            let mut c = Command::new("bash");
            c.args(["-c", cmd]);
            c
        };

        child.current_dir(workdir);
        let output = tokio::time::timeout(Duration::from_secs(BASH_TIMEOUT_SECS), child.output())
            .await
            .map_err(|_| {
                anyhow::anyhow!(format!("Command timed out after {}s", BASH_TIMEOUT_SECS))
            })??;

        let mut result = String::new();
        if !output.stdout.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            result.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        if result.is_empty() {
            result.push_str(&format!(
                "Command exited with code: {:?}",
                output.status.code()
            ));
        }

        Ok(result)
    }
}
