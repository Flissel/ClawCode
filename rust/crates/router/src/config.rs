//! Router configuration — maps agent roles to LLM backends.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Deserialize;

/// Top-level router config from `clawcode-router.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct RouterConfig {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub routing: HashMap<String, String>,
    #[serde(default)]
    pub fallback: FallbackConfig,
    #[serde(default)]
    pub backends: BackendsConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8090
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct FallbackConfig {
    #[serde(default)]
    pub chain: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BackendsConfig {
    #[serde(default)]
    pub claude: CliBackendConfig,
    #[serde(default)]
    pub kilo: CliBackendConfig,
    #[serde(default)]
    pub claw: ClawBackendConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CliBackendConfig {
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub binary: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

impl Default for CliBackendConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 600,
            binary: None,
            model: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClawBackendConfig {
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub binary: Option<String>,
    #[serde(default)]
    pub config_path: Option<String>,
}

impl Default for ClawBackendConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 600,
            binary: None,
            config_path: None,
        }
    }
}

fn default_timeout() -> u64 {
    600
}

impl RouterConfig {
    /// Load config from TOML file, falling back to defaults.
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default_config());
        }
        let content =
            fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        toml::from_str(&content).map_err(|e| format!("parse {}: {e}", path.display()))
    }

    /// Resolve which backend to use for a given agent role.
    ///
    /// Returns the backend string (e.g. "claude", "kilo", "claw:openrouter/qwen3.6:free").
    pub fn resolve_backend(&self, agent_role: &str) -> String {
        // 1. Exact match
        if let Some(backend) = self.routing.get(agent_role) {
            return backend.clone();
        }
        // 2. "default" key
        if let Some(backend) = self.routing.get("default") {
            return backend.clone();
        }
        // 3. First in fallback chain
        if let Some(first) = self.fallback.chain.first() {
            return first.clone();
        }
        // 4. Hardcoded default
        "claw:openrouter/qwen/qwen3.6-plus-preview:free".to_string()
    }

    fn default_config() -> Self {
        let mut routing = HashMap::new();
        routing.insert(
            "default".to_string(),
            "claw:openrouter/qwen/qwen3.6-plus-preview:free".to_string(),
        );
        Self {
            server: ServerConfig::default(),
            routing,
            fallback: FallbackConfig {
                chain: vec![
                    "claw:openrouter".to_string(),
                    "claude".to_string(),
                    "kilo".to_string(),
                    "claw:ollama".to_string(),
                ],
            },
            backends: BackendsConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_exact_match() {
        let mut routing = HashMap::new();
        routing.insert("architect".to_string(), "claude".to_string());
        routing.insert("tester".to_string(), "claw:ollama/codestral".to_string());
        let config = RouterConfig {
            server: ServerConfig::default(),
            routing,
            fallback: FallbackConfig::default(),
            backends: BackendsConfig::default(),
        };
        assert_eq!(config.resolve_backend("architect"), "claude");
        assert_eq!(config.resolve_backend("tester"), "claw:ollama/codestral");
    }

    #[test]
    fn falls_back_to_default_key() {
        let mut routing = HashMap::new();
        routing.insert("default".to_string(), "kilo".to_string());
        let config = RouterConfig {
            server: ServerConfig::default(),
            routing,
            fallback: FallbackConfig::default(),
            backends: BackendsConfig::default(),
        };
        assert_eq!(config.resolve_backend("unknown_agent"), "kilo");
    }

    #[test]
    fn parses_full_toml() {
        let toml_str = r#"
[server]
port = 9090

[routing]
architect = "claude"
backend_gen = "claw:openrouter/qwen3.6:free"
fixer = "kilo"
tester = "claw:ollama/qwen2.5-coder:7b"
default = "claw:openrouter/qwen3.6:free"

[fallback]
chain = ["claw:openrouter", "claude", "kilo"]

[backends.claude]
timeout_secs = 300

[backends.kilo]
model = "openai/gpt-5.4"

[backends.claw]
config_path = "/app/clawcode.toml"
"#;
        let config: RouterConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.resolve_backend("architect"), "claude");
        assert_eq!(config.resolve_backend("fixer"), "kilo");
        assert_eq!(config.fallback.chain.len(), 3);
    }
}
