use r30rg_core::invariant::{self, InvariantClass};
use r30rg_core::scenario::{Scenario, ScenarioCategory, ScenarioContext, ScenarioMeta};
use r30rg_core::types::{ContainerTarget, Finding, InfraFault, Layer, ScenarioOutcome, Severity};
use r30rg_live::docker::DockerChaos;
use r30rg_live::rpc::RpcHarness;
use tokio::time::{sleep, Duration};

/// Kill the sequencer, verify L2 stops producing blocks, restart it,
/// verify it recovers and blocks resume — all while checking L1 is unaffected.
pub struct SequencerKillAndRecover;

#[async_trait::async_trait]
impl Scenario for SequencerKillAndRecover {
    fn meta(&self) -> ScenarioMeta {
        ScenarioMeta {
            name: "sequencer-kill-and-recover".into(),
            description: "Kill the L2 sequencer, verify halt, restart, verify recovery. \
                          L1 must remain unaffected throughout."
                .into(),
            category: ScenarioCategory::SequencerChaos,
            target_layers: vec![Layer::L1, Layer::L2],
            severity_potential: Severity::High,
            destructive: true,
        }
    }

    async fn execute(&self, mut ctx: ScenarioContext) -> Result<ScenarioOutcome, anyhow::Error> {
        let docker = DockerChaos::connect(&ctx.config.docker_compose_project).await?;
        let rpc = RpcHarness::connect(&ctx.config.l1, &ctx.config.l2, None).await?;

        // Phase 1: Record baseline.
        let l1_block_before = rpc.block_number(Layer::L1).await?;
        let l2_block_before = rpc.block_number(Layer::L2).await?;
        tracing::info!(l1 = l1_block_before, l2 = l2_block_before, "baseline recorded");
        ctx.check_passed();

        // Phase 2: Kill the sequencer.
        let fault = InfraFault::KillContainer {
            target: ContainerTarget {
                name: "sequencer".into(),
                service: "sequencer".into(),
                layer: Layer::L2,
            },
        };
        docker.execute_fault(&fault).await?;
        tracing::warn!("sequencer killed");

        // Phase 3: Verify L1 keeps producing blocks (should be unaffected).
        sleep(Duration::from_secs(5)).await;
        let l1_block_during = rpc.block_number(Layer::L1).await?;
        if l1_block_during <= l1_block_before {
            ctx.report(invariant::violation(
                InvariantClass::BlockMonotonicity,
                Layer::L1,
                "L1 stopped while sequencer was down",
                format!(
                    "L1 block before={}, during={}",
                    l1_block_before, l1_block_during
                ),
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        // Phase 4: Verify L2 is indeed down (RPC should fail or block stale).
        let l2_down = rpc.block_number(Layer::L2).await;
        match l2_down {
            Ok(block) => {
                // If it somehow advanced a lot, that's suspicious.
                if block > l2_block_before + 5 {
                    ctx.report(Finding {
                        id: uuid::Uuid::new_v4().to_string(),
                        severity: Severity::Medium,
                        title: "L2 still advancing after sequencer kill".into(),
                        description: format!(
                            "L2 went from {} to {} despite sequencer being killed",
                            l2_block_before, block
                        ),
                        layer: Some(Layer::L2),
                        scenario: self.meta().name.clone(),
                        seed: ctx.rng.seed(),
                        tick: 0,
                        evidence: serde_json::json!({"before": l2_block_before, "during": block}),
                    });
                } else {
                    ctx.check_passed(); // Expected: stale or barely moved.
                }
            }
            Err(_) => {
                ctx.check_passed(); // Expected: RPC unreachable.
            }
        }

        // Phase 5: Restart the sequencer.
        let restart_fault = InfraFault::RestartContainer {
            target: ContainerTarget {
                name: "sequencer".into(),
                service: "sequencer".into(),
                layer: Layer::L2,
            },
        };
        docker.execute_fault(&restart_fault).await?;
        tracing::info!("sequencer restarted, waiting for recovery...");

        // Phase 6: Wait for L2 to recover (up to 60s).
        let mut recovered = false;
        for attempt in 0..12 {
            sleep(Duration::from_secs(5)).await;
            match rpc.block_number(Layer::L2).await {
                Ok(block) if block > l2_block_before => {
                    tracing::info!(
                        attempt = attempt,
                        l2_block = block,
                        "L2 recovered"
                    );
                    recovered = true;
                    ctx.check_passed();
                    break;
                }
                _ => continue,
            }
        }

        if !recovered {
            ctx.report(invariant::violation(
                InvariantClass::SequencerLiveness,
                Layer::L2,
                "L2 sequencer did not recover after restart",
                "L2 block number did not advance within 60s of restart",
                ctx.rng.seed(),
                0,
            ));
        }

        // Phase 7: Final L1 sanity check.
        let l1_block_after = rpc.block_number(Layer::L1).await?;
        if l1_block_after <= l1_block_during {
            ctx.report(invariant::violation(
                InvariantClass::BlockMonotonicity,
                Layer::L1,
                "L1 stalled during recovery phase",
                format!(
                    "L1 during={}, after={}",
                    l1_block_during, l1_block_after
                ),
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        Ok(ctx.into_outcome())
    }
}
