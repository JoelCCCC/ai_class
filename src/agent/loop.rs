use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use crate::llm::{LlmClient, Message, ToolCallDelta};
use crate::tools::{registry::ToolRegistry, ToolCall, ToolResult};

const MAX_TOOL_LOOP: usize = 25;

#[derive(Clone)]
pub struct AgentConfig {
    pub system_prompt: String,
    pub model: String,
    pub api_key: String,
    pub base_url: String,
    pub max_tokens: u32,
}

pub enum AgentEvent {
    AssistantDelta(String),
    ToolCallStart {
        name: String,
        _id: String,
    },
    ToolCallEnd(ToolResult),
    Thinking,
    Done,
    Error(String),
    WriteRequest {
        path: String,
        resume: oneshot::Sender<bool>,
    },
}

pub struct AgentLoop {
    client: LlmClient,
    registry: Arc<ToolRegistry>,
    pub config: AgentConfig,
    messages: Vec<Message>,
    deny_writes: bool,
}

impl AgentLoop {
    pub fn new(config: AgentConfig, registry: Arc<ToolRegistry>) -> Self {
        Self {
            client: LlmClient::new(
                config.api_key.clone(),
                config.model.clone(),
                config.base_url.clone(),
                config.max_tokens,
            ),
            registry,
            config,
            messages: Vec::new(),
            deny_writes: false,
        }
    }

