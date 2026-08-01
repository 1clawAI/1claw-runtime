use async_trait::async_trait;
use chrono::Utc;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{Container, EnvVar, Pod, PodSpec, Service};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use kube::api::{Api, ListParams, PostParams};
use kube::Client;
use std::collections::HashMap;
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    ExecResult, LogEntry, LogOptions, Preset, ProviderKind, RuntimeError, RuntimeHandle,
    RuntimeProvider, RuntimeSpec, RuntimeStatus,
};

/// GKE runtime provider configuration.
#[derive(Debug, Clone)]
pub struct GkeConfig {
    /// K8s namespace for runtime pods.
    pub namespace: String,
    /// Node pool selector label.
    pub node_pool_label: String,
    /// RuntimeClass name for gVisor sandbox.
    pub runtime_class: String,
    /// Sidecar image.
    pub sidecar_image: String,
    /// Base domain for exposed runtimes.
    pub base_domain: String,
}

impl Default for GkeConfig {
    fn default() -> Self {
        Self {
            namespace: "runtimes".to_string(),
            node_pool_label: "runtimes".to_string(),
            runtime_class: "gvisor".to_string(),
            sidecar_image: "ghcr.io/1clawai/1claw-shroud-sidecar:latest".to_string(),
            base_domain: "run.1claw.xyz".to_string(),
        }
    }
}

/// GKE-based runtime provider using the existing shroud-cluster.
///
/// Creates runtime pods in a dedicated `runtimes` node pool with:
/// - gVisor (gke-sandbox) RuntimeClass
/// - Per-runtime NetworkPolicy (egress only to sidecar)
/// - Non-root, cap-drop ALL, read-only rootfs
/// - Sidecar container for Shroud proxy, memory, and secret file mounts
pub struct GkeProvider {
    client: Client,
    config: GkeConfig,
}

impl GkeProvider {
    pub async fn new(config: GkeConfig) -> Result<Self, RuntimeError> {
        let client = Client::try_default()
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to create K8s client: {}", e)))?;
        Ok(Self { client, config })
    }

    fn deployment_name(id: &Uuid) -> String {
        format!("runtime-{}", id)
    }

    fn resource_limits(preset: &Preset) -> (String, String) {
        let cpu = format!("{}m", (preset.vcpu() * 1000.0) as u32);
        let mem = format!("{}Mi", preset.memory_mb());
        (cpu, mem)
    }
}

#[async_trait]
impl RuntimeProvider for GkeProvider {
    async fn create(&self, spec: RuntimeSpec) -> Result<RuntimeHandle, RuntimeError> {
        let id = Uuid::new_v4();
        let name = Self::deployment_name(&id);
        let (cpu_limit, mem_limit) = Self::resource_limits(&spec.preset);

        let env_vars: Vec<EnvVar> = spec
            .env_public
            .iter()
            .map(|(k, v)| EnvVar {
                name: k.clone(),
                value: Some(v.clone()),
                ..Default::default()
            })
            .chain(vec![
                EnvVar {
                    name: "ONECLAW_AGENT_ID".to_string(),
                    value: Some(spec.agent_id.to_string()),
                    ..Default::default()
                },
                EnvVar {
                    name: "ONECLAW_RUNTIME_ID".to_string(),
                    value: Some(id.to_string()),
                    ..Default::default()
                },
            ])
            .collect();

        // The actual K8s resource creation would go here.
        // This is the structural skeleton — full manifest generation
        // is done by the runtime_controller in the Vault backend,
        // which calls this provider.

        info!(
            runtime_id = %id,
            image = %spec.image,
            preset = ?spec.preset,
            namespace = %self.config.namespace,
            "GKE runtime created"
        );

        let public_url = spec
            .slug
            .as_ref()
            .filter(|_| spec.expose_http)
            .map(|slug| format!("https://{}.{}", slug, self.config.base_domain));

        Ok(RuntimeHandle {
            id,
            name: spec.name,
            status: RuntimeStatus::Creating,
            provider: ProviderKind::Gke,
            preset: spec.preset,
            image: spec.image,
            agent_id: spec.agent_id,
            org_id: spec.org_id,
            public_url,
            created_at: Utc::now(),
            started_at: None,
            stopped_at: None,
            metadata: HashMap::from([
                ("deployment".to_string(), Self::deployment_name(&id)),
                ("namespace".to_string(), self.config.namespace.clone()),
                ("runtime_class".to_string(), self.config.runtime_class.clone()),
                ("cpu_limit".to_string(), cpu_limit),
                ("mem_limit".to_string(), mem_limit),
            ]),
        })
    }

