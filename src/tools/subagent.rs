use std::sync::{Arc, Weak};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::agent::{AgentConfig, AgentLoop};
use crate::tools::{registry::ToolRegistry, Tool, ToolSpec};

pub struct SubAgentTool {
    config: AgentConfig,
    registry: Weak<ToolRegistry>,
}

impl SubAgentTool {
    pub fn new(config: AgentConfig, registry: &Arc<ToolRegistry>) -> Self {
        Self {
            config,
            registry: Arc::downgrade(registry),
        }
    }
}

#[async_trait]
impl Tool for SubAgentTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "subagent".into(),
            description:
                "Spawn a sub-agent to work on an independent task. The sub-agent has access to read, glob, grep, fetch, and bash tools. It cannot write files. Use for scouting, research, or complex multi-step sub-tasks that would clutter the main conversation. Returns the sub-agent's final text response.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task for the sub-agent. Be specific about what to investigate, read, or analyze."
                    }
                },
                "required": ["task"]
            }),
        }
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let task = args["task"].as_str().unwrap_or("");

        if task.is_empty() {
            return Err(anyhow::anyhow!("task is required for subagent"));
        }

        let registry = self
            .registry
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("Tool registry no longer available"))?;

        let mut sub_agent = AgentLoop::new_subagent(self.config.clone(), &registry);
        sub_agent.add_user_message(task.to_string());

        let (tx, mut rx) = mpsc::unbounded_channel();

        let agent_handle = tokio::spawn(async move { sub_agent.run(tx).await });

        tokio::spawn(async move { while rx.recv().await.is_some() {} });

        match agent_handle.await {
            Ok(Ok(response)) => {
                if response.is_empty() {
                    Ok("(sub-agent completed with no output)".into())
                } else {
                    Ok(response)
                }
            }
            Ok(Err(e)) => Err(anyhow::anyhow!("Sub-agent error: {}", e)),
            Err(join_err) => Err(anyhow::anyhow!("Sub-agent task failed: {}", join_err)),
        }
    }
}
