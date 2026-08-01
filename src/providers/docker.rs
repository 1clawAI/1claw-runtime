use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, LogOutput, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StopContainerOptions,
};
use bollard::models::{HostConfig, PortBinding};
use bollard::Docker;
use chrono::Utc;
use std::collections::HashMap;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    ExecResult, LogEntry, LogOptions, LogStream, ProviderKind, RuntimeError, RuntimeHandle,
    RuntimeProvider, RuntimeSpec, RuntimeStatus,
};

/// Allowed base images for local Docker runtimes.
const DEFAULT_BASE_IMAGES: &[&str] = &[
    "python:3.12-slim",
    "node:22-slim",
    "rust:1-slim",
    "golang:1.23-alpine",
    "ubuntu:24.04",
];

/// Configuration for the local Docker provider.
#[derive(Debug, Clone)]
pub struct DockerConfig {
    /// Additional allowed base images beyond the defaults.
    pub extra_allowed_images: Vec<String>,
    /// Docker network for isolated containers.
    pub network_name: String,
    /// Whether to allow internet access (default: false).
    pub allow_internet: bool,
    /// Sidecar image to use.
    pub sidecar_image: String,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            extra_allowed_images: Vec::new(),
            network_name: "1claw-isolated".to_string(),
            allow_internet: false,
            sidecar_image: "ghcr.io/1clawai/1claw-shroud-sidecar:latest".to_string(),
        }
    }
}

/// Local Docker runtime provider with hardened defaults.
///
/// Security: cap-drop ALL, no-new-privileges, read-only rootfs,
/// non-root user, PID limits, memory/CPU limits per preset.
/// No Docker socket mounting — ever.
pub struct LocalDockerProvider {
    docker: Docker,
    config: DockerConfig,
}

impl LocalDockerProvider {
    pub fn new(config: DockerConfig) -> Self {
        let docker = Docker::connect_with_local_defaults()
            .expect("Failed to connect to Docker daemon");
        Self { docker, config }
    }

    fn container_name(id: &Uuid) -> String {
        format!("1claw-runtime-{}", id)
    }

    fn is_image_allowed(&self, image: &str) -> bool {
        let base = image.split('@').next().unwrap_or(image);
        DEFAULT_BASE_IMAGES.iter().any(|allowed| base.starts_with(allowed))
            || self
                .config
                .extra_allowed_images
                .iter()
                .any(|allowed| base.starts_with(allowed))
    }
}