    pub fn new_subagent(config: AgentConfig, registry: &Arc<ToolRegistry>) -> Self {
        Self {
            client: LlmClient::new(
                config.api_key.clone(),
                config.model.clone(),
                config.base_url.clone(),
                config.max_tokens,
            ),
            registry: Arc::clone(registry),
            config,
            messages: Vec::new(),
            deny_writes: true,
        }
    }

    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(Message {
            role: "user".into(),
            content: Some(content),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn save_to_file(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.messages)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load_from_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let data = std::fs::read_to_string(path)?;
        self.messages = serde_json::from_str(&data)?;
        Ok(())
    }

    pub fn update_model(&mut self, model: String) {
        self.config.model = model.clone();
        self.client = LlmClient::new(
            self.config.api_key.clone(),
            model,
            self.config.base_url.clone(),
            self.config.max_tokens,
        );
    }

    pub fn update_url(&mut self, url: String) {
        self.config.base_url = url.clone();
        self.client = LlmClient::new(
            self.config.api_key.clone(),
            self.config.model.clone(),
            url,
            self.config.max_tokens,
        );
    }

    pub async fn run(&mut self, tx: mpsc::UnboundedSender<AgentEvent>) -> anyhow::Result<String> {
        let tools = self.registry.get_specs();
        let mut final_response = String::new();
        let mut loop_count = 0;

        loop {
            loop_count += 1;
            if loop_count > MAX_TOOL_LOOP {
                let _ = tx.send(AgentEvent::Error("Max tool loop exceeded".into()));
                break;
            }

            let _ = tx.send(AgentEvent::Thinking);

            let mut stream = self
                .client
                .stream_chat(&self.messages, &tools, &self.config.system_prompt)
                .await?;

            let mut assistant_content = String::new();
            let mut assistant_tool_calls: Vec<ToolCallDelta> = Vec::new();

            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(event) => {
                        if let Some(content) = &event.content {
                            assistant_content.push_str(content);
                            let _ = tx.send(AgentEvent::AssistantDelta(content.clone()));
                        }

                        if let Some(tc_deltas) = &event.tool_calls {
                            for tc in tc_deltas {
                                let index = tc.index.unwrap_or(assistant_tool_calls.len());
                                while assistant_tool_calls.len() <= index {
                                    assistant_tool_calls.push(ToolCallDelta {
                                        index: Some(assistant_tool_calls.len()),
                                        id: None,
                                        name: None,
                                        arguments: String::new(),
                                    });
                                }
                                assistant_tool_calls[index]
                                    .arguments
                                    .push_str(&tc.arguments);
                                if let Some(id) = &tc.id {
                                    assistant_tool_calls[index].id = Some(id.clone());
                                }
                                if let Some(name) = &tc.name {
                                    assistant_tool_calls[index].name = Some(name.clone());
                                }
                            }
                        }

                        if event.finished {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(AgentEvent::Error(e.to_string()));
                        return Err(anyhow::anyhow!("Stream error"));
                    }
                }
            }

            if assistant_tool_calls.is_empty() {
                final_response = assistant_content;
                if !final_response.is_empty() {
                    self.messages.push(Message {
                        role: "assistant".into(),
                        content: Some(final_response.clone()),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                let _ = tx.send(AgentEvent::Done);
                break;
            }

            for tc in &mut assistant_tool_calls {
                if tc.id.is_none() || tc.id.as_deref() == Some("") {
                    tc.id = Some(uuid::Uuid::new_v4().to_string());
                }
            }

            self.messages.push(Message {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(assistant_tool_calls.clone()),
                tool_call_id: None,
            });

            let tool_names: Vec<String> = self
                .registry
                .get_specs()
                .iter()
                .map(|s| s.name.clone())
                .collect();

            let mut tool_results = Vec::new();

            for tc_delta in &assistant_tool_calls {
                let id = tc_delta.id.clone().unwrap_or_default();
                let name = tc_delta.name.clone().unwrap_or_default();

                if name.is_empty() || !tool_names.contains(&name) {
                    let msg = if name.is_empty() {
                        "tool name is empty".to_string()
                    } else {
                        format!("unknown tool '{}'", name)
                    };
                    let result = ToolResult {
                        tool_call_id: id,
                        content: format!(
                            "Error: {}. Available tools: {}.",
                            msg,
                            tool_names.join(", ")
                        ),
                        is_error: true,
                    };
                    let _ = tx.send(AgentEvent::ToolCallEnd(result.clone()));
                    tool_results.push(result);
                    continue;
                }

                let arguments: serde_json::Value = match serde_json::from_str(&tc_delta.arguments) {
                    Ok(v) => v,
                    Err(e) => {
                        let result = ToolResult {
                                tool_call_id: id.clone(),
                                content: format!(
                                    "Error: failed to parse tool arguments as JSON: {}. Arguments received: {}",
                                    e, tc_delta.arguments
                                ),
                                is_error: true,
                            };
                        let _ = tx.send(AgentEvent::ToolCallEnd(result.clone()));
                        tool_results.push(result);
                        continue;
                    }
                };

                if name == "write" {
                    let file_path = arguments["file_path"].as_str().unwrap_or("unknown");
                    if self.deny_writes {
                        let result = ToolResult {
                            tool_call_id: id.clone(),
                            content: format!(
                                "Write denied: cannot write to {} (writes are disabled for sub-agents). Request the parent agent to perform the write instead.",
                                file_path
                            ),
                            is_error: true,
                        };
                        let _ = tx.send(AgentEvent::ToolCallEnd(result.clone()));
                        tool_results.push(result);
                        continue;
                    }
                    let (resume_tx, resume_rx) = oneshot::channel();
                    let _ = tx.send(AgentEvent::WriteRequest {
                        path: file_path.to_string(),
                        resume: resume_tx,
                    });
                    let approved = resume_rx.await.unwrap_or(false);
                    if !approved {
                        let result = ToolResult {
                            tool_call_id: id.clone(),
                            content: format!("Write denied by user: {}", file_path),
                            is_error: true,
                        };
                        let _ = tx.send(AgentEvent::ToolCallEnd(result.clone()));
                        tool_results.push(result);
                        continue;
                    }
                }

                let _ = tx.send(AgentEvent::ToolCallStart {
                    name: name.clone(),
                    _id: id.clone(),
                });

                let call = ToolCall {
                    id: id.clone(),
                    name: name.clone(),
                    arguments,
                };

                let result = self.registry.execute(&call).await;
                let _ = tx.send(AgentEvent::ToolCallEnd(result.clone()));
                tool_results.push(result);
            }

            for result in &tool_results {
                self.messages.push(Message {
                    role: "tool".into(),
                    content: Some(result.content.clone()),
                    tool_calls: None,
                    tool_call_id: Some(result.tool_call_id.clone()),
                });
            }
        }

        Ok(final_response)
    }
}
