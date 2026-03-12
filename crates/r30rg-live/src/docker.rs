use bollard::container::{
    KillContainerOptions, ListContainersOptions, RestartContainerOptions,
};
use bollard::Docker;
use r30rg_core::types::{ContainerTarget, InfraFault};
use std::collections::HashMap;
use tokio::time::{sleep, Duration};

/// Docker fault injector — executes infrastructure-level chaos against containers.
pub struct DockerChaos {
    client: Docker,
    project: String,
}

impl DockerChaos {
    pub async fn connect(project: &str) -> anyhow::Result<Self> {
        let client = Docker::connect_with_local_defaults()?;
        client.ping().await?;
        tracing::info!(project = project, "connected to Docker daemon");
        Ok(Self {
            client,
            project: project.to_string(),
        })
    }

    /// Resolve a service name to its container ID.
    /// When `running_only` is true, only matches running containers.
    /// When false, matches any status (needed after kill to find exited containers).
    async fn resolve_container(&self, service: &str, running_only: bool) -> anyhow::Result<String> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![format!("com.docker.compose.project={}", self.project)],
        );
        if running_only {
            filters.insert("status".to_string(), vec!["running".to_string()]);
        }

        let containers = self
            .client
            .list_containers(Some(ListContainersOptions {
                all: !running_only,
                filters,
                ..Default::default()
            }))
            .await?;

        for c in &containers {
            let labels = c.labels.as_ref();
            if let Some(labels) = labels {
                if labels.get("com.docker.compose.service") == Some(&service.to_string()) {
                    if let Some(id) = &c.id {
                        return Ok(id.clone());
                    }
                }
            }
        }

        anyhow::bail!("container for service '{}' not found in project '{}'", service, self.project)
    }

    /// Execute an infrastructure fault.
    pub async fn execute_fault(&self, fault: &InfraFault) -> anyhow::Result<()> {
        match fault {
            InfraFault::PauseContainer { target, duration_ms } => {
                self.pause_container(target, *duration_ms).await
            }
            InfraFault::KillContainer { target } => self.kill_container(target).await,
            InfraFault::RestartContainer { target } => self.restart_container(target).await,
            InfraFault::NetworkPartition {
                targets,
                duration_ms,
            } => self.network_partition(targets, *duration_ms).await,
            InfraFault::DiskPressure { target } => {
                tracing::warn!(target = %target.name, "disk pressure not yet implemented");
                Ok(())
            }
        }
    }

    async fn pause_container(&self, target: &ContainerTarget, duration_ms: u64) -> anyhow::Result<()> {
        let id = self.resolve_container(&target.service, true).await?;
        tracing::warn!(
            service = %target.service,
            duration_ms = duration_ms,
            "CHAOS: pausing container"
        );
        self.client.pause_container(&id).await?;

        let client = self.client.clone();
        let id_clone = id.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(duration_ms)).await;
            if let Err(e) = client.unpause_container(&id_clone).await {
                tracing::error!(error = %e, "failed to unpause container");
            } else {
                tracing::info!(container = %id_clone, "CHAOS: unpaused container");
            }
        });

        Ok(())
    }

    async fn kill_container(&self, target: &ContainerTarget) -> anyhow::Result<()> {
        let id = self.resolve_container(&target.service, true).await?;
        tracing::warn!(service = %target.service, "CHAOS: killing container");
        self.client
            .kill_container(&id, Some(KillContainerOptions { signal: "SIGKILL" }))
            .await?;
        Ok(())
    }

    async fn restart_container(&self, target: &ContainerTarget) -> anyhow::Result<()> {
        let id = self.resolve_container(&target.service, false).await?;
        tracing::warn!(service = %target.service, "CHAOS: restarting container");
        self.client
            .restart_container(&id, Some(RestartContainerOptions { t: 5 }))
            .await?;
        Ok(())
    }

    async fn network_partition(
        &self,
        targets: &[ContainerTarget],
        duration_ms: u64,
    ) -> anyhow::Result<()> {
        let names: Vec<_> = targets.iter().map(|t| t.service.as_str()).collect();
        tracing::warn!(
            targets = ?names,
            duration_ms = duration_ms,
            "CHAOS: network partition (via pause)"
        );
        // Approximate partition via container pause (true iptables partition
        // requires privileged access; pause is the pragmatic Docker approach).
        let mut ids = Vec::new();
        for t in targets {
            match self.resolve_container(&t.service, true).await {
                Ok(id) => {
                    self.client.pause_container(&id).await?;
                    ids.push(id);
                }
                Err(e) => {
                    tracing::warn!(service = %t.service, error = %e, "skipping partition target");
                }
            }
        }

        let client = self.client.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(duration_ms)).await;
            for id in &ids {
                if let Err(e) = client.unpause_container(id).await {
                    tracing::error!(error = %e, container = %id, "failed to heal partition");
                }
            }
            tracing::info!("CHAOS: partition healed");
        });

        Ok(())
    }

    /// List all running containers in the project.
    pub async fn list_services(&self) -> anyhow::Result<Vec<String>> {
        let mut filters = HashMap::new();
        filters.insert(
            "label".to_string(),
            vec![format!("com.docker.compose.project={}", self.project)],
        );
        filters.insert("status".to_string(), vec!["running".to_string()]);

        let containers = self
            .client
            .list_containers(Some(ListContainersOptions {
                filters,
                ..Default::default()
            }))
            .await?;

        let mut services = Vec::new();
        for c in containers {
            if let Some(labels) = c.labels {
                if let Some(svc) = labels.get("com.docker.compose.service") {
                    services.push(svc.clone());
                }
            }
        }
        services.sort();
        Ok(services)
    }
}
