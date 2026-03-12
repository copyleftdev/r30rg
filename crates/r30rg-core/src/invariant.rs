use crate::types::{Finding, Layer, Severity};
use serde::{Deserialize, Serialize};

/// Invariant category — what class of property is being checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InvariantClass {
    /// Balance consistency across layers (no money created/destroyed outside bridges).
    BalanceConsistency,
    /// Blocks are monotonically advancing.
    BlockMonotonicity,
    /// Batch poster is posting to L1 within expected intervals.
    BatchPostingLiveness,
    /// Sequencer is producing blocks within expected intervals.
    SequencerLiveness,
    /// Validator is confirming assertions.
    ValidatorLiveness,
    /// L1→L2 retryable tickets resolve (don't get stuck).
    RetryableResolution,
    /// L2→L3 messaging is functional.
    CrossChainMessaging,
    /// No double-spend or balance inflation.
    DoubleSpendProtection,
    /// State roots are consistent between sequencer and validator.
    StateRootConsistency,
    /// Gas pricing is sane (no zero or extreme values).
    GasPricingSanity,
}

/// Result of checking a single invariant.
#[derive(Debug)]
pub enum InvariantResult {
    /// Invariant holds.
    Holds,
    /// Invariant is violated.
    Violated(Finding),
    /// Could not check (e.g. RPC down — which may itself be a finding).
    Unavailable(String),
}

impl InvariantResult {
    pub fn is_violated(&self) -> bool {
        matches!(self, InvariantResult::Violated(_))
    }
}

/// Builder for invariant-violation findings.
pub fn violation(
    class: InvariantClass,
    layer: Layer,
    title: impl Into<String>,
    description: impl Into<String>,
    seed: u64,
    tick: u64,
) -> Finding {
    Finding {
        id: uuid::Uuid::new_v4().to_string(),
        severity: class_severity(class),
        title: title.into(),
        description: description.into(),
        layer: Some(layer),
        scenario: format!("invariant::{:?}", class),
        seed,
        tick,
        evidence: serde_json::Value::Null,
    }
}

fn class_severity(class: InvariantClass) -> Severity {
    match class {
        InvariantClass::DoubleSpendProtection => Severity::Critical,
        InvariantClass::BalanceConsistency => Severity::Critical,
        InvariantClass::StateRootConsistency => Severity::Critical,
        InvariantClass::RetryableResolution => Severity::High,
        InvariantClass::BatchPostingLiveness => Severity::High,
        InvariantClass::SequencerLiveness => Severity::High,
        InvariantClass::ValidatorLiveness => Severity::Medium,
        InvariantClass::CrossChainMessaging => Severity::Medium,
        InvariantClass::BlockMonotonicity => Severity::Medium,
        InvariantClass::GasPricingSanity => Severity::Low,
    }
}
