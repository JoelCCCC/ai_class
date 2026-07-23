use std::sync::Arc;

use futures::StreamExt;
use tokio::sync::mpsc;

use crate::llm::{LlmClient, Message, ToolCallDelta};
use crate::tools::{registry::ToolRegistry, ToolCall, ToolResult};

const MAX_TOOL_LOOP: usize = 25;

#[derive(Clone)]
pub struct AgentConfig {
    pub system_prompt: String,
    pub model: String,
    pub api_key: String,
    pub base_url: String,
}

pub enum AgentEvent {
    AssistantDelta(String),
    ToolCallStart { name: String, _id: String },
    ToolCallEnd(ToolResult),
    Thinking,
    Done,
    Error(String),
}

pub struct AgentLoop {
    client: LlmClient,
    registry: Arc<ToolRegistry>,
    pub config: AgentConfig,
    messages: Vec<Message>,
}

impl AgentLoop {
    pub fn new(config: AgentConfig, registry: ToolRegistry) -> Self {
        Self {
            client: LlmClient::new(
                config.api_key.clone(),
                config.model.clone(),
                config.base_url.clone(),
            ),
            registry: Arc::new(registry),
            config,
            messages: Vec::new(),
        }
    }

    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(Message {
            role: "user".into(),
            content,
            tool_calls: None,
            tool_call_id: None,
        });
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
                                if let Some(idx) = assistant_tool_calls
                                    .iter()
                                    .position(|t| t.id == tc.id)
                                {
                                    assistant_tool_calls[idx]
                                        .arguments
                                        .push_str(&tc.arguments);
                                    if let Some(name) = &tc.name {
                                        assistant_tool_calls[idx].name = Some(name.clone());
                                    }
                                } else {
                                    assistant_tool_calls.push(tc.clone());
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
                        content: final_response.clone(),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
                let _ = tx.send(AgentEvent::Done);
                break;
            }

            self.messages.push(Message {
                role: "assistant".into(),
                content: assistant_content,
                tool_calls: Some(assistant_tool_calls.clone()),
                tool_call_id: None,
            });

            let mut tool_results = Vec::new();

            for tc_delta in &assistant_tool_calls {
                let id = tc_delta.id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let name = tc_delta.name.clone().unwrap_or_default();
                let arguments: serde_json::Value =
                    serde_json::from_str(&tc_delta.arguments).unwrap_or(serde_json::Value::Null);

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
                    content: result.content.clone(),
                    tool_calls: None,
                    tool_call_id: Some(result.tool_call_id.clone()),
                });
            }
        }

        Ok(final_response)
    }
}
