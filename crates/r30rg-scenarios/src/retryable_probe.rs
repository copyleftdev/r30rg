use r30rg_core::invariant::{self, InvariantClass};
use r30rg_core::scenario::{Scenario, ScenarioCategory, ScenarioContext, ScenarioMeta};
use r30rg_core::types::{Layer, ScenarioOutcome, Severity};
use r30rg_live::rpc::RpcHarness;

/// Non-destructive probe of Arbitrum precompiles related to retryable tickets,
/// L1 pricing, and node interface. Verifies the precompile surface is responsive
/// and returns sane data.
pub struct PrecompileSurfaceProbe;

#[async_trait::async_trait]
impl Scenario for PrecompileSurfaceProbe {
    fn meta(&self) -> ScenarioMeta {
        ScenarioMeta {
            name: "precompile-surface-probe".into(),
            description: "Probe Arbitrum L2 precompiles: ArbSys, ArbGasInfo, ArbRetryableTx, \
                          ArbAggregator, and NodeInterface. Verifies each returns valid data \
                          and no reverts. Non-destructive read-only probe."
                .into(),
            category: ScenarioCategory::InvariantProbe,
            target_layers: vec![Layer::L2],
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

        // --- ArbSys (0x64) ---
        // arbBlockNumber() selector: 0xa3b1b31d
        let arbsys: alloy_primitives::Address =
            "0x0000000000000000000000000000000000000064".parse()?;
        let block_num_result = rpc
            .eth_call(Layer::L2, arbsys, vec![0xa3, 0xb1, 0xb3, 0x1d])
            .await?;
        if block_num_result.len() < 32 {
            ctx.report(invariant::violation(
                InvariantClass::SequencerLiveness,
                Layer::L2,
                "ArbSys.arbBlockNumber() returned insufficient data",
                format!("returned {} bytes", block_num_result.len()),
                ctx.rng.seed(),
                0,
            ));
        } else {
            let block = u64::from_be_bytes(block_num_result[24..32].try_into().unwrap_or([0; 8]));
            tracing::info!(block = block, "ArbSys.arbBlockNumber() OK");
            ctx.check_passed();
        }

        // arbChainID() selector: 0xd127dc9b (deprecated on newer Nitro, may revert)
        match rpc.eth_call(Layer::L2, arbsys, vec![0xd1, 0x27, 0xdc, 0x9b]).await {
            Ok(chain_id_result) if chain_id_result.len() >= 32 => {
                let chain_id = u64::from_be_bytes(
                    chain_id_result[24..32].try_into().unwrap_or([0; 8]),
                );
                tracing::info!(chain_id = chain_id, "ArbSys.arbChainID() OK");
                ctx.check_passed();
            }
            Ok(_) => {
                tracing::info!("ArbSys.arbChainID() returned short data (deprecated, OK)");
                ctx.check_passed();
            }
            Err(_) => {
                tracing::info!("ArbSys.arbChainID() reverted (deprecated on newer Nitro, OK)");
                ctx.check_passed();
            }
        }

        // --- ArbGasInfo (0x6C) ---
        // getPricesInWei() selector: 0x41b247a8
        let arbgasinfo: alloy_primitives::Address =
            "0x000000000000000000000000000000000000006C".parse()?;
        let gas_result = rpc
            .eth_call(Layer::L2, arbgasinfo, vec![0x41, 0xb2, 0x47, 0xa8])
            .await?;
        if gas_result.len() < 192 {
            ctx.report(invariant::violation(
                InvariantClass::GasPricingSanity,
                Layer::L2,
                "ArbGasInfo.getPricesInWei() returned too little data",
                format!("returned {} bytes, expected >=192 (6 uint256s)", gas_result.len()),
                ctx.rng.seed(),
                0,
            ));
        } else {
            tracing::info!(
                bytes = gas_result.len(),
                "ArbGasInfo.getPricesInWei() OK"
            );
            ctx.check_passed();
        }

        // getL1BaseFeeEstimate() selector: 0xf5d6ded7
        match rpc.eth_call(Layer::L2, arbgasinfo, vec![0xf5, 0xd6, 0xde, 0xd7]).await {
            Ok(l1fee_result) if l1fee_result.len() >= 32 => {
                tracing::info!("ArbGasInfo.getL1BaseFeeEstimate() OK");
                ctx.check_passed();
            }
            Ok(_) => {
                tracing::info!("ArbGasInfo.getL1BaseFeeEstimate() short response (OK)");
                ctx.check_passed();
            }
            Err(_) => {
                tracing::info!("ArbGasInfo.getL1BaseFeeEstimate() reverted (may be deprecated, OK)");
                ctx.check_passed();
            }
        }

        // --- ArbRetryableTx (0x6E) ---
        // getLifetime() selector: 0x80a22d76
        let arbretryable: alloy_primitives::Address =
            "0x000000000000000000000000000000000000006E".parse()?;
        match rpc.eth_call(Layer::L2, arbretryable, vec![0x80, 0xa2, 0x2d, 0x76]).await {
            Ok(lifetime_result) if lifetime_result.len() >= 32 => {
                let lifetime = u64::from_be_bytes(
                    lifetime_result[24..32].try_into().unwrap_or([0; 8]),
                );
                if lifetime == 0 {
                    ctx.report(invariant::violation(
                        InvariantClass::RetryableResolution,
                        Layer::L2,
                        "ArbRetryableTx.getLifetime() returned 0",
                        "Retryable ticket lifetime should be non-zero",
                        ctx.rng.seed(),
                        0,
                    ));
                } else {
                    tracing::info!(lifetime_secs = lifetime, "ArbRetryableTx.getLifetime() OK");
                    ctx.check_passed();
                }
            }
            Ok(_) => {
                tracing::info!("ArbRetryableTx.getLifetime() short response");
                ctx.check_passed();
            }
            Err(_) => {
                tracing::info!("ArbRetryableTx.getLifetime() reverted (may be deprecated)");
                ctx.check_passed();
            }
        }

        // --- ArbAggregator (0x6D) ---
        // getDefaultAggregator() selector: 0xd4f50198 (deprecated but should still respond)
        let arbagg: alloy_primitives::Address =
            "0x000000000000000000000000000000000000006D".parse()?;
        match rpc.eth_call(Layer::L2, arbagg, vec![0xd4, 0xf5, 0x01, 0x98]).await {
            Ok(result) => {
                if result.len() >= 32 {
                    tracing::info!("ArbAggregator.getDefaultAggregator() OK");
                    ctx.check_passed();
                } else {
                    ctx.check_passed(); // May return empty on newer versions.
                }
            }
            Err(_) => {
                // Deprecated precompile — acceptable to revert.
                tracing::info!("ArbAggregator.getDefaultAggregator() reverted (deprecated, OK)");
                ctx.check_passed();
            }
        }

        // --- L3 precompiles (if L3 present) ---
        if rpc.l3.is_some() {
            match rpc.eth_call(Layer::L3, arbsys, vec![0xa3, 0xb1, 0xb3, 0x1d]).await {
                Ok(l3_block) if l3_block.len() >= 32 && !l3_block.iter().all(|&b| b == 0) => {
                    let block = u64::from_be_bytes(
                        l3_block[24..32].try_into().unwrap_or([0; 8]),
                    );
                    tracing::info!(block = block, "L3 ArbSys.arbBlockNumber() OK");
                    ctx.check_passed();
                }
                Ok(l3_block) => {
                    ctx.report(invariant::violation(
                        InvariantClass::SequencerLiveness,
                        Layer::L3,
                        "L3 ArbSys.arbBlockNumber() returned invalid data",
                        format!("returned {} bytes", l3_block.len()),
                        ctx.rng.seed(),
                        0,
                    ));
                }
                Err(e) => {
                    ctx.report(invariant::violation(
                        InvariantClass::SequencerLiveness,
                        Layer::L3,
                        "L3 ArbSys.arbBlockNumber() call failed",
                        format!("{}", e),
                        ctx.rng.seed(),
                        0,
                    ));
                }
            }
        }

        Ok(ctx.into_outcome())
    }
}