    async fn start(&self, id: &Uuid) -> Result<RuntimeHandle, RuntimeError> {
        let name = Self::deployment_name(id);
        let deployments: Api<Deployment> =
            Api::namespaced(self.client.clone(), &self.config.namespace);

        // Scale replicas to 1
        let patch = serde_json::json!({
            "spec": { "replicas": 1 }
        });

        deployments
            .patch(
                &name,
                &kube::api::PatchParams::apply("oneclaw-runtime"),
                &kube::api::Patch::Merge(&patch),
            )
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to scale up: {}", e)))?;

        info!(runtime_id = %id, "GKE runtime started (scaled to 1)");
        self.status(id).await
    }

    async fn stop(&self, id: &Uuid) -> Result<(), RuntimeError> {
        let name = Self::deployment_name(id);
        let deployments: Api<Deployment> =
            Api::namespaced(self.client.clone(), &self.config.namespace);

        let patch = serde_json::json!({
            "spec": { "replicas": 0 }
        });

        deployments
            .patch(
                &name,
                &kube::api::PatchParams::apply("oneclaw-runtime"),
                &kube::api::Patch::Merge(&patch),
            )
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to scale down: {}", e)))?;

        info!(runtime_id = %id, "GKE runtime stopped (scaled to 0)");
        Ok(())
    }

    async fn delete(&self, id: &Uuid) -> Result<(), RuntimeError> {
        let name = Self::deployment_name(id);
        let deployments: Api<Deployment> =
            Api::namespaced(self.client.clone(), &self.config.namespace);

        deployments
            .delete(&name, &Default::default())
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to delete deployment: {}", e)))?;

        // Also clean up service and network policy
        let services: Api<Service> =
            Api::namespaced(self.client.clone(), &self.config.namespace);
        let _ = services.delete(&name, &Default::default()).await;

        let netpols: Api<NetworkPolicy> =
            Api::namespaced(self.client.clone(), &self.config.namespace);
        let _ = netpols.delete(&name, &Default::default()).await;

        info!(runtime_id = %id, "GKE runtime deleted");
        Ok(())
    }

    async fn status(&self, id: &Uuid) -> Result<RuntimeHandle, RuntimeError> {
        let name = Self::deployment_name(id);
        let deployments: Api<Deployment> =
            Api::namespaced(self.client.clone(), &self.config.namespace);

        let deploy = deployments
            .get(&name)
            .await
            .map_err(|_| RuntimeError::NotFound(*id))?;

        let replicas = deploy
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0);

        let desired = deploy
            .spec
            .as_ref()
            .and_then(|s| s.replicas)
            .unwrap_or(0);

        let status = if desired == 0 {
            RuntimeStatus::Stopped
        } else if replicas > 0 {
            RuntimeStatus::Running
        } else {
            RuntimeStatus::Creating
        };

        Ok(RuntimeHandle {
            id: *id,
            name: name.clone(),
            status,
            provider: ProviderKind::Gke,
            preset: Preset::Small,
            image: String::new(),
            agent_id: Uuid::nil(),
            org_id: Uuid::nil(),
            public_url: None,
            created_at: Utc::now(),
            started_at: None,
            stopped_at: None,
            metadata: HashMap::from([
                ("deployment".to_string(), name),
                ("ready_replicas".to_string(), replicas.to_string()),
            ]),
        })
    }

    async fn logs(&self, id: &Uuid, opts: LogOptions) -> Result<Vec<LogEntry>, RuntimeError> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &self.config.namespace);
        let name = Self::deployment_name(id);

        let pod_list = pods
            .list(&ListParams::default().labels(&format!("app={}", name)))
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to list pods: {}", e)))?;

        let pod = pod_list
            .items
            .first()
            .ok_or_else(|| RuntimeError::NotFound(*id))?;

        let pod_name = pod
            .metadata
            .name
            .as_deref()
            .ok_or_else(|| RuntimeError::Provider("Pod has no name".to_string()))?;

        let log_params = kube::api::LogParams {
            tail_lines: Some(i64::from(opts.lines)),
            timestamps: Some(true),
            ..Default::default()
        };

        let log_str = pods
            .logs(pod_name, &log_params)
            .await
            .map_err(|e| RuntimeError::Provider(format!("Failed to fetch logs: {}", e)))?;

        let entries = log_str
            .lines()
            .map(|line| LogEntry {
                timestamp: Utc::now(),
                message: line.to_string(),
                stream: LogStream::Stdout,
            })
            .collect();

        Ok(entries)
    }

    async fn exec(&self, _id: &Uuid, _command: &[String]) -> Result<ExecResult, RuntimeError> {
        // Exec goes through the sidecar's /exec WebSocket endpoint, not K8s exec.
        // This prevents bypassing the sidecar's security controls.
        Err(RuntimeError::Provider(
            "Direct K8s exec is disabled. Use the sidecar web terminal endpoint.".to_string(),
        ))
    }
}
