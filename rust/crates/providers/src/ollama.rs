//! Ollama provider — local models via the Ollama REST API.
//!
//! Uses `/api/chat` endpoint with NDJSON streaming.

use serde::{Deserialize, Serialize};

use runtime::{
    ApiClient, ApiRequest, AssistantEvent, ContentBlock, MessageRole, RuntimeError, TokenUsage,
};

use crate::config::ProviderEntry;

const DEFAULT_BASE_URL: &str = "http://localhost:11434";

pub struct OllamaProvider {
    http: reqwest::blocking::Client,
    base_url: String,
    model: String,
}

impl OllamaProvider {
    /// Create from a config entry.
    ///
    /// # Errors
    /// Returns `RuntimeError` if the client cannot be created.
    pub fn new(entry: &ProviderEntry) -> Result<Self, RuntimeError> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| RuntimeError::new(format!("HTTP client: {e}")))?,
            base_url: entry
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            model: entry.model.clone(),
        })
    }

    fn convert_messages(request: &ApiRequest) -> Vec<OllamaMessage> {
        let mut messages = Vec::new();

        // System prompt
        if !request.system_prompt.is_empty() {
            messages.push(OllamaMessage {
                role: "system".to_string(),
                content: request.system_prompt.join("\n\n"),
            });
        }

        for msg in &request.messages {
            let role = match msg.role {
                MessageRole::System => "system",
                MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
            };

            let text: String = msg
                .blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    ContentBlock::ToolResult { output, .. } => Some(output.as_str()),
                    ContentBlock::ToolUse { input, .. } => Some(input.as_str()),
                })
                .collect::<Vec<_>>()
                .join("\n");

            if !text.is_empty() {
                messages.push(OllamaMessage {
                    role: role.to_string(),
                    content: text,
                });
            }
        }

        messages
    }
}

impl ApiClient for OllamaProvider {
    fn stream(&mut self, request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        let messages = Self::convert_messages(&request);

        let body = OllamaChatRequest {
            model: self.model.clone(),
            messages,
            stream: false,
            options: OllamaOptions {
                temperature: 0.4,
                num_ctx: 32768,
            },
        };

        let resp = self
            .http
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .map_err(|e| {
                RuntimeError::new(format!(
                    "Ollama not reachable at {}: {e}",
                    self.base_url
                ))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().unwrap_or_default();
            return Err(RuntimeError::new(format!(
                "Ollama HTTP {status}: {text}"
            )));
        }

        let response = resp
            .json::<OllamaChatResponse>()
            .map_err(|e| RuntimeError::new(format!("Ollama parse: {e}")))?;

        let mut events = Vec::new();

        let content = response
            .message
            .map(|m| m.content)
            .unwrap_or_default();
        if !content.is_empty() {
            events.push(AssistantEvent::TextDelta(content));
        }

        events.push(AssistantEvent::Usage(TokenUsage {
            input_tokens: response.prompt_eval_count.unwrap_or(0),
            output_tokens: response.eval_count.unwrap_or(0),
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        }));

        events.push(AssistantEvent::MessageStop);
        Ok(events)
    }
}

// ── Ollama wire types ──

#[derive(Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaOptions {
    temperature: f32,
    num_ctx: u32,
}

#[derive(Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaResponseMessage>,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

#[derive(Deserialize)]
struct OllamaResponseMessage {
    content: String,
}
