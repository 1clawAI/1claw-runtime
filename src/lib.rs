//! # oneclaw-runtime
//!
//! RuntimeProvider trait and implementations for 1Claw agent cloud runtimes.
//!
//! This crate defines the abstraction layer for managing containerized agent
//! runtimes across different infrastructure providers (GKE, Cloud Run, local Docker).
//!
//! ## Usage
//!
//! ```rust,no_run
//! use oneclaw_runtime::{RuntimeProvider, RuntimeSpec, Preset};
//!
//! # async fn example() -> Result<(), oneclaw_runtime::RuntimeError> {
//! let provider = oneclaw_runtime::providers::docker::LocalDockerProvider::new(Default::default());
//! let spec = RuntimeSpec::builder()
//!     .name("my-agent-runtime")
//!     .image("python:3.12-slim")
//!     .preset(Preset::Small)
//!     .build();
//! let handle = provider.create(spec).await?;
//! println!("Runtime started: {}", handle.id);
//! # Ok(())
//! # }
//! ```

pub mod providers;
mod types;

pub use types::*;

use async_trait::async_trait;
use uuid::Uuid;

/// Core trait for runtime lifecycle management.
///
/// Implementations manage containers on different infrastructure backends.
/// All operations are idempotent where possible.
#[async_trait]
pub trait RuntimeProvider: Send + Sync {
    /// Create and start a new runtime container.
    async fn create(&self, spec: RuntimeSpec) -> Result<RuntimeHandle, RuntimeError>;

    /// Start a stopped runtime.
    async fn start(&self, id: &Uuid) -> Result<RuntimeHandle, RuntimeError>;

    /// Stop a running runtime (preserves state for restart).
    async fn stop(&self, id: &Uuid) -> Result<(), RuntimeError>;

    /// Permanently delete a runtime and all its resources.
    async fn delete(&self, id: &Uuid) -> Result<(), RuntimeError>;

    /// Get current status and metadata.
    async fn status(&self, id: &Uuid) -> Result<RuntimeHandle, RuntimeError>;

    /// Stream recent logs from the runtime container.
    async fn logs(&self, id: &Uuid, opts: LogOptions) -> Result<Vec<LogEntry>, RuntimeError>;

    /// Execute a command inside the running container.
    /// Returns (stdout, stderr, exit_code).
    async fn exec(&self, id: &Uuid, command: &[String]) -> Result<ExecResult, RuntimeError>;
}
