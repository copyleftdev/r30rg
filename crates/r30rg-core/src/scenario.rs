use crate::types::{Finding, ScenarioOutcome, StackConfig};
use crate::prng::DeterministicRng;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Metadata describing a chaos scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioMeta {
    pub name: String,
    pub description: String,
    pub category: ScenarioCategory,
    pub target_layers: Vec<crate::types::Layer>,
    pub severity_potential: crate::types::Severity,
    pub destructive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ScenarioCategory {
    /// Transaction-level adversarial attacks.
    TransactionAdversarial,
    /// Infrastructure fault injection (containers, network).
    InfrastructureChaos,
    /// Cross-chain messaging attacks.
    BridgeAdversarial,
    /// Sequencer/batch poster misbehavior.
    SequencerChaos,
    /// Validator/BOLD dispute protocol testing.
    DisputeAdversarial,
    /// Timeboost auction manipulation.
    TimeboostAdversarial,
    /// State consistency invariant probing.
    InvariantProbe,
    /// Deterministic simulation (no live infra needed).
    Simulation,
}

impl fmt::Display for ScenarioCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScenarioCategory::TransactionAdversarial => write!(f, "tx-adversarial"),
            ScenarioCategory::InfrastructureChaos => write!(f, "infra-chaos"),
            ScenarioCategory::BridgeAdversarial => write!(f, "bridge-adversarial"),
            ScenarioCategory::SequencerChaos => write!(f, "sequencer-chaos"),
            ScenarioCategory::DisputeAdversarial => write!(f, "dispute-adversarial"),
            ScenarioCategory::TimeboostAdversarial => write!(f, "timeboost-adversarial"),
            ScenarioCategory::InvariantProbe => write!(f, "invariant-probe"),
            ScenarioCategory::Simulation => write!(f, "simulation"),
        }
    }
}

/// The trait every chaos scenario must implement.
///
/// Scenarios are deterministic: given the same seed and stack state,
/// they produce the same fault injection sequence and invariant checks.
#[async_trait::async_trait]
pub trait Scenario: Send + Sync {
    /// Metadata about this scenario.
    fn meta(&self) -> ScenarioMeta;

    /// Execute the scenario against a live stack.
    /// The `rng` is seeded deterministically — all random decisions must use it.
    /// Takes ownership of ctx so it can be consumed into the final outcome.
    async fn execute(
        &self,
        ctx: ScenarioContext,
    ) -> Result<ScenarioOutcome, anyhow::Error>;
}

/// Runtime context passed to every scenario execution.
pub struct ScenarioContext {
    pub rng: DeterministicRng,
    pub config: StackConfig,
    pub findings: Vec<Finding>,
    pub checks_run: u64,
    pub start_time: std::time::Instant,
}

impl ScenarioContext {
    pub fn new(seed: u64, config: StackConfig) -> Self {
        Self {
            rng: DeterministicRng::new(seed),
            config,
            findings: Vec::new(),
            checks_run: 0,
            start_time: std::time::Instant::now(),
        }
    }

    /// Record a finding.
    pub fn report(&mut self, finding: Finding) {
        tracing::warn!(
            severity = %finding.severity,
            title = %finding.title,
            "r30rg finding"
        );
        self.findings.push(finding);
    }

    /// Record a passed check.
    pub fn check_passed(&mut self) {
        self.checks_run += 1;
    }

    /// Elapsed wall-clock ms since scenario start.
    pub fn elapsed_ms(&self) -> u64 {
        self.start_time.elapsed().as_millis() as u64
    }

    /// Build the final outcome.
    pub fn into_outcome(self) -> ScenarioOutcome {
        let duration_ms = self.elapsed_ms();
        if self.findings.is_empty() {
            ScenarioOutcome::Passed {
                duration_ms,
                checks_run: self.checks_run,
            }
        } else {
            ScenarioOutcome::Failed {
                duration_ms,
                findings: self.findings,
            }
        }
    }
}
