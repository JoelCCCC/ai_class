use anyhow::Context;
use futures::StreamExt;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDelta {
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
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
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
    client: reqwest::Client,
}

impl LlmClient {
    pub fn new(api_key: String, model: String, base_url: String) -> Self {
        Self {
            api_key,
            model,
            base_url,
            client: reqwest::Client::new(),
        }
    }

    pub async fn stream_chat(
        &self,
        messages: &[Message],
        tools: &[crate::tools::ToolSpec],
        system_prompt: &str,
    ) -> anyhow::Result<impl futures::Stream<Item = anyhow::Result<StreamEvent>>> {
        let mut full_messages: Vec<serde_json::Value> = vec![serde_json::json!({
            "role": "system",
            "content": system_prompt
        })];

        for msg in messages {
            full_messages.push(serde_json::json!(msg));
        }

        let request_body = serde_json::json!({
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
            "max_tokens": 8192
        });

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

        let stream = response
            .bytes_stream()
            .map(|chunk| -> anyhow::Result<StreamEvent> {
                let bytes = chunk?;
                let text = String::from_utf8_lossy(&bytes);

                let mut content_delta: Option<String> = None;
                let mut tool_calls = None;
                let mut finished = false;

                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || !line.starts_with("data: ") {
                        continue;
                    }

                    let data = &line[6..];
                    if data == "[DONE]" {
                        finished = true;
                        continue;
                    }

                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(choices) = event["choices"].as_array() {
                            for choice in choices {
                                if choice["finish_reason"].as_str().is_some_and(|r| r == "stop") {
                                    finished = true;
                                }

                                let delta = &choice["delta"];

                                if let Some(c) = delta["content"].as_str() {
                                    match &mut content_delta {
                                        Some(ref mut existing) => existing.push_str(c),
                                        None => content_delta = Some(c.to_string()),
                                    }
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
                                                    id: None,
                                                    name: None,
                                                    arguments: String::new(),
                                                });
                                            }

                                            if let Some(id) = tc["id"].as_str() {
                                                tc_list[index].id = Some(id.to_string());
                                            }
                                            if let Some(fn_name) =
                                                tc["function"]["name"].as_str()
                                            {
                                                tc_list[index].name = Some(fn_name.to_string());
                                            }
                                            if let Some(args) =
                                                tc["function"]["arguments"].as_str()
                                            {
                                                tc_list[index].arguments.push_str(args);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Ok(StreamEvent {
                    content: content_delta,
                    tool_calls,
                    finished,
                })
            });

        Ok(stream)
    }
}
