use alloy_primitives::{Address, U256};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Which layer of the stack we're targeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Layer {
    L1,
    L2,
    L3,
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Layer::L1 => write!(f, "L1"),
            Layer::L2 => write!(f, "L2"),
            Layer::L3 => write!(f, "L3"),
        }
    }
}

/// RPC endpoint configuration for one layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerEndpoint {
    pub layer: Layer,
    pub rpc_url: String,
    pub ws_url: Option<String>,
    pub chain_id: u64,
}

/// Full stack target configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackConfig {
    pub l1: LayerEndpoint,
    pub l2: LayerEndpoint,
    pub l3: Option<LayerEndpoint>,
    pub docker_compose_project: String,
    pub docker_compose_dir: String,
    /// L1 Inbox contract address (from rollup deployment). Required for L1→L2 deposit tests.
    pub inbox_addr: Option<String>,
    /// L1 Bridge contract address (from rollup deployment). Used for bridge balance checks.
    pub bridge_addr: Option<String>,
    /// ExpressLaneAuction contract address (from timeboost deployment). Used for timeboost probing.
    pub auction_addr: Option<String>,
}

impl Default for StackConfig {
    fn default() -> Self {
        Self {
            l1: LayerEndpoint {
                layer: Layer::L1,
                rpc_url: "http://127.0.0.1:8545".into(),
                ws_url: Some("ws://127.0.0.1:8546".into()),
                chain_id: 1337,
            },
            l2: LayerEndpoint {
                layer: Layer::L2,
                rpc_url: "http://127.0.0.1:8547".into(),
                ws_url: Some("ws://127.0.0.1:8548".into()),
                chain_id: 412346,
            },
            l3: Some(LayerEndpoint {
                layer: Layer::L3,
                rpc_url: "http://127.0.0.1:3347".into(),
                ws_url: Some("ws://127.0.0.1:3348".into()),
                chain_id: 0, // discovered at runtime
            }),
            docker_compose_project: "nitro-testnode-live".into(),
            docker_compose_dir: String::new(),
            inbox_addr: None,
            bridge_addr: None,
            auction_addr: None,
        }
    }
}

/// Severity of a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Info => write!(f, "INFO"),
            Severity::Low => write!(f, "LOW"),
            Severity::Medium => write!(f, "MEDIUM"),
            Severity::High => write!(f, "HIGH"),
            Severity::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// A finding produced by an invariant check or scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    pub description: String,
    pub layer: Option<Layer>,
    pub scenario: String,
    pub seed: u64,
    pub tick: u64,
    pub evidence: serde_json::Value,
}

/// Outcome of a single scenario run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScenarioOutcome {
    Passed {
        duration_ms: u64,
        checks_run: u64,
    },
    Failed {
        duration_ms: u64,
        findings: Vec<Finding>,
    },
    Error {
        message: String,
    },
}

impl ScenarioOutcome {
    pub fn is_pass(&self) -> bool {
        matches!(self, ScenarioOutcome::Passed { .. })
    }
}

/// A snapshot of balances across layers for consistency checking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceSnapshot {
    pub address: Address,
    pub l1_balance: U256,
    pub l2_balance: U256,
    pub l3_balance: Option<U256>,
    pub block_l1: u64,
    pub block_l2: u64,
    pub block_l3: Option<u64>,
}

/// Container identity for Docker fault injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerTarget {
    pub name: String,
    pub service: String,
    pub layer: Layer,
}

/// What kind of fault to inject at the infrastructure level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InfraFault {
    PauseContainer { target: ContainerTarget, duration_ms: u64 },
    KillContainer { target: ContainerTarget },
    RestartContainer { target: ContainerTarget },
    NetworkPartition { targets: Vec<ContainerTarget>, duration_ms: u64 },
    DiskPressure { target: ContainerTarget },
}

impl fmt::Display for InfraFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InfraFault::PauseContainer { target, duration_ms } => {
                write!(f, "PAUSE {} for {}ms", target.name, duration_ms)
            }
            InfraFault::KillContainer { target } => {
                write!(f, "KILL {}", target.name)
            }
            InfraFault::RestartContainer { target } => {
                write!(f, "RESTART {}", target.name)
            }
            InfraFault::NetworkPartition { targets, duration_ms } => {
                let names: Vec<_> = targets.iter().map(|t| t.name.as_str()).collect();
                write!(f, "PARTITION [{}] for {}ms", names.join(", "), duration_ms)
            }
            InfraFault::DiskPressure { target } => {
                write!(f, "DISK_PRESSURE {}", target.name)
            }
        }
    }
}
