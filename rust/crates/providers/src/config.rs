//! Configuration for ClawCode providers.
//!
//! Reads `clawcode.toml` for provider settings.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use runtime::RuntimeError;

/// Top-level config from `clawcode.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    /// Map of provider name → settings.
    #[serde(default)]
    pub provider: HashMap<String, ProviderEntry>,

    /// Default provider name.
    #[serde(default = "default_provider_name")]
    pub default_provider: String,
}

fn default_provider_name() -> String {
    "anthropic".to_string()
}

impl Default for ProviderConfig {
    fn default() -> Self {
        let mut provider = HashMap::new();
        provider.insert(
            "anthropic".to_string(),
            ProviderEntry {
                model: "claude-sonnet-4-6".to_string(),
                base_url: None,
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            },
        );
        Self {
            provider,
            default_provider: "anthropic".to_string(),
        }
    }
}

/// Per-provider configuration entry.
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderEntry {
    /// Model identifier (e.g. "claude-sonnet-4-6", "qwen/qwen3-coder:free").
    pub model: String,

    /// Base URL override (e.g. "http://localhost:11434" for Ollama).
    #[serde(default)]
    pub base_url: Option<String>,

    /// Environment variable name holding the API key.
    #[serde(default)]
    pub api_key_env: Option<String>,
}

/// Load config from a TOML file path.
///
/// Falls back to defaults if file doesn't exist.
///
/// # Errors
/// Returns `RuntimeError` if the file exists but cannot be parsed.
pub fn load_config(path: &Path) -> Result<ProviderConfig, RuntimeError> {
    if !path.exists() {
        return Ok(ProviderConfig::default());
    }

    let content = fs::read_to_string(path)
        .map_err(|e| RuntimeError::new(format!("Failed to read {}: {e}", path.display())))?;

    toml::from_str(&content)
        .map_err(|e| RuntimeError::new(format!("Failed to parse {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_anthropic() {
        let config = ProviderConfig::default();
        assert_eq!(config.default_provider, "anthropic");
        assert!(config.provider.contains_key("anthropic"));
    }

    #[test]
    fn parses_toml_config() {
        let toml = r#"
default_provider = "openrouter"

[provider.anthropic]
model = "claude-sonnet-4-6"
api_key_env = "ANTHROPIC_API_KEY"

[provider.openrouter]
model = "qwen/qwen3-coder:free"
api_key_env = "OPENROUTER_API_KEY"
base_url = "https://openrouter.ai/api/v1"

[provider.ollama]
model = "qwen2.5-coder:7b"
base_url = "http://localhost:11434"
"#;
        let config: ProviderConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.default_provider, "openrouter");
        assert_eq!(config.provider.len(), 3);
        assert_eq!(
            config.provider["ollama"].base_url.as_deref(),
            Some("http://localhost:11434")
        );
    }
}
