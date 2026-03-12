use crate::prng::DeterministicRng;
use crate::types::{ContainerTarget, InfraFault, Layer};
use serde::{Deserialize, Serialize};

/// Chaos profile — controls fault injection intensity.
/// Inspired by TigerBeetle's VOPR: every decision is seeded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChaosProfile {
    /// Probability of pausing a container per tick (0.0–1.0).
    pub container_pause_rate: f64,
    /// Probability of killing a container per tick.
    pub container_kill_rate: f64,
    /// Probability of injecting a network partition per tick.
    pub network_partition_rate: f64,
    /// Min/max pause duration in ms.
    pub pause_duration_min_ms: u64,
    pub pause_duration_max_ms: u64,
    /// Min/max partition duration in ms.
    pub partition_duration_min_ms: u64,
    pub partition_duration_max_ms: u64,
    /// Whether to target L1 (extremely destructive).
    pub target_l1: bool,
    /// Whether to target L3.
    pub target_l3: bool,
}

impl ChaosProfile {
    /// Gentle — low fault rates, no kills, short pauses.
    pub fn gentle() -> Self {
        Self {
            container_pause_rate: 0.001,
            container_kill_rate: 0.0,
            network_partition_rate: 0.0005,
            pause_duration_min_ms: 500,
            pause_duration_max_ms: 2_000,
            partition_duration_min_ms: 1_000,
            partition_duration_max_ms: 5_000,
            target_l1: false,
            target_l3: false,
        }
    }

    /// Moderate — some kills, longer faults.
    pub fn moderate() -> Self {
        Self {
            container_pause_rate: 0.005,
            container_kill_rate: 0.001,
            network_partition_rate: 0.002,
            pause_duration_min_ms: 1_000,
            pause_duration_max_ms: 10_000,
            partition_duration_min_ms: 5_000,
            partition_duration_max_ms: 30_000,
            target_l1: false,
            target_l3: true,
        }
    }

    /// Apocalyptic — high fault rates, kills, L1 targeting, long partitions.
    pub fn apocalyptic() -> Self {
        Self {
            container_pause_rate: 0.02,
            container_kill_rate: 0.005,
            network_partition_rate: 0.01,
            pause_duration_min_ms: 5_000,
            pause_duration_max_ms: 60_000,
            partition_duration_min_ms: 10_000,
            partition_duration_max_ms: 120_000,
            target_l1: true,
            target_l3: true,
        }
    }
}

/// Known container services in the nitro-testnode stack.
pub fn testnode_containers() -> Vec<ContainerTarget> {
    vec![
        ContainerTarget {
            name: "sequencer".into(),
            service: "sequencer".into(),
            layer: Layer::L2,
        },
        ContainerTarget {
            name: "poster".into(),
            service: "poster".into(),
            layer: Layer::L2,
        },
        ContainerTarget {
            name: "validator".into(),
            service: "validator".into(),
            layer: Layer::L2,
        },
        ContainerTarget {
            name: "validation_node".into(),
            service: "validation_node".into(),
            layer: Layer::L2,
        },
        ContainerTarget {
            name: "l3node".into(),
            service: "l3node".into(),
            layer: Layer::L3,
        },
        ContainerTarget {
            name: "geth".into(),
            service: "geth".into(),
            layer: Layer::L1,
        },
        ContainerTarget {
            name: "redis".into(),
            service: "redis".into(),
            layer: Layer::L2,
        },
        ContainerTarget {
            name: "timeboost-auctioneer".into(),
            service: "timeboost-auctioneer".into(),
            layer: Layer::L2,
        },
    ]
}

/// Fault generator — uses the deterministic PRNG to decide what to break.
pub struct FaultGenerator {
    profile: ChaosProfile,
    containers: Vec<ContainerTarget>,
}

impl FaultGenerator {
    pub fn new(profile: ChaosProfile) -> Self {
        let mut containers = testnode_containers();
        if !profile.target_l1 {
            containers.retain(|c| c.layer != Layer::L1);
        }
        if !profile.target_l3 {
            containers.retain(|c| c.layer != Layer::L3);
        }
        Self { profile, containers }
    }

    /// Roll the dice and maybe produce a fault for this tick.
    pub fn maybe_inject(&self, rng: &mut DeterministicRng) -> Option<InfraFault> {
        if self.containers.is_empty() {
            return None;
        }

        if rng.chance(self.profile.container_kill_rate) {
            let target = rng.pick(&self.containers).clone();
            return Some(InfraFault::KillContainer { target });
        }

        if rng.chance(self.profile.container_pause_rate) {
            let target = rng.pick(&self.containers).clone();
            let duration_ms = rng.range(
                self.profile.pause_duration_min_ms,
                self.profile.pause_duration_max_ms,
            );
            return Some(InfraFault::PauseContainer {
                target,
                duration_ms,
            });
        }

        if rng.chance(self.profile.network_partition_rate) {
            let count = rng.range(1, self.containers.len().min(3) as u64) as usize;
            let mut targets: Vec<ContainerTarget> = self.containers.clone();
            rng.shuffle(&mut targets);
            targets.truncate(count);
            let duration_ms = rng.range(
                self.profile.partition_duration_min_ms,
                self.profile.partition_duration_max_ms,
            );
            return Some(InfraFault::NetworkPartition {
                targets,
                duration_ms,
            });
        }

        None
    }
}
