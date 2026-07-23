use std::collections::HashMap;
use std::sync::Arc;

use super::{Tool, ToolCall, ToolResult};

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) -> &mut Self {
        let spec = tool.spec();
        self.tools.insert(spec.name, Arc::new(tool));
        self
    }

    pub fn get_specs(&self) -> Vec<super::ToolSpec> {
        self.tools.values().map(|t| t.spec()).collect()
    }

    pub async fn execute(&self, call: &ToolCall) -> ToolResult {
        match self.tools.get(&call.name) {
            Some(tool) => match tool.execute(call.arguments.clone()).await {
                Ok(content) => ToolResult {
                    tool_call_id: call.id.clone(),
                    content,
                    is_error: false,
                },
                Err(e) => ToolResult {
                    tool_call_id: call.id.clone(),
                    content: format!("Error: {}", e),
                    is_error: true,
                },
            },
            None => ToolResult {
                tool_call_id: call.id.clone(),
                content: format!("Unknown tool: {}", call.name),
                is_error: true,
            },
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
