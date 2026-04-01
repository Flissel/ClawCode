//! ClawCode Router — Multi-backend LLM routing service.
//!
//! Exposes an OpenAI-compatible `/v1/chat/completions` endpoint that routes
//! requests to Claude Code, Kilo Code, or ClawCode backends based on agent role.
//!
//! Usage:
//!     clawcode-router                              # defaults to port 8090
//!     clawcode-router --config router.toml         # custom config
//!     clawcode-router --port 9090                  # custom port
//!     clawcode-router --dry-run                    # test config + backends without starting server

mod config;
mod dispatch;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Json;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;

use config::RouterConfig;
use dispatch::{dispatch, ChatRequest, ChatResponse, ErrorDetail, ErrorResponse};

struct AppState {
    config: RouterConfig,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let (config_path, port_override, dry_run) = parse_args(&args);

    let config = match RouterConfig::load(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {e}");
            std::process::exit(1);
        }
    };

    let port = port_override.unwrap_or(config.server.port);

    if dry_run {
        run_dry_test(&config);
        return;
    }

    let state = Arc::new(AppState { config });

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/health", get(health))
        .route("/config", get(show_config))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("ClawCode Router listening on {addr}");
    println!("ClawCode Router listening on http://{addr}");
    println!("  POST /v1/chat/completions  — OpenAI-compatible chat endpoint");
    println!("  GET  /health               — Health check");
    println!("  GET  /config               — Show routing config");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, (StatusCode, Json<ErrorResponse>)> {
    let agent = request.agent_role.as_deref().unwrap_or("default");
    tracing::info!(agent, "incoming request");

    match dispatch(&state.config, &request) {
        Ok(response) => {
            tracing::info!(
                backend = %response.backend,
                latency_ms = response.latency_ms,
                "request completed"
            );
            Ok(Json(response))
        }
        Err(e) => {
            tracing::error!(error = %e, "all backends failed");
            Err((
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: ErrorDetail {
                        message: e,
                        r#type: "backend_error".to_string(),
                    },
                }),
            ))
        }
    }
}

async fn health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "service": "clawcode-router",
        "backends": ["claude", "kilo", "clawcode"],
        "routing_rules": state.config.routing.len(),
        "fallback_chain": state.config.fallback.chain,
    }))
}

async fn show_config(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "routing": state.config.routing,
        "fallback": state.config.fallback.chain,
        "server_port": state.config.server.port,
    }))
}

fn parse_args(args: &[String]) -> (PathBuf, Option<u16>, bool) {
    let mut config_path = PathBuf::from("clawcode-router.toml");
    let mut port = None;
    let mut dry_run = false;
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                if let Some(v) = args.get(i + 1) {
                    config_path = PathBuf::from(v);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--port" | "-p" => {
                if let Some(v) = args.get(i + 1) {
                    port = v.parse().ok();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            "--help" | "-h" => {
                println!("clawcode-router — Multi-backend LLM routing service");
                println!();
                println!("Usage:");
                println!("  clawcode-router [OPTIONS]");
                println!();
                println!("Options:");
                println!("  --config PATH   Config file (default: clawcode-router.toml)");
                println!("  --port PORT     Listen port (default: 8090)");
                println!("  --dry-run       Test config and backends, then exit");
                std::process::exit(0);
            }
            _ => i += 1,
        }
    }

    (config_path, port, dry_run)
}

/// Dry-run: test config parsing, backend availability, and routing resolution.
fn run_dry_test(config: &RouterConfig) {
    println!("=== ClawCode Router Dry Run ===\n");

    // 1. Config
    println!("[Config]");
    println!("  Server: {}:{}", config.server.host, config.server.port);
    println!("  Routing rules: {}", config.routing.len());
    for (agent, backend) in &config.routing {
        println!("    {agent:20} -> {backend}");
    }
    println!("  Fallback chain: {:?}", config.fallback.chain);

    // 2. Backend availability
    println!("\n[Backends]");

    // Claude
    let claude_bin = config
        .backends
        .claude
        .binary
        .as_deref()
        .unwrap_or("claude");
    let claude_ok = check_binary(claude_bin);
    println!(
        "  claude:   {} ({})",
        if claude_ok { "OK" } else { "NOT FOUND" },
        claude_bin
    );

    // Kilo
    let kilo_bin = config.backends.kilo.binary.as_deref().unwrap_or("kilo");
    let kilo_ok = check_binary(kilo_bin);
    println!(
        "  kilo:     {} ({})",
        if kilo_ok { "OK" } else { "NOT FOUND" },
        kilo_bin
    );

    // ClawCode
    let claw_bin = config
        .backends
        .claw
        .binary
        .as_deref()
        .unwrap_or("rusty-claude-cli");
    let claw_ok = check_binary(claw_bin);
    println!(
        "  clawcode: {} ({})",
        if claw_ok { "OK" } else { "NOT FOUND" },
        claw_bin
    );

    // 3. Routing resolution test
    println!("\n[Routing Test]");
    let test_agents = [
        "architect",
        "backend_gen",
        "frontend_gen",
        "fixer",
        "reviewer",
        "tester",
        "database_gen",
        "unknown_agent",
    ];
    for agent in test_agents {
        let backend = config.resolve_backend(agent);
        println!("  {agent:20} -> {backend}");
    }

    // 4. Live LLM test (only if at least one backend works)
    println!("\n[Live Test]");
    if claw_ok || claude_ok || kilo_ok {
        let test_request = ChatRequest {
            model: None,
            messages: vec![dispatch::ChatMessage {
                role: "user".to_string(),
                content: Some("Reply with exactly: DRY_RUN_OK".to_string()),
            }],
            max_tokens: Some(10),
            temperature: None,
            agent_role: Some("default".to_string()),
        };

        match dispatch(config, &test_request) {
            Ok(resp) => {
                let content = resp
                    .choices
                    .first()
                    .and_then(|c| c.message.content.as_deref())
                    .unwrap_or("(empty)");
                println!(
                    "  Backend: {}\n  Latency: {}ms\n  Response: {}",
                    resp.backend,
                    resp.latency_ms,
                    &content[..content.len().min(100)]
                );
                println!("\n  DRY RUN PASSED");
            }
            Err(e) => {
                println!("  FAILED: {e}");
            }
        }
    } else {
        println!("  SKIPPED (no backends available)");
    }
}

fn check_binary(name: &str) -> bool {
    use std::process::Command;
    Command::new(name)
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}
