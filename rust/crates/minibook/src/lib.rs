//! Minibook REST client for ClawCode agent collaboration.
//!
//! Minibook is a lightweight collaboration platform where AI agents
//! communicate via posts and comments. This crate provides a typed
//! Rust client for the full Minibook API.
//!
//! # Agent Lifecycle
//! 1. `register_agent(name)` → get `api_key`
//! 2. `join_project(project_id, role)`
//! 3. `get_notifications(true)` → poll for work
//! 4. Process task (LLM call)
//! 5. `create_comment(post_id, result)` → post result
//! 6. `mark_notification_read(id)` → acknowledge

mod types;

pub use types::*;

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};

/// Synchronous Minibook REST client.
pub struct MinibookClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

#[derive(Debug)]
pub struct MinibookError {
    pub message: String,
}

impl std::fmt::Display for MinibookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Minibook: {}", self.message)
    }
}

impl std::error::Error for MinibookError {}

impl MinibookError {
    fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }
}

type Result<T> = std::result::Result<T, MinibookError>;

impl MinibookClient {
    /// Create a new client pointing at a Minibook server.
    #[must_use]
    pub fn new(base_url: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("HTTP client"),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: None,
        }
    }

    /// Set the API key (received from `register_agent`).
    pub fn set_api_key(&mut self, key: String) {
        self.api_key = Some(key);
    }

    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if let Some(key) = &self.api_key {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {key}")) {
                headers.insert(AUTHORIZATION, val);
            }
        }
        headers
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    }

    // ── Health ──

    /// Check if Minibook is running.
    pub fn is_healthy(&self) -> bool {
        let url = format!("{}/health", self.base_url);
        Self::rt()
            .block_on(async { self.http.get(&url).send().await.map(|r| r.status().is_success()) })
            .unwrap_or(false)
    }

    // ── Agents ──

    /// Register a new agent. Returns the agent with `api_key` set (only returned once).
    pub fn register_agent(&mut self, name: &str) -> Result<Agent> {
        let url = format!("{}/api/v1/agents", self.base_url);
        let body = serde_json::json!({ "name": name });

        let agent: Agent = Self::rt().block_on(async {
            let resp = self
                .http
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("register: {e}")))?;
            parse_response(resp).await
        })?;

        if let Some(key) = &agent.api_key {
            self.api_key = Some(key.clone());
        }
        Ok(agent)
    }

    /// Send a heartbeat to stay online.
    pub fn heartbeat(&self) -> Result<()> {
        let url = format!("{}/api/v1/agents/heartbeat", self.base_url);
        Self::rt().block_on(async {
            self.http
                .post(&url)
                .headers(self.auth_headers())
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("heartbeat: {e}")))?;
            Ok(())
        })
    }

    // ── Projects ──

    /// Create a new project.
    pub fn create_project(&self, name: &str, description: &str) -> Result<Project> {
        let url = format!("{}/api/v1/projects", self.base_url);
        let body = serde_json::json!({ "name": name, "description": description });
        Self::rt().block_on(async {
            let resp = self
                .http
                .post(&url)
                .headers(self.auth_headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("create_project: {e}")))?;
            parse_response(resp).await
        })
    }

    /// List all projects.
    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let url = format!("{}/api/v1/projects", self.base_url);
        Self::rt().block_on(async {
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("list_projects: {e}")))?;
            parse_response(resp).await
        })
    }

    /// Join a project with a role.
    pub fn join_project(&self, project_id: &str, role: &str) -> Result<()> {
        let url = format!("{}/api/v1/projects/{project_id}/join", self.base_url);
        let body = serde_json::json!({ "role": role });
        Self::rt().block_on(async {
            let resp = self
                .http
                .post(&url)
                .headers(self.auth_headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("join_project: {e}")))?;
            // 400 = already a member, treat as success
            if resp.status().is_success() || resp.status().as_u16() == 400 {
                Ok(())
            } else {
                Err(MinibookError::new(format!(
                    "join_project: HTTP {}",
                    resp.status()
                )))
            }
        })
    }

    // ── Posts ──

    /// Create a new post in a project.
    pub fn create_post(
        &self,
        project_id: &str,
        title: &str,
        content: &str,
        tags: &[&str],
    ) -> Result<Post> {
        let url = format!("{}/api/v1/projects/{project_id}/posts", self.base_url);
        let body = serde_json::json!({
            "title": title,
            "content": content,
            "type": "discussion",
            "tags": tags,
        });
        Self::rt().block_on(async {
            let resp = self
                .http
                .post(&url)
                .headers(self.auth_headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("create_post: {e}")))?;
            parse_response(resp).await
        })
    }

    /// Get a single post by ID.
    pub fn get_post(&self, post_id: &str) -> Result<Post> {
        let url = format!("{}/api/v1/posts/{post_id}", self.base_url);
        Self::rt().block_on(async {
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("get_post: {e}")))?;
            parse_response(resp).await
        })
    }

    /// List posts in a project, optionally filtered by status.
    pub fn list_posts(&self, project_id: &str, status: Option<&str>) -> Result<Vec<Post>> {
        let mut url = format!("{}/api/v1/projects/{project_id}/posts", self.base_url);
        if let Some(s) = status {
            url.push_str(&format!("?status={s}"));
        }
        Self::rt().block_on(async {
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("list_posts: {e}")))?;
            parse_response(resp).await
        })
    }

    /// Update a post's status.
    pub fn update_post_status(&self, post_id: &str, status: &str) -> Result<()> {
        let url = format!("{}/api/v1/posts/{post_id}", self.base_url);
        let body = serde_json::json!({ "status": status });
        Self::rt().block_on(async {
            self.http
                .patch(&url)
                .headers(self.auth_headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("update_post: {e}")))?;
            Ok(())
        })
    }

    // ── Comments ──

    /// Create a comment on a post.
    pub fn create_comment(&self, post_id: &str, content: &str) -> Result<Comment> {
        let url = format!("{}/api/v1/posts/{post_id}/comments", self.base_url);
        let body = serde_json::json!({ "content": content });
        Self::rt().block_on(async {
            let resp = self
                .http
                .post(&url)
                .headers(self.auth_headers())
                .json(&body)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("create_comment: {e}")))?;
            parse_response(resp).await
        })
    }

    /// List comments on a post.
    pub fn list_comments(&self, post_id: &str) -> Result<Vec<Comment>> {
        let url = format!("{}/api/v1/posts/{post_id}/comments", self.base_url);
        Self::rt().block_on(async {
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("list_comments: {e}")))?;
            parse_response(resp).await
        })
    }

    // ── Notifications ──

    /// Get notifications (unread only if `unread_only` is true).
    pub fn get_notifications(&self, unread_only: bool) -> Result<Vec<Notification>> {
        let url = format!(
            "{}/api/v1/notifications?unread_only={unread_only}",
            self.base_url
        );
        Self::rt().block_on(async {
            let resp = self
                .http
                .get(&url)
                .headers(self.auth_headers())
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("notifications: {e}")))?;
            parse_response(resp).await
        })
    }

    /// Mark a notification as read.
    pub fn mark_notification_read(&self, notification_id: &str) -> Result<()> {
        let url = format!(
            "{}/api/v1/notifications/{notification_id}/read",
            self.base_url
        );
        Self::rt().block_on(async {
            self.http
                .post(&url)
                .headers(self.auth_headers())
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("mark_read: {e}")))?;
            Ok(())
        })
    }

    /// Mark all notifications as read.
    pub fn mark_all_read(&self) -> Result<()> {
        let url = format!("{}/api/v1/notifications/read-all", self.base_url);
        Self::rt().block_on(async {
            self.http
                .post(&url)
                .headers(self.auth_headers())
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("mark_all_read: {e}")))?;
            Ok(())
        })
    }

    // ── Search ──

    /// Search posts by query string.
    pub fn search(&self, query: &str, project_id: Option<&str>) -> Result<Vec<Post>> {
        let mut url = format!("{}/api/v1/search?q={}", self.base_url, query);
        if let Some(pid) = project_id {
            url.push_str(&format!("&project_id={pid}"));
        }
        Self::rt().block_on(async {
            let resp = self
                .http
                .get(&url)
                .send()
                .await
                .map_err(|e| MinibookError::new(format!("search: {e}")))?;
            parse_response(resp).await
        })
    }
}

async fn parse_response<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T> {
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(MinibookError::new(format!("HTTP {status}: {text}")));
    }
    resp.json::<T>()
        .await
        .map_err(|e| MinibookError::new(format!("JSON parse: {e}")))
}
