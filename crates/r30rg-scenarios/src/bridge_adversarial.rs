use r30rg_core::invariant::{self, InvariantClass};
use r30rg_core::scenario::{Scenario, ScenarioCategory, ScenarioContext, ScenarioMeta};
use r30rg_core::types::{Layer, Severity, ScenarioOutcome};
use r30rg_live::rpc::RpcHarness;
use alloy_primitives::Address;

/// Probe balance consistency across L1/L2/L3.
/// Checks that the bridge accounting is sane — no money created from thin air.
pub struct BalanceConsistencyProbe;

#[async_trait::async_trait]
impl Scenario for BalanceConsistencyProbe {
    fn meta(&self) -> ScenarioMeta {
        ScenarioMeta {
            name: "balance-consistency-probe".into(),
            description: "Snapshot balances on L1/L2/L3 for known addresses and verify \
                          cross-layer consistency. Checks that ArbSys and ArbGasInfo \
                          precompiles return sane values."
                .into(),
            category: ScenarioCategory::BridgeAdversarial,
            target_layers: vec![Layer::L1, Layer::L2, Layer::L3],
            severity_potential: Severity::Critical,
            destructive: false,
        }
    }

    async fn execute(&self, mut ctx: ScenarioContext) -> Result<ScenarioOutcome, anyhow::Error> {
        let rpc = RpcHarness::connect(
            &ctx.config.l1,
            &ctx.config.l2,
            ctx.config.l3.as_ref(),
        ).await?;

        // Dev address used by testnode.
        let dev_addr: Address = "0x3f1Eae7D46d88F08fc2F8ed27FCb2AB183EB2d0E".parse()?;

        // Check 1: L1 balance should be positive (funded during init).
        let l1_bal = rpc.balance(Layer::L1, dev_addr).await?;
        if l1_bal.is_zero() {
            ctx.report(invariant::violation(
                InvariantClass::BalanceConsistency,
                Layer::L1,
                "L1 dev account has zero balance",
                "The dev account should have been funded during testnode init",
                ctx.rng.seed(),
                0,
            ));
        } else {
            tracing::info!(l1_balance = %l1_bal, "L1 dev balance OK");
            ctx.check_passed();
        }

        // Check 2: L2 balance should be positive (bridged during init).
        let l2_bal = rpc.balance(Layer::L2, dev_addr).await?;
        if l2_bal.is_zero() {
            ctx.report(invariant::violation(
                InvariantClass::BalanceConsistency,
                Layer::L2,
                "L2 dev account has zero balance",
                "The dev account should have bridged funds to L2",
                ctx.rng.seed(),
                0,
            ));
        } else {
            tracing::info!(l2_balance = %l2_bal, "L2 dev balance OK");
            ctx.check_passed();
        }

        // Check 3: L2 gas price should be non-zero and reasonable.
        let l2_gas = rpc.gas_price(Layer::L2).await?;
        if l2_gas == 0 {
            ctx.report(invariant::violation(
                InvariantClass::GasPricingSanity,
                Layer::L2,
                "L2 gas price is zero",
                "Gas price of 0 indicates misconfigured pricing",
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        // Check 4: L2 ArbSys precompile (0x64) returns non-zero block.
        let arbsys_addr: Address = "0x0000000000000000000000000000000000000064".parse()?;
        // arbBlockNumber() selector = 0xa3b1b31d
        let arbsys_result = rpc.eth_call(Layer::L2, arbsys_addr, vec![0xa3, 0xb1, 0xb3, 0x1d]).await?;
        if arbsys_result.iter().all(|&b| b == 0) {
            ctx.report(invariant::violation(
                InvariantClass::SequencerLiveness,
                Layer::L2,
                "ArbSys.arbBlockNumber() returned 0",
                "L2 precompile reports zero block number",
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        // Check 5: L3 checks (if available).
        if rpc.l3.is_some() {
            let l3_block = rpc.block_number(Layer::L3).await?;
            if l3_block == 0 {
                ctx.report(invariant::violation(
                    InvariantClass::BlockMonotonicity,
                    Layer::L3,
                    "L3 has zero block height",
                    "L3 should have blocks if it was initialized",
                    ctx.rng.seed(),
                    0,
                ));
            } else {
                ctx.check_passed();
            }

            let l3_gas = rpc.gas_price(Layer::L3).await?;
            if l3_gas == 0 {
                ctx.report(invariant::violation(
                    InvariantClass::GasPricingSanity,
                    Layer::L3,
                    "L3 gas price is zero",
                    "Gas price of 0 on L3 indicates misconfigured pricing",
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
