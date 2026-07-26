use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;

use crate::agent::{AgentConfig, AgentLoop};
use crate::teams::TeamConfig;
use crate::tools::{registry::ToolRegistry, Tool, ToolSpec};

const SUBAGENT_BASE_PROMPT: &str = r#"You are an AI coding agent working on a specific task. You have access to the following tools:

read - Read a file. Required: file_path.
bash - Execute a shell command. Required: command.
glob - Find files by glob pattern. Required: pattern.
grep - Search file contents by regex. Required: pattern.
fetch - Fetch a URL via HTTP GET. Required: url.

Rules:
- Always use absolute paths for file operations.
- Always respond in English.
- If a tool returns an error, fix the arguments and retry.
- Do not call a tool name that is not in the list above.
- Be concise and direct.
- You CANNOT write files. Return your findings to the team lead who can write them.
"#;

pub struct SubAgentTool {
    config: AgentConfig,
    active_team: Arc<RwLock<Option<TeamConfig>>>,
}

impl SubAgentTool {
    pub fn new(config: AgentConfig, active_team: Arc<RwLock<Option<TeamConfig>>>) -> Self {
        Self {
            config,
            active_team,
        }
    }
}

#[async_trait]
impl Tool for SubAgentTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "subagent".into(),
            description:
                "Spawn a sub-agent to work on an independent task. The sub-agent has access to all tools except write. Use for scouting, research, or complex multi-step sub-tasks that would clutter the main conversation. When part of a team, prefix the task with 'role:' to assign it to that role (e.g. 'plan: design the architecture'). Returns the sub-agent's final text response."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task for the sub-agent. When a team is active, prefix with 'role_name:' to route to a specific team member (e.g. 'plan:', 'backend:', 'frontend:', 'read_code:')."
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

        let mut sub_config = self.config.clone();
        let mut sub_task = task.to_string();

        if let Ok(guard) = self.active_team.read() {
            if let Some(ref team) = *guard {
                for role in &team.roles {
                    if let Some(stripped) = sub_task.strip_prefix(&format!("{}:", role.name)) {
                        let mut base = SUBAGENT_BASE_PROMPT.to_string();

                        if let Some(ref slug) = role.expert {
                            if let Some(profile) = crate::experts::load_profile(slug) {
                                base = format!(
                                    "{}\n\n--- Expert profile: {} ---\n{}",
                                    base, profile.name, profile.prompt
                                );
                            }
                        }

                        if !role.instructions.is_empty() {
                            sub_config.system_prompt = format!(
                                "{}\n\n--- Team role: {} ---\n{}",
                                base, role.name, role.instructions
                            );
                        } else {
                            sub_config.system_prompt = base;
                        }

                        sub_task = stripped.trim().to_string();
                        break;
                    }
                }
            }
        }

        let mut sub_registry = ToolRegistry::new();
        sub_registry
            .register(crate::tools::read::ReadTool::new())
            .register(crate::tools::bash::BashTool::new())
            .register(crate::tools::glob::GlobTool::new())
            .register(crate::tools::grep::GrepTool::new())
            .register(crate::tools::fetch::FetchTool::new());
        let sub_registry = Arc::new(sub_registry);

        let mut sub_agent = AgentLoop::new_subagent(sub_config, &sub_registry);
        sub_agent.add_user_message(sub_task);

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
