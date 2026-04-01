//! Multi-provider abstraction for ClawCode.
//!
//! Wraps the runtime `ApiClient` trait with implementations for:
//! - Anthropic (via existing `api` crate)
//! - OpenRouter (OpenAI-compatible API)
//! - Ollama (local models)
//!
//! # Usage
//! ```no_run
//! use providers::{create_provider, load_config};
//! use std::path::Path;
//!
//! let config = load_config(Path::new("clawcode.toml")).unwrap();
//! let mut client = create_provider(&config, "openrouter").unwrap();
//! ```

mod anthropic_adapter;
mod config;
mod ollama;
mod openrouter;

pub use anthropic_adapter::AnthropicAdapter;
pub use config::{load_config, ProviderConfig, ProviderEntry};
pub use ollama::OllamaProvider;
pub use openrouter::OpenRouterProvider;

use runtime::{ApiClient, RuntimeError};

/// Create a provider by name from config.
///
/// # Errors
/// Returns `RuntimeError` if the provider name is unknown or config is missing.
pub fn create_provider(
    config: &ProviderConfig,
    name: &str,
) -> Result<Box<dyn ApiClient>, RuntimeError> {
    match name {
        "anthropic" => {
            let entry = config
                .provider
                .get("anthropic")
                .ok_or_else(|| RuntimeError::new("No [provider.anthropic] in config"))?;
            let client = AnthropicAdapter::from_env(&entry.model)?;
            Ok(Box::new(client))
        }
        "openrouter" => {
            let entry = config
                .provider
                .get("openrouter")
                .ok_or_else(|| RuntimeError::new("No [provider.openrouter] in config"))?;
            let client = OpenRouterProvider::new(entry)?;
            Ok(Box::new(client))
        }
        "ollama" => {
            let entry = config
                .provider
                .get("ollama")
                .ok_or_else(|| RuntimeError::new("No [provider.ollama] in config"))?;
            let client = OllamaProvider::new(entry)?;
            Ok(Box::new(client))
        }
        other => Err(RuntimeError::new(format!("Unknown provider: {other}"))),
    }
}
