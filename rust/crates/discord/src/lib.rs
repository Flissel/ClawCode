//! Discord notifier for ClawCode pipeline monitoring.
//!
//! Posts structured messages to Discord channels that the DaveFelix
//! discord_monitor.py and discord_debug.py tools can parse.
//!
//! # Channel Layout
//! - `#orchestrator` — Master orchestrator status
//! - `#dev-tasks` — Agent task assignments
//! - `#integration` — Integration test results
//! - `#fixes` — Bug reports and fix attempts
//! - `#testing` — Test results
//! - `#done` — Completed tasks

use std::collections::HashMap;

use serde::Serialize;

const DISCORD_API: &str = "https://discord.com/api/v10";

/// Discord bot client for posting to channels.
pub struct DiscordNotifier {
    http: reqwest::Client,
    bot_token: String,
    channels: HashMap<String, String>,
}

#[derive(Debug)]
pub struct DiscordError {
    pub message: String,
}

impl std::fmt::Display for DiscordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Discord: {}", self.message)
    }
}

impl std::error::Error for DiscordError {}

type Result<T> = std::result::Result<T, DiscordError>;

/// A structured message with hidden JSON metadata (parsed by discord_debug.py).
#[derive(Debug, Clone, Serialize)]
pub struct StructuredMessage {
    pub r#type: String,
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DiscordNotifier {
    /// Create a new notifier with bot token and channel mapping.
    #[must_use]
    pub fn new(bot_token: &str, channels: HashMap<String, String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            bot_token: bot_token.to_string(),
            channels,
        }
    }

    /// Create from environment variables.
    ///
    /// Reads `DISCORD_BOT_TOKEN` and channel IDs from env.
    pub fn from_env() -> Result<Self> {
        let bot_token = std::env::var("DISCORD_BOT_TOKEN")
            .map_err(|_| DiscordError { message: "DISCORD_BOT_TOKEN not set".to_string() })?;

        let mut channels = HashMap::new();
        let channel_names = [
            "orchestrator",
            "dev-tasks",
            "integration",
            "fixes",
            "testing",
            "done",
        ];
        for name in channel_names {
            let env_key = format!("DISCORD_CHANNEL_{}", name.to_uppercase().replace('-', "_"));
            if let Ok(id) = std::env::var(&env_key) {
                channels.insert(name.to_string(), id);
            }
        }

        Ok(Self::new(&bot_token, channels))
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    }

    /// Send a plain text message to a named channel.
    pub fn send(&self, channel_name: &str, message: &str) -> Result<()> {
        let channel_id = self.channels.get(channel_name).ok_or_else(|| DiscordError {
            message: format!("Unknown channel: {channel_name}"),
        })?;

        // Truncate to Discord's 2000 char limit
        let msg = if message.len() > 1990 {
            format!("{}...", &message[..1987])
        } else {
            message.to_string()
        };

        let url = format!("{DISCORD_API}/channels/{channel_id}/messages");
        let body = serde_json::json!({ "content": msg });

        Self::rt().block_on(async {
            self.http
                .post(&url)
                .header("Authorization", format!("Bot {}", self.bot_token))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| DiscordError {
                    message: format!("send: {e}"),
                })?;
            Ok(())
        })
    }

    /// Send a structured message with hidden JSON metadata.
    ///
    /// Format: `visible text ||{"type":"...","scope":"..."}||`
    /// This is parsed by discord_debug.py via spoiler tag extraction.
    pub fn send_structured(
        &self,
        channel_name: &str,
        visible: &str,
        meta: &StructuredMessage,
    ) -> Result<()> {
        let json = serde_json::to_string(meta).unwrap_or_default();
        let full_msg = format!("{visible} ||{json}||");
        self.send(channel_name, &full_msg)
    }

    // ── Convenience methods matching DaveFelix StructuredDiscord ──

    /// Post a FIX_NEEDED message to #fixes.
    pub fn fix_needed(&self, task_id: &str, error: &str) -> Result<()> {
        self.send_structured(
            "fixes",
            &format!("FIX_NEEDED [{task_id}]: {error}"),
            &StructuredMessage {
                r#type: "FIX_NEEDED".to_string(),
                scope: "fixes".to_string(),
                task_id: Some(task_id.to_string()),
                error: Some(error.to_string()),
            },
        )
    }

    /// Post a FIX_APPLIED message to #fixes.
    pub fn fix_applied(&self, task_id: &str) -> Result<()> {
        self.send_structured(
            "fixes",
            &format!("FIX_APPLIED [{task_id}]"),
            &StructuredMessage {
                r#type: "FIX_APPLIED".to_string(),
                scope: "fixes".to_string(),
                task_id: Some(task_id.to_string()),
                error: None,
            },
        )
    }

    /// Post a test result to #testing.
    pub fn test_result(&self, task_id: &str, passed: bool) -> Result<()> {
        let status = if passed { "SUCCESS" } else { "FAILED" };
        self.send_structured(
            "testing",
            &format!("TEST_{status} [{task_id}]"),
            &StructuredMessage {
                r#type: format!("TEST_{status}"),
                scope: "testing".to_string(),
                task_id: Some(task_id.to_string()),
                error: None,
            },
        )
    }

    /// Post a task completion to #done.
    pub fn task_done(&self, task_id: &str) -> Result<()> {
        self.send_structured(
            "done",
            &format!("COMPLETE [{task_id}]"),
            &StructuredMessage {
                r#type: "COMPLETE".to_string(),
                scope: "done".to_string(),
                task_id: Some(task_id.to_string()),
                error: None,
            },
        )
    }

    /// Post a generation summary to #orchestrator.
    pub fn generation_summary(&self, project: &str, status: &str) -> Result<()> {
        self.send("orchestrator", &format!("Generation {status}: {project}"))
    }
}
