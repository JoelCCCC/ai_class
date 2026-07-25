use std::time::Duration;

use anyhow::Context;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDelta {
    #[serde(skip)]
    pub index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn to_request_value(&self) -> serde_json::Value {
        let mut msg = serde_json::json!({
            "role": self.role,
        });
        if let Some(ref content) = self.content {
            msg["content"] = serde_json::Value::String(content.clone());
        } else {
            msg["content"] = serde_json::Value::Null;
        }
        if let Some(ref tcs) = self.tool_calls {
            let calls: Vec<serde_json::Value> = tcs
                .iter()
                .map(|tc| {
                    let tc_id = tc.id.clone().unwrap_or_default();
                    let tc_name = tc.name.clone().unwrap_or_default();
                    serde_json::json!({
                        "id": tc_id,
                        "type": "function",
                        "function": {
                            "name": tc_name,
                            "arguments": tc.arguments
                        }
                    })
                })
                .collect();
            msg["tool_calls"] = serde_json::Value::Array(calls);
        }
        if let Some(ref tcid) = self.tool_call_id {
            msg["tool_call_id"] = serde_json::Value::String(tcid.clone());
        }
        msg
    }
}

#[derive(Debug, Clone)]
pub struct StreamEvent {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCallDelta>>,
    pub finished: bool,
}

pub struct LlmClient {
    api_key: String,
    model: String,
    base_url: String,
    max_tokens: u32,
    client: reqwest::Client,
}

impl LlmClient {
    pub fn new(api_key: String, model: String, base_url: String, max_tokens: u32) -> Self {
        Self {
            api_key,
            model,
            base_url,
            max_tokens,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .unwrap(),
        }
    }

    pub async fn stream_chat(
        &self,
        messages: &[Message],
        tools: &[crate::tools::ToolSpec],
        system_prompt: &str,
    ) -> anyhow::Result<
        std::pin::Pin<Box<dyn futures::Stream<Item = anyhow::Result<StreamEvent>> + Send>>,
    > {
        let mut full_messages: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "system",
            "content": system_prompt
        })];

        for msg in messages {
            full_messages.push(msg.to_request_value());
        }

        let mut request_body = serde_json::json!({
            "model": self.model,
            "messages": full_messages,
            "tools": tools.iter().map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    }
                })
            }).collect::<Vec<_>>(),
            "stream": true,
        });

        if self.max_tokens > 0 {
            request_body["max_tokens"] = serde_json::json!(self.max_tokens);
        }

        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .context("Failed to send request to LLM API")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("API error {}: {}", status, body));
        }

        let (event_tx, event_rx) = mpsc::unbounded_channel::<anyhow::Result<StreamEvent>>();
        let mut byte_stream = response.bytes_stream();

        tokio::spawn(async move {
            let mut buffer = String::new();
            while let Some(chunk_result) = byte_stream.next().await {
                let bytes = match chunk_result {
                    Ok(b) => b,
                    Err(e) => {
                        let _ = event_tx.send(Err(anyhow::anyhow!("Stream error: {}", e)));
                        return;
                    }
                };
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() || !trimmed.starts_with("data: ") {
                        continue;
                    }
                    let data = &trimmed[6..];
                    if data == "[DONE]" {
                        let _ = event_tx.send(Ok(StreamEvent {
                            content: None,
                            tool_calls: None,
                            finished: true,
                        }));
                        return;
                    }

                    let event_json = match serde_json::from_str::<serde_json::Value>(data) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let mut content_delta: Option<String> = None;
                    let mut tool_calls: Option<Vec<ToolCallDelta>> = None;
                    let mut finished = false;

                    if let Some(choices) = event_json["choices"].as_array() {
                        for choice in choices {
                            if choice["finish_reason"]
                                .as_str()
                                .is_some_and(|r| r == "stop")
                            {
                                finished = true;
                            }

                            let delta = &choice["delta"];

                            if let Some(c) = delta["content"].as_str() {
                                content_delta = Some(c.to_string());
                            }

                            if let Some(tc_deltas) = delta["tool_calls"].as_array() {
                                for tc in tc_deltas {
                                    if tool_calls.is_none() {
                                        tool_calls = Some(Vec::new());
                                    }
                                    let index = tc["index"].as_u64().unwrap_or(0) as usize;

                                    if let Some(tc_list) = &mut tool_calls {
                                        while tc_list.len() <= index {
                                            tc_list.push(ToolCallDelta {
                                                index: Some(tc_list.len()),
                                                id: None,
                                                name: None,
                                                arguments: String::new(),
                                            });
                                        }

                                        tc_list[index].index = Some(index);
                                        if let Some(id) = tc["id"].as_str() {
                                            tc_list[index].id = Some(id.to_string());
                                        }
                                        if let Some(fn_name) = tc["function"]["name"].as_str() {
                                            tc_list[index].name = Some(fn_name.to_string());
                                        }
                                        if let Some(args) = tc["function"]["arguments"].as_str() {
                                            tc_list[index].arguments.push_str(args);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let _ = event_tx.send(Ok(StreamEvent {
                        content: content_delta,
                        tool_calls,
                        finished,
                    }));
                }
            }
        });

        Ok(Box::pin(futures::stream::unfold(
            event_rx,
            |mut rx| async move { rx.recv().await.map(|e| (e, rx)) },
        )))
    }
}
