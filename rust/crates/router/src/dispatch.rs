//! Backend dispatcher — executes LLM calls via Claude, Kilo, or ClawCode CLIs.

use std::io::Write;
use std::process::Command;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::config::RouterConfig;

/// OpenAI-compatible chat completion request (subset).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    #[serde(default)]
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Agent role for routing (custom extension).
    #[serde(default)]
    pub agent_role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
}

/// OpenAI-compatible chat completion response.
#[derive(Debug, Clone, Serialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: ChatUsage,
    /// Extra: which backend handled this request.
    pub backend: String,
    /// Extra: latency in ms.
    pub latency_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    pub message: String,
    pub r#type: String,
}

/// Dispatch a request to the appropriate backend.
pub fn dispatch(config: &RouterConfig, request: &ChatRequest) -> Result<ChatResponse, String> {
    let agent_role = request.agent_role.as_deref().unwrap_or("default");
    let backend = request
        .model
        .clone()
        .unwrap_or_else(|| config.resolve_backend(agent_role));

    tracing::info!(agent_role, backend, "routing request");

    // Try primary backend, then fallback chain
    let result = try_backend(config, &backend, request);
    if result.is_ok() {
        return result;
    }

    let primary_error = result.unwrap_err();
    tracing::warn!(backend, error = %primary_error, "primary backend failed, trying fallbacks");

    for fallback in &config.fallback.chain {
        if fallback == &backend {
            continue; // skip the one that already failed
        }
        tracing::info!(fallback, "trying fallback backend");
        match try_backend(config, fallback, request) {
            Ok(mut resp) => {
                resp.backend = format!("{fallback} (fallback from {backend})");
                return Ok(resp);
            }
            Err(e) => {
                tracing::warn!(fallback, error = %e, "fallback also failed");
            }
        }
    }

    Err(format!(
        "All backends failed. Primary ({backend}): {primary_error}"
    ))
}

fn try_backend(
    config: &RouterConfig,
    backend: &str,
    request: &ChatRequest,
) -> Result<ChatResponse, String> {
    let prompt = build_prompt(&request.messages);
    let start = Instant::now();

    let (output, used_model) = if backend == "claude" {
        run_claude_cli(config, &prompt)?
    } else if backend == "kilo" {
        run_kilo_cli(config, &prompt)?
    } else if backend.starts_with("claw:") || backend.starts_with("clawcode:") {
        let provider_model = backend.split_once(':').map(|(_, r)| r).unwrap_or("");
        run_clawcode_cli(config, &prompt, provider_model)?
    } else {
        // Treat as clawcode with default provider
        run_clawcode_cli(config, &prompt, backend)?
    };

    let latency_ms = start.elapsed().as_millis() as u64;
    let word_count = output.split_whitespace().count() as u32;

    Ok(ChatResponse {
        id: format!("claw-{}", uuid_short()),
        object: "chat.completion".to_string(),
        model: used_model.clone(),
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some(output),
            },
            finish_reason: "stop".to_string(),
        }],
        usage: ChatUsage {
            prompt_tokens: prompt.len() as u32 / 4, // rough estimate
            completion_tokens: word_count * 2,       // rough estimate
            total_tokens: (prompt.len() as u32 / 4) + word_count * 2,
        },
        backend: backend.to_string(),
        latency_ms,
    })
}

fn build_prompt(messages: &[ChatMessage]) -> String {
    let mut parts = Vec::new();
    for msg in messages {
        let content = msg.content.as_deref().unwrap_or("");
        match msg.role.as_str() {
            "system" => parts.push(format!("<system>\n{content}\n</system>")),
            "user" => parts.push(content.to_string()),
            "assistant" => parts.push(format!("[Previous response]\n{content}")),
            _ => parts.push(content.to_string()),
        }
    }
    parts.join("\n\n")
}

