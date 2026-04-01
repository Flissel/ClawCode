//! Adapter wrapping the existing `api::AnthropicClient` behind the `runtime::ApiClient` trait.

use api::{
    AnthropicClient, InputContentBlock, InputMessage, MessageRequest, OutputContentBlock,
    ToolResultContentBlock,
};

use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ConversationMessage, ContentBlock, MessageRole,
    RuntimeError, TokenUsage,
};

/// Wraps `AnthropicClient` to implement the runtime `ApiClient` trait.
pub struct AnthropicAdapter {
    client: AnthropicClient,
    model: String,
    max_tokens: u32,
}

impl AnthropicAdapter {
    /// Create from environment variables.
    ///
    /// # Errors
    /// Returns `RuntimeError` if `ANTHROPIC_API_KEY` is not set.
    pub fn from_env(model: &str) -> Result<Self, RuntimeError> {
        let client =
            AnthropicClient::from_env().map_err(|e| RuntimeError::new(format!("Anthropic: {e}")))?;
        Ok(Self {
            client,
            model: model.to_string(),
            max_tokens: 16384,
        })
    }

    fn convert_messages(messages: &[ConversationMessage]) -> Vec<InputMessage> {
        messages
            .iter()
            .filter_map(|msg| {
                let role = match msg.role {
                    MessageRole::User | MessageRole::Tool => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::System => return None, // handled via system_prompt
                };

                let content: Vec<InputContentBlock> = msg
                    .blocks
                    .iter()
                    .map(|block| match block {
                        ContentBlock::Text { text } => InputContentBlock::Text {
                            text: text.clone(),
                        },
                        ContentBlock::ToolUse { id, name, input } => InputContentBlock::ToolUse {
                            id: id.clone(),
                            name: name.clone(),
                            input: serde_json::from_str(input).unwrap_or_default(),
                        },
                        ContentBlock::ToolResult {
                            tool_use_id,
                            output,
                            is_error,
                            ..
                        } => InputContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: vec![ToolResultContentBlock::Text {
                                text: output.clone(),
                            }],
                            is_error: *is_error,
                        },
                    })
                    .collect();

                Some(InputMessage {
                    role: role.to_string(),
                    content,
                })
            })
            .collect()
    }
}

impl ApiClient for AnthropicAdapter {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let system = if request.system_prompt.is_empty() {
            None
        } else {
            Some(request.system_prompt.join("\n\n"))
        };

        let messages = Self::convert_messages(&request.messages);

        let msg_request = MessageRequest {
            model: self.model.clone(),
            max_tokens: self.max_tokens,
            messages,
            system,
            tools: None,
            tool_choice: None,
            stream: false,
        };

        // Run async in sync context — try existing runtime first, then create one
        let response = match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                // Already in a tokio runtime — use it via spawn_blocking trick
                std::thread::scope(|s| {
                    s.spawn(|| {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| RuntimeError::new(format!("tokio: {e}")))?;
                        rt.block_on(self.client.send_message(&msg_request))
                            .map_err(|e| RuntimeError::new(format!("Anthropic API: {e}")))
                    })
                    .join()
                    .unwrap()
                })?
            }
            Err(_) => {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| RuntimeError::new(format!("tokio: {e}")))?;
                rt.block_on(self.client.send_message(&msg_request))
                    .map_err(|e| RuntimeError::new(format!("Anthropic API: {e}")))?
            }
        };

        let mut events = Vec::new();

        for block in &response.content {
            match block {
                OutputContentBlock::Text { text } => {
                    events.push(AssistantEvent::TextDelta(text.clone()));
                }
                OutputContentBlock::ToolUse { id, name, input } => {
                    events.push(AssistantEvent::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.to_string(),
                    });
                }
            }
        }

        events.push(AssistantEvent::Usage(TokenUsage {
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            cache_creation_input_tokens: response.usage.cache_creation_input_tokens,
            cache_read_input_tokens: response.usage.cache_read_input_tokens,
        }));

        events.push(AssistantEvent::MessageStop);

        Ok(events)
    }
}
