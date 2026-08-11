use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Runtime compute preset — determines CPU, memory, and cost tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Small,
    Medium,
    Large,
    /// Confidential Compute (AMD SEV-SNP) — Business+ tier only.
    SmallCc,
    MediumCc,
    LargeCc,
}

impl Preset {
    pub fn vcpu(&self) -> f32 {
        match self {
            Preset::Small | Preset::SmallCc => 0.5,
            Preset::Medium | Preset::MediumCc => 1.0,
            Preset::Large | Preset::LargeCc => 2.0,
        }
    }

    pub fn memory_mb(&self) -> u32 {
        match self {
            Preset::Small | Preset::SmallCc => 1024,
            Preset::Medium | Preset::MediumCc => 2048,
            Preset::Large | Preset::LargeCc => 4096,
        }
    }

    pub fn is_confidential(&self) -> bool {
        matches!(self, Preset::SmallCc | Preset::MediumCc | Preset::LargeCc)
    }

    pub fn monthly_cost_usd(&self) -> f64 {
        match self {
            Preset::Small => 5.0,
            Preset::Medium => 15.0,
            Preset::Large => 35.0,
            Preset::SmallCc => 15.0,
            Preset::MediumCc => 35.0,
            Preset::LargeCc => 65.0,
        }
    }
}

/// Infrastructure provider backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Gke,
    CloudRun,
    Local,
}

/// Specification for creating a new runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSpec {
    pub name: String,
    pub image: String,
    pub preset: Preset,
    pub agent_id: Uuid,
    pub org_id: Uuid,
    /// Public (non-secret) environment variables.
    #[serde(default)]
    pub env_public: HashMap<String, String>,
    /// Seconds of inactivity before auto-stop (default: 1800).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout_secs: u32,
    /// Whether to expose an HTTP endpoint.
    #[serde(default)]
    pub expose_http: bool,
    /// Port the user container listens on (default: 8000).
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    /// Public slug for `{slug}.run.1claw.xyz`.
    pub slug: Option<String>,
    /// Inbound authentication mode when exposed.
    #[serde(default = "default_inbound_auth")]
    pub inbound_auth: InboundAuth,
    /// Secret paths to mount as tmpfs files (fetched by sidecar).
    #[serde(default)]
    pub secret_mounts: Vec<SecretMount>,
    /// Sidecar image override (defaults to latest stable).
    pub sidecar_image: Option<String>,
    /// Whether interactive shell access is enabled for this runtime.
    #[serde(default)]
    pub shell_access_enabled: bool,
    /// Authentication policy for shell sessions (`password`, `totp`, `passkey`).
    #[serde(default = "default_shell_auth_policy")]
    pub shell_auth_policy: String,
    /// Maximum shell session duration in minutes.
    #[serde(default = "default_shell_max_session_minutes")]
    pub shell_max_session_minutes: i32,
}

impl RuntimeSpec {
    pub fn builder() -> RuntimeSpecBuilder {
        RuntimeSpecBuilder::default()
    }
}

fn default_idle_timeout() -> u32 {
    1800
}

fn default_http_port() -> u16 {
    8000
}

fn default_inbound_auth() -> InboundAuth {
    InboundAuth::ApiKey
}

fn default_shell_auth_policy() -> String {
    "password".to_string()
}

fn default_shell_max_session_minutes() -> i32 {
    30
}

/// Inbound authentication mode for exposed runtimes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum InboundAuth {
    #[default]
    ApiKey,
    Jwt,
    Public,
}

/// A secret to mount as a file inside the container.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretMount {
    pub vault_id: Uuid,
    pub secret_path: String,
    /// Filesystem path inside the container (e.g. `/secrets/api-key`).
    pub mount_path: String,
}

/// Handle to a running or stopped runtime with current status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeHandle {
    pub id: Uuid,
    pub name: String,
    pub status: RuntimeStatus,
    pub provider: ProviderKind,
    pub preset: Preset,
    pub image: String,
    pub agent_id: Uuid,
    pub org_id: Uuid,
    pub public_url: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    /// Provider-specific metadata (container ID, pod name, etc.).
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Whether interactive shell access is enabled.
    #[serde(default)]
    pub shell_access_enabled: bool,
}

/// Runtime lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Creating,
    Running,
    Stopping,
    Stopped,
    Failed,
    Deleting,
}

/// Options for fetching logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogOptions {
    /// Number of recent lines to fetch.
    #[serde(default = "default_log_lines")]
    pub lines: u32,
    /// Only include logs after this timestamp.
    pub since: Option<DateTime<Utc>>,
    /// Include sidecar logs alongside user container logs.
    #[serde(default)]
    pub include_sidecar: bool,
}