#[async_trait]
impl RuntimeProvider for LocalDockerProvider {
    async fn create(&self, spec: RuntimeSpec) -> Result<RuntimeHandle, RuntimeError> {
        if !self.is_image_allowed(&spec.image) {
            return Err(RuntimeError::ImageDenied(format!(
                "Image '{}' is not in the allowed base image list",
                spec.image
            )));
        }

        let id = Uuid::new_v4();
        let container_name = Self::container_name(&id);

        let mut env_vars: Vec<String> = spec
            .env_public
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        env_vars.push(format!("ONECLAW_AGENT_ID={}", spec.agent_id));
        env_vars.push(format!("ONECLAW_RUNTIME_ID={}", id));

        let memory_limit = i64::from(spec.preset.memory_mb()) * 1024 * 1024;
        let nano_cpus = (spec.preset.vcpu() * 1_000_000_000.0) as i64;

        let mut port_bindings = HashMap::new();
        if spec.expose_http {
            port_bindings.insert(
                format!("{}/tcp", spec.http_port),
                Some(vec![PortBinding {
                    host_ip: Some("127.0.0.1".to_string()),
                    host_port: Some("0".to_string()),
                }]),
            );
        }

        let host_config = HostConfig {
            memory: Some(memory_limit),
            nano_cpus: Some(nano_cpus),
            cap_drop: Some(vec!["ALL".to_string()]),
            security_opt: Some(vec!["no-new-privileges".to_string()]),
            read_only_rootfs: Some(true),
            pids_limit: Some(256),
            network_mode: if self.config.allow_internet {
                None
            } else {
                Some(self.config.network_name.clone())
            },
            port_bindings: if spec.expose_http {
                Some(port_bindings)
            } else {
                None
            },
            tmpfs: Some(HashMap::from([
                ("/tmp".to_string(), "size=128m,noexec,nosuid".to_string()),
                ("/secrets".to_string(), "size=16m,noexec,nosuid,nodev".to_string()),
            ])),
            ..Default::default()
        };

        let container_config = Config {
            image: Some(spec.image.clone()),
            env: Some(env_vars),
            user: Some("65534:65534".to_string()),
            host_config: Some(host_config),
            ..Default::default()
        };

        let opts = CreateContainerOptions {
            name: &container_name,
            platform: None,
        };

        self.docker
            .create_container(Some(opts), container_config)
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to create container: {}", e)))?;

        self.docker
            .start_container(&container_name, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to start container: {}", e)))?;

        info!(runtime_id = %id, image = %spec.image, preset = ?spec.preset, "Runtime created");

        Ok(RuntimeHandle {
            id,
            name: spec.name,
            status: RuntimeStatus::Running,
            provider: ProviderKind::Local,
            preset: spec.preset,
            image: spec.image,
            agent_id: spec.agent_id,
            org_id: spec.org_id,
            public_url: None,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            stopped_at: None,
            metadata: HashMap::from([("container_name".to_string(), container_name)]),
        })
    }

    async fn start(&self, id: &Uuid) -> Result<RuntimeHandle, RuntimeError> {
        let name = Self::container_name(id);
        self.docker
            .start_container(&name, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to start: {}", e)))?;

        info!(runtime_id = %id, "Runtime started");
        self.status(id).await
    }

    async fn stop(&self, id: &Uuid) -> Result<(), RuntimeError> {
        let name = Self::container_name(id);
        self.docker
            .stop_container(&name, Some(StopContainerOptions { t: 30 }))
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to stop: {}", e)))?;

        info!(runtime_id = %id, "Runtime stopped");
        Ok(())
    }

    async fn delete(&self, id: &Uuid) -> Result<(), RuntimeError> {
        let name = Self::container_name(id);
        self.docker
            .remove_container(
                &name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to delete: {}", e)))?;

        info!(runtime_id = %id, "Runtime deleted");
        Ok(())
    }

    async fn status(&self, id: &Uuid) -> Result<RuntimeHandle, RuntimeError> {
        let name = Self::container_name(id);
        let inspect = self
            .docker
            .inspect_container(&name, None)
            .await
            .map_err(|_| RuntimeError::NotFound(*id))?;

        let state = inspect.state.as_ref();
        let status = match state.and_then(|s| s.status) {
            Some(bollard::models::ContainerStateStatusEnum::RUNNING) => RuntimeStatus::Running,
            Some(bollard::models::ContainerStateStatusEnum::EXITED) => RuntimeStatus::Stopped,
            Some(bollard::models::ContainerStateStatusEnum::CREATED) => RuntimeStatus::Creating,
            _ => RuntimeStatus::Failed,
        };

        Ok(RuntimeHandle {
            id: *id,
            name: inspect
                .name
                .unwrap_or_default()
                .trim_start_matches('/')
                .to_string(),
            status,
            provider: ProviderKind::Local,
            preset: Preset::Small,
            image: inspect.config.and_then(|c| c.image).unwrap_or_default(),
            agent_id: Uuid::nil(),
            org_id: Uuid::nil(),
            public_url: None,
            created_at: Utc::now(),
            started_at: state
                .and_then(|s| s.started_at.as_deref())
                .and_then(|s| s.parse().ok()),
            stopped_at: None,
            metadata: HashMap::from([("container_name".to_string(), name)]),
        })
    }

    async fn logs(&self, id: &Uuid, opts: LogOptions) -> Result<Vec<LogEntry>, RuntimeError> {
        let name = Self::container_name(id);
        let log_opts = LogsOptions::<String> {
            stdout: true,
            stderr: true,
            tail: opts.lines.to_string(),
            timestamps: true,
            ..Default::default()
        };

        use futures_util::StreamExt;
        let mut stream = self.docker.logs(&name, Some(log_opts));
        let mut entries = Vec::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(output) => {
                    let (stream_type, message) = match output {
                        LogOutput::StdOut { message } => {
                            (LogStream::Stdout, String::from_utf8_lossy(&message).to_string())
                        }
                        LogOutput::StdErr { message } => {
                            (LogStream::Stderr, String::from_utf8_lossy(&message).to_string())
                        }
                        _ => continue,
                    };
                    entries.push(LogEntry {
                        timestamp: Utc::now(),
                        message,
                        stream: stream_type,
                    });
                }
                Err(e) => {
                    warn!(runtime_id = %id, error = %e, "Error reading logs");
                    break;
                }
            }
        }

        Ok(entries)
    }

    async fn exec(&self, id: &Uuid, command: &[String]) -> Result<ExecResult, RuntimeError> {
        let name = Self::container_name(id);

        let exec_opts = bollard::exec::CreateExecOptions {
            cmd: Some(command.to_vec()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            user: Some("65534".to_string()),
            ..Default::default()
        };

        let exec = self
            .docker
            .create_exec(&name, exec_opts)
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to create exec: {}", e)))?;

        let output = self
            .docker
            .start_exec(&exec.id, None)
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to start exec: {}", e)))?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        if let bollard::exec::StartExecResults::Attached { mut output, .. } = output {
            use futures_util::StreamExt;
            while let Some(Ok(msg)) = output.next().await {
                match msg {
                    LogOutput::StdOut { message } => {
                        stdout.push_str(&String::from_utf8_lossy(&message));
                    }
                    LogOutput::StdErr { message } => {
                        stderr.push_str(&String::from_utf8_lossy(&message));
                    }
                    _ => {}
                }
            }
        }

        let inspect = self
            .docker
            .inspect_exec(&exec.id)
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to inspect exec: {}", e)))?;

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code: inspect.exit_code.unwrap_or(-1) as i32,
        })
    }
}
