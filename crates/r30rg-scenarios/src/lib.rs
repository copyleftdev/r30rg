pub mod sequencer_chaos;
pub mod batch_poster_chaos;
pub mod validator_chaos;
pub mod bridge_adversarial;
pub mod invariant_probe;
pub mod retryable_probe;
pub mod bridge_stress;
pub mod timeboost_adversarial;

use r30rg_core::scenario::Scenario;

/// Registry of all available scenarios.
pub fn all_scenarios() -> Vec<Box<dyn Scenario>> {
    vec![
        Box::new(sequencer_chaos::SequencerKillAndRecover),
        Box::new(batch_poster_chaos::BatchPosterKillAndRecover),
        Box::new(validator_chaos::ValidatorKillAndRecover),
        Box::new(bridge_adversarial::BalanceConsistencyProbe),
        Box::new(invariant_probe::FullStackHealthProbe),
        Box::new(retryable_probe::PrecompileSurfaceProbe),
        Box::new(bridge_stress::BridgeDepositWithdrawStress),
        Box::new(timeboost_adversarial::TimeboostAuctionProbe),
    ]
}