fn run_claude_cli(config: &RouterConfig, prompt: &str) -> Result<(String, String), String> {
    let binary = config
        .backends
        .claude
        .binary
        .as_deref()
        .unwrap_or("claude");
    let model = config
        .backends
        .claude
        .model
        .as_deref()
        .unwrap_or("claude-sonnet-4-6");
    let timeout = config.backends.claude.timeout_secs;

    let mut cmd = Command::new(binary);
    cmd.args(["--model", model, "-p"]);
    run_cli_with_stdin(&mut cmd, prompt, timeout, model)
}

fn run_kilo_cli(config: &RouterConfig, prompt: &str) -> Result<(String, String), String> {
    let binary = config.backends.kilo.binary.as_deref().unwrap_or("kilo");
    let model = config
        .backends
        .kilo
        .model
        .as_deref()
        .unwrap_or("openai/gpt-5.4");
    let timeout = config.backends.kilo.timeout_secs;

    let mut cmd = Command::new(binary);
    cmd.args(["--model", model, "-p"]);
    run_cli_with_stdin(&mut cmd, prompt, timeout, model)
}

fn run_clawcode_cli(
    config: &RouterConfig,
    prompt: &str,
    provider_model: &str,
) -> Result<(String, String), String> {
    let binary = config
        .backends
        .claw
        .binary
        .as_deref()
        .unwrap_or("rusty-claude-cli");
    let timeout = config.backends.claw.timeout_secs;

    let mut cmd = Command::new(binary);

    // Parse "provider/model" or just "provider"
    if !provider_model.is_empty() {
        let (provider, model) = if let Some((p, m)) = provider_model.split_once('/') {
            (p, Some(m))
        } else {
            (provider_model, None)
        };
        cmd.args(["--provider", provider]);
        if let Some(m) = model {
            // Re-join for models with slashes like "qwen/qwen3.6:free"
            let full_model = if let Some(rest) = provider_model.strip_prefix(&format!("{provider}/"))
            {
                rest
            } else {
                provider_model
            };
            cmd.args(["--model", full_model]);
        }
    }

    if let Some(cfg) = &config.backends.claw.config_path {
        cmd.args(["--config", cfg]);
    }

    cmd.arg("prompt");
    run_cli_with_prompt_arg(&mut cmd, prompt, timeout, provider_model)
}

fn run_cli_with_stdin(
    cmd: &mut Command,
    prompt: &str,
    timeout_secs: u64,
    model_name: &str,
) -> Result<(String, String), String> {
    use std::process::Stdio;
    use std::time::Duration;

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes());
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("wait: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("exit {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let clean = strip_ansi(&stdout);
    Ok((clean, model_name.to_string()))
}

fn run_cli_with_prompt_arg(
    cmd: &mut Command,
    prompt: &str,
    timeout_secs: u64,
    model_name: &str,
) -> Result<(String, String), String> {
    // ClawCode takes prompt as positional arg after "prompt" subcommand
    cmd.arg(prompt);

    let output = cmd.output().map_err(|e| format!("spawn: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("exit {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let clean = strip_ansi(&stdout);
    Ok((clean, model_name.to_string()))
}

fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
            continue;
        }
        if in_escape {
            if ch.is_ascii_alphabetic() {
                in_escape = false;
            }
            continue;
        }
        // Skip control chars except newline/tab
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        result.push(ch);
    }
    // Remove spinner lines
    result
        .lines()
        .filter(|l| {
            !l.contains("Waiting for Claude")
                && !l.contains("Claude response complete")
                && !l.contains("Claude request failed")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("{t:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_prompt_from_messages() {
        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: Some("You are helpful.".to_string()),
            },
            ChatMessage {
                role: "user".to_string(),
                content: Some("Hello".to_string()),
            },
        ];
        let prompt = build_prompt(&messages);
        assert!(prompt.contains("<system>"));
        assert!(prompt.contains("Hello"));
    }

    #[test]
    fn strips_ansi_codes() {
        let input = "\x1b[38;5;12mHello\x1b[0m World";
        assert_eq!(strip_ansi(input), "Hello World");
    }

    #[test]
    fn strips_spinner_lines() {
        let input = "⠋ Waiting for Claude\nActual content\n✔ Claude response complete";
        assert_eq!(strip_ansi(input), "Actual content");
    }
}