impl Default for LogOptions {
    fn default() -> Self {
        Self {
            lines: 100,
            since: None,
            include_sidecar: false,
        }
    }
}

fn default_log_lines() -> u32 {
    100
}

/// A single log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub message: String,
    pub stream: LogStream,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    Stdout,
    Stderr,
    Sidecar,
}

/// Result of executing a command inside a runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Errors from runtime operations.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("Runtime not found: {0}")]
    NotFound(Uuid),
    #[error("Runtime is in an invalid state for this operation: {status:?}")]
    InvalidState { status: RuntimeStatus },
    #[error("Provider error: {0}")]
    Provider(String),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Resource limit exceeded: {0}")]
    ResourceLimit(String),
    #[error("Image not allowed: {0}")]
    ImageDenied(String),
    #[error("Timeout waiting for runtime")]
    Timeout,
}

/// Builder for `RuntimeSpec`.
#[derive(Default)]
pub struct RuntimeSpecBuilder {
    name: Option<String>,
    image: Option<String>,
    preset: Option<Preset>,
    agent_id: Option<Uuid>,
    org_id: Option<Uuid>,
    env_public: HashMap<String, String>,
    idle_timeout_secs: u32,
    expose_http: bool,
    http_port: u16,
    slug: Option<String>,
    inbound_auth: InboundAuth,
    secret_mounts: Vec<SecretMount>,
    sidecar_image: Option<String>,
    shell_access_enabled: bool,
    shell_auth_policy: String,
    shell_max_session_minutes: i32,
}

impl RuntimeSpecBuilder {
    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    pub fn image(mut self, image: &str) -> Self {
        self.image = Some(image.to_string());
        self
    }

    pub fn preset(mut self, preset: Preset) -> Self {
        self.preset = Some(preset);
        self
    }

    pub fn agent_id(mut self, id: Uuid) -> Self {
        self.agent_id = Some(id);
        self
    }

    pub fn org_id(mut self, id: Uuid) -> Self {
        self.org_id = Some(id);
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env_public.insert(key.to_string(), value.to_string());
        self
    }

    pub fn idle_timeout(mut self, secs: u32) -> Self {
        self.idle_timeout_secs = secs;
        self
    }

    pub fn expose_http(mut self, expose: bool) -> Self {
        self.expose_http = expose;
        self
    }

    pub fn http_port(mut self, port: u16) -> Self {
        self.http_port = port;
        self
    }

    pub fn slug(mut self, slug: &str) -> Self {
        self.slug = Some(slug.to_string());
        self
    }

    pub fn inbound_auth(mut self, auth: InboundAuth) -> Self {
        self.inbound_auth = auth;
        self
    }

    pub fn secret_mount(mut self, mount: SecretMount) -> Self {
        self.secret_mounts.push(mount);
        self
    }

    pub fn sidecar_image(mut self, image: &str) -> Self {
        self.sidecar_image = Some(image.to_string());
        self
    }

    pub fn shell_access(mut self, enabled: bool) -> Self {
        self.shell_access_enabled = enabled;
        self
    }

    pub fn shell_auth_policy(mut self, policy: &str) -> Self {
        self.shell_auth_policy = policy.to_string();
        self
    }

    pub fn shell_max_session_minutes(mut self, minutes: i32) -> Self {
        self.shell_max_session_minutes = minutes;
        self
    }

    pub fn build(self) -> RuntimeSpec {
        RuntimeSpec {
            name: self.name.unwrap_or_else(|| "runtime".to_string()),
            image: self.image.unwrap_or_else(|| "python:3.12-slim".to_string()),
            preset: self.preset.unwrap_or(Preset::Small),
            agent_id: self.agent_id.unwrap_or_else(Uuid::new_v4),
            org_id: self.org_id.unwrap_or_else(Uuid::new_v4),
            env_public: self.env_public,
            idle_timeout_secs: if self.idle_timeout_secs == 0 { 1800 } else { self.idle_timeout_secs },
            expose_http: self.expose_http,
            http_port: if self.http_port == 0 { 8000 } else { self.http_port },
            slug: self.slug,
            inbound_auth: self.inbound_auth,
            secret_mounts: self.secret_mounts,
            sidecar_image: self.sidecar_image,
            shell_access_enabled: self.shell_access_enabled,
            shell_auth_policy: if self.shell_auth_policy.is_empty() { "password".to_string() } else { self.shell_auth_policy },
            shell_max_session_minutes: if self.shell_max_session_minutes == 0 { 30 } else { self.shell_max_session_minutes },
        }
    }
}
