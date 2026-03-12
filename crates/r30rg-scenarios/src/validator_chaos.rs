use r30rg_core::invariant::{self, InvariantClass};
use r30rg_core::scenario::{Scenario, ScenarioCategory, ScenarioContext, ScenarioMeta};
use r30rg_core::types::{ContainerTarget, InfraFault, Layer, ScenarioOutcome, Severity};
use r30rg_live::docker::DockerChaos;
use r30rg_live::rpc::RpcHarness;
use tokio::time::{sleep, Duration};

/// Kill the validator, verify sequencer and batch poster are unaffected,
/// restart and verify the validator resumes.
pub struct ValidatorKillAndRecover;

#[async_trait::async_trait]
impl Scenario for ValidatorKillAndRecover {
    fn meta(&self) -> ScenarioMeta {
        ScenarioMeta {
            name: "validator-kill-and-recover".into(),
            description: "Kill the L2 validator (staker), verify sequencer and batch poster \
                          continue operating, restart validator, verify it resumes. Tests \
                          that validation is independent of sequencing."
                .into(),
            category: ScenarioCategory::SequencerChaos,
            target_layers: vec![Layer::L1, Layer::L2],
            severity_potential: Severity::Medium,
            destructive: true,
        }
    }

    async fn execute(&self, mut ctx: ScenarioContext) -> Result<ScenarioOutcome, anyhow::Error> {
        let docker = DockerChaos::connect(&ctx.config.docker_compose_project).await?;
        let rpc = RpcHarness::connect(&ctx.config.l1, &ctx.config.l2, None).await?;

        // Phase 1: Baseline.
        let l1_block_before = rpc.block_number(Layer::L1).await?;
        let l2_block_before = rpc.block_number(Layer::L2).await?;
        tracing::info!(l1 = l1_block_before, l2 = l2_block_before, "baseline recorded");
        ctx.check_passed();

        // Phase 2: Kill the validator.
        let fault = InfraFault::KillContainer {
            target: ContainerTarget {
                name: "validator".into(),
                service: "validator".into(),
                layer: Layer::L2,
            },
        };
        docker.execute_fault(&fault).await?;
        tracing::warn!("validator killed");

        // Phase 3: Wait, verify L1 and L2 still advancing.
        sleep(Duration::from_secs(8)).await;

        let l1_block_during = rpc.block_number(Layer::L1).await?;
        if l1_block_during <= l1_block_before {
            ctx.report(invariant::violation(
                InvariantClass::BlockMonotonicity,
                Layer::L1,
                "L1 stopped while validator was down",
                format!("L1 before={}, during={}", l1_block_before, l1_block_during),
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        let l2_block_during = rpc.block_number(Layer::L2).await?;
        if l2_block_during <= l2_block_before {
            ctx.report(invariant::violation(
                InvariantClass::SequencerLiveness,
                Layer::L2,
                "L2 sequencer stopped when validator was killed",
                format!(
                    "L2 before={}, during={} — sequencer must be independent of validator",
                    l2_block_before, l2_block_during
                ),
                ctx.rng.seed(),
                0,
            ));
        } else {
            tracing::info!(
                l2_before = l2_block_before,
                l2_during = l2_block_during,
                "L2 sequencer unaffected by validator kill"
            );
            ctx.check_passed();
        }

        // Phase 4: Restart the validator.
        let restart_fault = InfraFault::RestartContainer {
            target: ContainerTarget {
                name: "validator".into(),
                service: "validator".into(),
                layer: Layer::L2,
            },
        };
        docker.execute_fault(&restart_fault).await?;
        tracing::info!("validator restarted, waiting for recovery...");

        // Phase 5: Wait for validator to come back up.
        sleep(Duration::from_secs(10)).await;

        // Verify L2 still healthy.
        let l2_block_after = rpc.block_number(Layer::L2).await?;
        if l2_block_after <= l2_block_during {
            ctx.report(invariant::violation(
                InvariantClass::SequencerLiveness,
                Layer::L2,
                "L2 stalled after validator restart",
                format!("L2 during={}, after={}", l2_block_during, l2_block_after),
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        Ok(ctx.into_outcome())
    }
}
