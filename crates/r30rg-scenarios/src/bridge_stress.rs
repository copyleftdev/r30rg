use r30rg_core::invariant::{self, InvariantClass};
use r30rg_core::scenario::{Scenario, ScenarioCategory, ScenarioContext, ScenarioMeta};
use r30rg_core::types::{Layer, Severity, ScenarioOutcome};
use r30rg_live::rpc::RpcHarness;
use alloy_primitives::{Address, U256};
use tokio::time::{sleep, Duration};

/// ArbSys precompile — same address on ALL Arbitrum chains (universal).
const ARBSYS_ADDR: &str = "0x0000000000000000000000000000000000000064";

/// Bridge stress test: exercises the cross-layer bridge on any Arbitrum stack.
///
/// **Portable design:**
/// - Withdrawal via ArbSys (0x64) works on every Arbitrum chain — always tested.
/// - L1→L2 deposit requires the Inbox address from the rollup deployment.
///   Pass `--inbox-addr` and `--bridge-addr` to enable deposit testing.
///   If not provided, the deposit phase is gracefully skipped.
/// - Uses `eth_accounts` + `eth_sendTransaction` (requires dev-mode or
///   unlocked accounts on both L1 and L2).
pub struct BridgeDepositWithdrawStress;

#[async_trait::async_trait]
impl Scenario for BridgeDepositWithdrawStress {
    fn meta(&self) -> ScenarioMeta {
        ScenarioMeta {
            name: "bridge-deposit-withdraw-stress".into(),
            description: "Exercise L1↔L2 bridge: deposit ETH via Inbox (if configured), \
                          withdraw via ArbSys, verify balance conservation. \
                          Pass --inbox-addr and --bridge-addr for full deposit testing."
                .into(),
            category: ScenarioCategory::BridgeAdversarial,
            target_layers: vec![Layer::L1, Layer::L2],
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

        let arbsys_addr: Address = ARBSYS_ADDR.parse()?;

        // --- Phase A: L1→L2 Deposit (requires --inbox-addr / --bridge-addr) ---
        let inbox_addr = ctx.config.inbox_addr.as_ref().and_then(|s| s.parse::<Address>().ok());
        let bridge_addr = ctx.config.bridge_addr.as_ref().and_then(|s| s.parse::<Address>().ok());

        if let (Some(inbox), Some(bridge)) = (inbox_addr, bridge_addr) {
            self.test_deposit(&rpc, &mut ctx, inbox, bridge).await?;
        } else {
            tracing::info!(
                "Skipping L1→L2 deposit test (no --inbox-addr / --bridge-addr). \
                 Pass these flags to enable. They come from your rollup's deployment.json."
            );
        }

        // --- Phase B: L2→L1 Withdrawal via ArbSys (works on any Arbitrum chain) ---
        self.test_withdrawal(&rpc, &mut ctx, arbsys_addr).await?;

        // --- Phase C: ArbSys state consistency (works on any Arbitrum chain) ---
        self.test_arbsys_state(&rpc, &mut ctx, arbsys_addr).await?;

        Ok(ctx.into_outcome())
    }
}

impl BridgeDepositWithdrawStress {
    /// Test L1→L2 deposit via Inbox contract. Only called when addresses are configured.
    async fn test_deposit(
        &self,
        rpc: &RpcHarness,
        ctx: &mut ScenarioContext,
        inbox: Address,
        bridge: Address,
    ) -> anyhow::Result<()> {
        // Get L1 unlocked account (dev-mode geth).
        let l1_accounts = rpc.accounts(Layer::L1).await?;
        if l1_accounts.is_empty() {
            tracing::warn!("No unlocked L1 accounts — skipping deposit test. \
                            L1 must be dev-mode geth with unlocked accounts.");
            return Ok(());
        }
        let l1_sender = l1_accounts[0];
        tracing::info!(sender = %l1_sender, "L1 dev account for deposit");

        // Snapshot pre-deposit state.
        let bridge_bal_before = rpc.balance(Layer::L1, bridge).await?;
        let l2_bal_before = rpc.balance(Layer::L2, l1_sender).await?;
        tracing::info!(bridge = %bridge_bal_before, l2 = %l2_bal_before, "pre-deposit");

        // depositEth() selector = 0x439370b1
        let deposit_value = U256::from(100_000_000_000_000_000u64); // 0.1 ETH

        tracing::info!(value = %deposit_value, inbox = %inbox, "sending L1→L2 deposit");
        let tx_hash = rpc.send_transaction(
            Layer::L1,
            l1_sender,
            inbox,
            deposit_value,
            vec![0x43, 0x93, 0x70, 0xb1],
        ).await?;
        tracing::info!(tx = %tx_hash, "L1 deposit tx confirmed");
        ctx.check_passed();

        // Verify bridge balance increased.
        let bridge_bal_after = rpc.balance(Layer::L1, bridge).await?;
        if bridge_bal_after < bridge_bal_before + deposit_value {
            ctx.report(invariant::violation(
                InvariantClass::BalanceConsistency,
                Layer::L1,
                "Bridge balance did not increase after deposit",
                format!("before={} after={} deposit={}", bridge_bal_before, bridge_bal_after, deposit_value),
                ctx.rng.seed(),
                0,
            ));
        } else {
            tracing::info!(delta = %(bridge_bal_after - bridge_bal_before), "bridge balance increased");
            ctx.check_passed();
        }

        // Wait for deposit to arrive on L2 (up to 60s).
        let timeout = Duration::from_secs(60);
        let poll = Duration::from_secs(3);
        let start = tokio::time::Instant::now();
        let mut arrived = false;

        while start.elapsed() < timeout {
            let l2_now = rpc.balance(Layer::L2, l1_sender).await?;
            if l2_now > l2_bal_before {
                tracing::info!(
                    increase = %(l2_now - l2_bal_before),
                    elapsed_ms = start.elapsed().as_millis(),
                    "L2 deposit arrived"
                );
                arrived = true;
                ctx.check_passed();
                break;
            }
            sleep(poll).await;
        }

        if !arrived {
            ctx.report(invariant::violation(
                InvariantClass::RetryableResolution,
                Layer::L2,
                "L1→L2 deposit did not arrive within 60s",
                format!("Deposited {} via Inbox but L2 balance unchanged", deposit_value),
                ctx.rng.seed(),
                0,
            ));
        }

        // Final bridge conservation: balance should be >= initial.
        let bridge_final = rpc.balance(Layer::L1, bridge).await?;
        if bridge_final < bridge_bal_before {
            ctx.report(invariant::violation(
                InvariantClass::BalanceConsistency,
                Layer::L1,
                "Bridge balance decreased without completed withdrawal",
                format!("initial={} final={}", bridge_bal_before, bridge_final),
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        Ok(())
    }

    /// Test L2→L1 withdrawal via ArbSys. Works on any Arbitrum chain.
    async fn test_withdrawal(
        &self,
        rpc: &RpcHarness,
        ctx: &mut ScenarioContext,
        arbsys: Address,
    ) -> anyhow::Result<()> {
        let l2_accounts = rpc.accounts(Layer::L2).await?;
        if l2_accounts.is_empty() {
            tracing::warn!("No unlocked L2 accounts — skipping withdrawal test. \
                            L2 must have unlocked accounts for tx signing.");
            return Ok(());
        }
        let l2_sender = l2_accounts[0];

        let l2_balance = rpc.balance(Layer::L2, l2_sender).await?;
        if l2_balance.is_zero() {
            tracing::warn!(account = %l2_sender, "L2 sender has zero balance — skipping withdrawal");
            return Ok(());
        }

        // withdrawEth(address destination) selector = 0x25e16063
        let withdraw_value = U256::from(10_000_000_000_000_000u64); // 0.01 ETH
        if l2_balance < withdraw_value {
            tracing::warn!(balance = %l2_balance, "L2 balance too low for 0.01 ETH withdrawal — skipping");
            return Ok(());
        }

        let mut calldata = vec![0x25, 0xe1, 0x60, 0x63];
        let mut padded_addr = vec![0u8; 12];
        padded_addr.extend_from_slice(l2_sender.as_slice());
        calldata.extend_from_slice(&padded_addr);

        let pre_balance = rpc.balance(Layer::L2, l2_sender).await?;
        tracing::info!(value = %withdraw_value, from = %l2_sender, "L2→L1 withdrawal via ArbSys");

        match rpc.send_transaction(Layer::L2, l2_sender, arbsys, withdraw_value, calldata).await {
            Ok(tx_hash) => {
                tracing::info!(tx = %tx_hash, "withdrawal tx confirmed");

                let post_balance = rpc.balance(Layer::L2, l2_sender).await?;
                if post_balance >= pre_balance {
                    ctx.report(invariant::violation(
                        InvariantClass::BalanceConsistency,
                        Layer::L2,
                        "L2 balance did not decrease after withdrawal",
                        format!("before={} after={}", pre_balance, post_balance),
                        ctx.rng.seed(),
                        0,
                    ));
                } else {
                    let spent = pre_balance - post_balance;
                    tracing::info!(spent = %spent, "L2 withdrawal deducted");
                    ctx.check_passed();
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "withdrawal tx failed — L2 may require funded unlocked account");
            }
        }

        Ok(())
    }

    /// Verify ArbSys precompile state consistency. Works on any Arbitrum chain.
    async fn test_arbsys_state(
        &self,
        rpc: &RpcHarness,
        ctx: &mut ScenarioContext,
        arbsys: Address,
    ) -> anyhow::Result<()> {
        // arbBlockNumber() = 0xa3b1b31d
        let block_result = rpc.eth_call(Layer::L2, arbsys, vec![0xa3, 0xb1, 0xb3, 0x1d]).await?;
        if block_result.iter().all(|&b| b == 0) {
            ctx.report(invariant::violation(
                InvariantClass::SequencerLiveness,
                Layer::L2,
                "ArbSys.arbBlockNumber() returned 0",
                "L2 precompile reports zero block number after operations",
                ctx.rng.seed(),
                0,
            ));
        } else {
            tracing::info!("ArbSys.arbBlockNumber() OK");
            ctx.check_passed();
        }

        // arbOSVersion() = 0x051038f2
        match rpc.eth_call(Layer::L2, arbsys, vec![0x05, 0x10, 0x38, 0xf2]).await {
            Ok(data) if !data.is_empty() => {
                tracing::info!(len = data.len(), "ArbSys.arbOSVersion() responded");
                ctx.check_passed();
            }
            Ok(_) => {
                tracing::warn!("ArbSys.arbOSVersion() returned empty — may be deprecated");
            }
            Err(e) => {
                tracing::warn!(error = %e, "ArbSys.arbOSVersion() reverted — may be deprecated");
            }
        }

        Ok(())
    }
}
