//! OpenRouter provider — OpenAI-compatible API supporting 200+ models.
//!
//! Translates `ApiRequest` to OpenAI ChatCompletion format and parses the response
//! back into `AssistantEvent`s.

use std::env;

use serde::{Deserialize, Serialize};
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ContentBlock, MessageRole, RuntimeError, TokenUsage,
};

use crate::config::ProviderEntry;

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

pub struct OpenRouterProvider {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl OpenRouterProvider {
    /// Create from a config entry.
    ///
    /// # Errors
    /// Returns `RuntimeError` if the API key env var is not set.
    pub fn new(entry: &ProviderEntry) -> Result<Self, RuntimeError> {
        let env_var = entry
            .api_key_env
            .as_deref()
            .unwrap_or("OPENROUTER_API_KEY");
        let api_key = env::var(env_var)
            .map_err(|_| RuntimeError::new(format!("{env_var} not set")))?;

        Ok(Self {
            http: reqwest::Client::new(),
            api_key,
            base_url: entry
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model: entry.model.clone(),
        })
    }

    fn convert_messages(request: &ApiRequest) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        // System prompt
        if !request.system_prompt.is_empty() {
            messages.push(ChatMessage {
                role: "system".to_string(),
                content: Some(request.system_prompt.join("\n\n")),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        for msg in &request.messages {
            match msg.role {
                MessageRole::System => {
                    for block in &msg.blocks {
                        if let ContentBlock::Text { text } = block {
                            messages.push(ChatMessage {
                                role: "system".to_string(),
                                content: Some(text.clone()),
                                tool_calls: None,
                                tool_call_id: None,
                            });
                        }
                    }
                }
                MessageRole::User => {
                    let text: String = msg
                        .blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    // Also handle tool results (sent as user messages in OpenAI format)
                    for block in &msg.blocks {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            output,
                            ..
                        } = block
                        {
                            messages.push(ChatMessage {
                                role: "tool".to_string(),
                                content: Some(output.clone()),
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id.clone()),
                            });
                        }
                    }

                    if !text.is_empty() {
                        messages.push(ChatMessage {
                            role: "user".to_string(),
                            content: Some(text),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }
                MessageRole::Assistant => {
                    let text: String = msg
                        .blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");

                    let tool_calls: Vec<ToolCall> = msg
                        .blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::ToolUse { id, name, input } => Some(ToolCall {
                                id: id.clone(),
                                r#type: "function".to_string(),
                                function: FunctionCall {
                                    name: name.clone(),
                                    arguments: input.clone(),
                                },
                            }),
                            _ => None,
                        })
                        .collect();

                    messages.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: if text.is_empty() { None } else { Some(text) },
                        tool_calls: if tool_calls.is_empty() {
                            None
                        } else {
                            Some(tool_calls)
                        },
                        tool_call_id: None,
                    });
                }
                MessageRole::Tool => {
                    for block in &msg.blocks {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            output,
                            ..
                        } = block
                        {
                            messages.push(ChatMessage {
                                role: "tool".to_string(),
                                content: Some(output.clone()),
                                tool_calls: None,
                                tool_call_id: Some(tool_use_id.clone()),
                            });
                        }
                    }
                }
            }
        }

        messages
    }
}

impl ApiClient for OpenRouterProvider {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let messages = Self::convert_messages(&request);

        let body = ChatCompletionRequest {
            model: self.model.clone(),
            messages,
            stream: false,
            max_tokens: Some(16384),
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| RuntimeError::new(format!("tokio: {e}")))?;

        let response = rt.block_on(async {
            self.http
                .post(format!("{}/chat/completions", self.base_url))
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("HTTP-Referer", "https://clawcode.dev")
                .header("X-Title", "ClawCode")
                .json(&body)
                .send()
                .await
                .map_err(|e| RuntimeError::new(format!("OpenRouter request: {e}")))?
                .json::<ChatCompletionResponse>()
                .await
                .map_err(|e| RuntimeError::new(format!("OpenRouter parse: {e}")))
        })?;

        let mut events = Vec::new();

        if let Some(choice) = response.choices.first() {
            if let Some(content) = &choice.message.content {
                events.push(AssistantEvent::TextDelta(content.clone()));
            }
            if let Some(tool_calls) = &choice.message.tool_calls {
                for tc in tool_calls {
                    events.push(AssistantEvent::ToolUse {
                        id: tc.id.clone(),
                        name: tc.function.name.clone(),
                        input: tc.function.arguments.clone(),
                    });
                }
            }
        }

        if let Some(usage) = response.usage {
            events.push(AssistantEvent::Usage(TokenUsage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            }));
        }

        events.push(AssistantEvent::MessageStop);
        Ok(events)
    }
}

// ── OpenAI-compatible wire types ──

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ToolCall {
    id: String,
    r#type: String,
    function: FunctionCall,
}

#[derive(Serialize, Deserialize)]
struct FunctionCall {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Deserialize)]
struct ChatUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
}
