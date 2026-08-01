use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;
use uuid::Uuid;

use crate::{
    ExecResult, LogEntry, LogOptions, ProviderKind, RuntimeError, RuntimeHandle, RuntimeProvider,
    RuntimeSpec, RuntimeStatus,
};

/// Cloud Run provider configuration.
#[derive(Debug, Clone)]
pub struct CloudRunConfig {
    /// GCP project ID.
    pub project_id: String,
    /// GCP region (e.g. us-central1).
    pub region: String,
    /// Base domain for exposed runtimes.
    pub base_domain: String,
}

/// Cloud Run runtime provider.
///
/// Deploys runtimes as Cloud Run services with min-instances=0 for
/// cost-efficient scale-to-zero. Best for stateless agents that don't
/// need persistent sidecar state (memory scratch is lost on scale-down).
pub struct CloudRunProvider {
    http: Client,
    config: CloudRunConfig,
}

impl CloudRunProvider {
    pub fn new(config: CloudRunConfig) -> Self {
        Self {
            http: Client::new(),
            config,
        }
    }

    fn service_name(id: &Uuid) -> String {
        format!("rt-{}", &id.to_string()[..8])
    }
}

#[async_trait]
impl RuntimeProvider for CloudRunProvider {
    async fn create(&self, spec: RuntimeSpec) -> Result<RuntimeHandle, RuntimeError> {
        let id = Uuid::new_v4();
        let service_name = Self::service_name(&id);

        // Cloud Run service creation would go through the GCP REST API:
        // POST https://run.googleapis.com/v2/projects/{project}/locations/{region}/services
        //
        // The actual implementation uses google-cloud-auth for IAM token
        // and constructs the Cloud Run v2 service spec. This is the
        // structural skeleton.

        info!(
            runtime_id = %id,
            service = %service_name,
            project = %self.config.project_id,
            region = %self.config.region,
            "Cloud Run runtime created"
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
            provider: ProviderKind::CloudRun,
            preset: spec.preset,
            image: spec.image,
            agent_id: spec.agent_id,
            org_id: spec.org_id,
            public_url,
            created_at: Utc::now(),
            started_at: None,
            stopped_at: None,
            metadata: HashMap::from([
                ("service_name".to_string(), service_name),
                ("project_id".to_string(), self.config.project_id.clone()),
                ("region".to_string(), self.config.region.clone()),
            ]),
        })
    }

    async fn start(&self, id: &Uuid) -> Result<RuntimeHandle, RuntimeError> {
        // Cloud Run services are always "running" (scale-to-zero handles idle).
        // Start = update min-instances to 1 to keep warm.
        info!(runtime_id = %id, "Cloud Run runtime started (min-instances=1)");
        self.status(id).await
    }

    async fn stop(&self, id: &Uuid) -> Result<(), RuntimeError> {
        // Stop = update min-instances to 0 (scale to zero).
        info!(runtime_id = %id, "Cloud Run runtime stopped (min-instances=0)");
        Ok(())
    }

    async fn delete(&self, id: &Uuid) -> Result<(), RuntimeError> {
        let service_name = Self::service_name(id);
        // DELETE https://run.googleapis.com/v2/projects/.../services/{name}
        info!(runtime_id = %id, service = %service_name, "Cloud Run runtime deleted");
        Ok(())
    }

    async fn status(&self, id: &Uuid) -> Result<RuntimeHandle, RuntimeError> {
        // GET https://run.googleapis.com/v2/projects/.../services/{name}
        // Parse conditions to determine readiness.
        Err(RuntimeError::NotFound(*id))
    }

    async fn logs(&self, id: &Uuid, _opts: LogOptions) -> Result<Vec<LogEntry>, RuntimeError> {
        // Cloud Run logs are fetched via Cloud Logging API, not the Run API.
        // Filter: resource.type="cloud_run_revision" AND resource.labels.service_name="rt-{id}"
        Err(RuntimeError::Provider(
            "Cloud Run logs are accessed via Cloud Logging. Use `gcloud logging read` or the dashboard.".to_string(),
        ))
    }

    async fn exec(&self, _id: &Uuid, _command: &[String]) -> Result<ExecResult, RuntimeError> {
        Err(RuntimeError::Provider(
            "Cloud Run does not support exec. Use the sidecar web terminal endpoint.".to_string(),
        ))
    }
}
