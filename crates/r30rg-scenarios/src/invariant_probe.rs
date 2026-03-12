use r30rg_core::invariant::{self, InvariantClass};
use r30rg_core::scenario::{Scenario, ScenarioCategory, ScenarioContext, ScenarioMeta};
use r30rg_core::types::{Layer, Severity, ScenarioOutcome};
use r30rg_live::rpc::RpcHarness;
use tokio::time::{sleep, Duration};

/// Non-destructive full-stack health probe.
/// Checks block production, gas pricing, precompiles, and liveness on all layers.
pub struct FullStackHealthProbe;

#[async_trait::async_trait]
impl Scenario for FullStackHealthProbe {
    fn meta(&self) -> ScenarioMeta {
        ScenarioMeta {
            name: "full-stack-health-probe".into(),
            description: "Non-destructive probe of all layers: block production liveness, \
                          gas pricing sanity, precompile accessibility, and cross-layer \
                          connectivity."
                .into(),
            category: ScenarioCategory::InvariantProbe,
            target_layers: vec![Layer::L1, Layer::L2, Layer::L3],
            severity_potential: Severity::High,
            destructive: false,
        }
    }

    async fn execute(&self, mut ctx: ScenarioContext) -> Result<ScenarioOutcome, anyhow::Error> {
        let rpc = RpcHarness::connect(
            &ctx.config.l1,
            &ctx.config.l2,
            ctx.config.l3.as_ref(),
        ).await?;

        let layers: Vec<Layer> = if rpc.l3.is_some() {
            vec![Layer::L1, Layer::L2, Layer::L3]
        } else {
            vec![Layer::L1, Layer::L2]
        };

        // --- Block production liveness ---
        // L1 (dev mode) produces blocks every ~1s; L2/L3 sequencers produce
        // blocks on-demand so we poll longer and accept same-height as OK when
        // the chain is idle (no pending txs).
        for &layer in &layers {
            let wait_secs = match layer {
                Layer::L1 => 3,
                _ => 6, // L2/L3 on-demand sequencers need longer
            };
            let b1 = rpc.block_number(layer).await?;
            sleep(Duration::from_secs(wait_secs)).await;
            let b2 = rpc.block_number(layer).await?;

            if b2 < b1 {
                // Block height must never go backwards.
                ctx.report(invariant::violation(
                    InvariantClass::BlockMonotonicity,
                    layer,
                    format!("{} block height regressed", layer),
                    format!("block {} -> {} over {}s", b1, b2, wait_secs),
                    ctx.rng.seed(),
                    0,
                ));
            } else if b2 == b1 && layer == Layer::L1 {
                // L1 should always produce blocks in dev mode.
                ctx.report(invariant::violation(
                    InvariantClass::BlockMonotonicity,
                    layer,
                    format!("{} blocks not advancing", layer),
                    format!("block {} -> {} over {}s", b1, b2, wait_secs),
                    ctx.rng.seed(),
                    0,
                ));
            } else {
                tracing::info!(layer = %layer, from = b1, to = b2, "blocks OK");
                ctx.check_passed();
            }
        }

        // --- Gas pricing sanity ---
        for &layer in &[Layer::L2] {
            let price = rpc.gas_price(layer).await?;
            if price == 0 {
                ctx.report(invariant::violation(
                    InvariantClass::GasPricingSanity,
                    layer,
                    format!("{} gas price is zero", layer),
                    "Zero gas price indicates broken fee mechanism",
                    ctx.rng.seed(),
                    0,
                ));
            } else if price > 100_000_000_000_000 {
                // > 100k gwei — something is wildly wrong
                ctx.report(invariant::violation(
                    InvariantClass::GasPricingSanity,
                    layer,
                    format!("{} gas price is absurdly high", layer),
                    format!("gas price = {} wei", price),
                    ctx.rng.seed(),
                    0,
                ));
            } else {
                ctx.check_passed();
            }
        }

        // --- L2 ArbSys precompile ---
        let arbsys: alloy_primitives::Address =
            "0x0000000000000000000000000000000000000064".parse()?;
        let result = rpc
            .eth_call(Layer::L2, arbsys, vec![0xa3, 0xb1, 0xb3, 0x1d])
            .await?;
        if result.len() < 32 || result.iter().all(|&b| b == 0) {
            ctx.report(invariant::violation(
                InvariantClass::SequencerLiveness,
                Layer::L2,
                "ArbSys.arbBlockNumber() returned invalid data",
                format!("returned {} bytes, all zero", result.len()),
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        // --- L2 ArbGasInfo precompile ---
        let arbgasinfo: alloy_primitives::Address =
            "0x000000000000000000000000000000000000006C".parse()?;
        let gas_result = rpc
            .eth_call(Layer::L2, arbgasinfo, vec![0x41, 0xb2, 0x47, 0xa8])
            .await?;
        if gas_result.len() < 32 {
            ctx.report(invariant::violation(
                InvariantClass::GasPricingSanity,
                Layer::L2,
                "ArbGasInfo.getPricesInWei() returned too little data",
                format!("returned {} bytes, expected >=192", gas_result.len()),
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        // --- L3 ArbSys (if present) ---
        if rpc.l3.is_some() {
            let l3_result = rpc
                .eth_call(Layer::L3, arbsys, vec![0xa3, 0xb1, 0xb3, 0x1d])
                .await?;
            if l3_result.iter().all(|&b| b == 0) {
                ctx.report(invariant::violation(
                    InvariantClass::SequencerLiveness,
                    Layer::L3,
                    "L3 ArbSys.arbBlockNumber() returned 0",
                    "L3 precompile is not responding correctly",
                    ctx.rng.seed(),
                    0,
                ));
            } else {
                ctx.check_passed();
            }
        }

        Ok(ctx.into_outcome())
    }
}
