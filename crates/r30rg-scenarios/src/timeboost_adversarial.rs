use r30rg_core::invariant::{self, InvariantClass};
use r30rg_core::scenario::{Scenario, ScenarioCategory, ScenarioContext, ScenarioMeta};
use r30rg_core::types::{Layer, Severity, ScenarioOutcome};
use r30rg_live::rpc::RpcHarness;
use alloy_primitives::{Address, U256};

/// ExpressLaneAuction contract function selectors (from Solidity ABI).
mod selector {
    // currentRound() → uint64
    pub const CURRENT_ROUND: [u8; 4] = [0x8a, 0x19, 0xc8, 0xbc];
    // currentExpressLaneController() → address
    pub const CURRENT_CONTROLLER: [u8; 4] = [0x4f, 0x2a, 0x9b, 0xdb];
    // initialRoundTimestamp() → uint256
    pub const INITIAL_ROUND_TS: [u8; 4] = [0xc0, 0x38, 0x99, 0x79];
    // roundDurationSeconds() → uint64
    pub const ROUND_DURATION: [u8; 4] = [0xcc, 0x96, 0x3d, 0x15];
    // getCurrentReservePrice() → uint256
    pub const RESERVE_PRICE: [u8; 4] = [0xb9, 0x41, 0xce, 0x6e];
    // getminimalReservePrice() → uint256
    pub const MIN_RESERVE: [u8; 4] = [0x03, 0xba, 0x66, 0x62];
    // bidReceiver() → address
    pub const BID_RECEIVER: [u8; 4] = [0x4b, 0xc3, 0x7e, 0xa6];
    // expressLaneControllerByRound(uint256) → address
    pub const CONTROLLER_BY_ROUND: [u8; 4] = [0x82, 0x96, 0xdf, 0x03];
}

/// Timeboost adversarial scenario: probes the express lane auction contract
/// for configuration sanity, monopolization risk, and timing attack windows.
///
/// **Portable design:**
/// - Requires `--auction-addr` pointing to the ExpressLaneAuction contract on
///   the **L2 chain** (or whichever chain hosts the auction).
/// - All checks are read-only `eth_call` — no transactions sent.
/// - If no auction address is provided, the scenario reports it and exits cleanly.
pub struct TimeboostAuctionProbe;

#[async_trait::async_trait]
impl Scenario for TimeboostAuctionProbe {
    fn meta(&self) -> ScenarioMeta {
        ScenarioMeta {
            name: "timeboost-auction-probe".into(),
            description: "Probe the ExpressLaneAuction contract for configuration sanity, \
                          express lane monopolization risk, round timing analysis, and \
                          reserve price invariants. Pass --auction-addr to enable."
                .into(),
            category: ScenarioCategory::BridgeAdversarial,
            target_layers: vec![Layer::L1, Layer::L2],
            severity_potential: Severity::High,
            destructive: false,
        }
    }

    async fn execute(&self, mut ctx: ScenarioContext) -> Result<ScenarioOutcome, anyhow::Error> {
        let auction_addr = match &ctx.config.auction_addr {
            Some(addr) => match addr.parse::<Address>() {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!(addr = %addr, error = %e, "invalid --auction-addr");
                    anyhow::bail!("invalid --auction-addr '{}': {}", addr, e);
                }
            },
            None => {
                tracing::info!(
                    "Skipping timeboost probe (no --auction-addr). \
                     Pass --auction-addr <ExpressLaneAuction address> to enable."
                );
                return Ok(ctx.into_outcome());
            }
        };

        // Auction contract lives on L1 in the standard deployment.
        // However, some deployments put it on L2. We try L1 first, fall back to L2.
        let rpc = RpcHarness::connect(
            &ctx.config.l1,
            &ctx.config.l2,
            ctx.config.l3.as_ref(),
        ).await?;

        let auction_layer = self.detect_auction_layer(&rpc, auction_addr).await;
        tracing::info!(layer = %auction_layer, contract = %auction_addr, "auction contract detected");

        // --- Check 1: Round configuration ---
        self.check_round_config(&rpc, &mut ctx, auction_addr, auction_layer).await?;

        // --- Check 2: Current express lane controller ---
        self.check_express_lane_controller(&rpc, &mut ctx, auction_addr, auction_layer).await?;

        // --- Check 3: Reserve price invariants ---
        self.check_reserve_prices(&rpc, &mut ctx, auction_addr, auction_layer).await?;

        // --- Check 4: Monopolization risk (same controller across rounds) ---
        self.check_monopolization(&rpc, &mut ctx, auction_addr, auction_layer).await?;

        // --- Check 5: Bid receiver configuration ---
        self.check_bid_receiver(&rpc, &mut ctx, auction_addr, auction_layer).await?;

        Ok(ctx.into_outcome())
    }
}

impl TimeboostAuctionProbe {
    /// Try to determine which layer hosts the auction contract.
    async fn detect_auction_layer(&self, rpc: &RpcHarness, addr: Address) -> Layer {
        // Try currentRound() on L1 first.
        if rpc.eth_call(Layer::L1, addr, selector::CURRENT_ROUND.to_vec()).await.is_ok() {
            return Layer::L1;
        }
        Layer::L2
    }

    /// Verify round timing configuration is sane.
    async fn check_round_config(
        &self,
        rpc: &RpcHarness,
        ctx: &mut ScenarioContext,
        addr: Address,
        layer: Layer,
    ) -> anyhow::Result<()> {
        // currentRound()
        let round_data = rpc.eth_call(layer, addr, selector::CURRENT_ROUND.to_vec()).await?;
        let current_round = decode_uint64(&round_data);
        tracing::info!(round = current_round, "current auction round");

        if current_round == 0 {
            ctx.report(invariant::violation(
                InvariantClass::SequencerLiveness,
                layer,
                "Timeboost auction reports round 0",
                "currentRound() returned 0 — auction may not be initialized",
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        // roundDurationSeconds()
        let duration_data = rpc.eth_call(layer, addr, selector::ROUND_DURATION.to_vec()).await?;
        let duration_secs = decode_uint64(&duration_data);
        tracing::info!(seconds = duration_secs, "round duration");

        if duration_secs == 0 {
            ctx.report(invariant::violation(
                InvariantClass::GasPricingSanity,
                layer,
                "Round duration is 0 seconds",
                "roundDurationSeconds() returned 0 — broken auction config",
                ctx.rng.seed(),
                0,
            ));
        } else if duration_secs < 15 {
            ctx.report(invariant::violation(
                InvariantClass::GasPricingSanity,
                layer,
                "Round duration suspiciously short",
                format!("roundDurationSeconds()={} — too short for fair bidding (spec says 60s)", duration_secs),
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        // initialRoundTimestamp()
        let ts_data = rpc.eth_call(layer, addr, selector::INITIAL_ROUND_TS.to_vec()).await?;
        let initial_ts = decode_u256(&ts_data);
        tracing::info!(timestamp = %initial_ts, "initial round timestamp");

        if initial_ts.is_zero() {
            ctx.report(invariant::violation(
                InvariantClass::SequencerLiveness,
                layer,
                "Initial round timestamp is 0",
                "initialRoundTimestamp() returned 0 — auction not configured",
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        Ok(())
    }

    /// Check who currently controls the express lane.
    async fn check_express_lane_controller(
        &self,
        rpc: &RpcHarness,
        ctx: &mut ScenarioContext,
        addr: Address,
        layer: Layer,
    ) -> anyhow::Result<()> {
        let ctrl_data = rpc.eth_call(layer, addr, selector::CURRENT_CONTROLLER.to_vec()).await?;
        let controller = decode_address(&ctrl_data);
        tracing::info!(controller = %controller, "current express lane controller");

        // Zero address means no one controls the express lane (no bids won).
        if controller == Address::ZERO {
            tracing::info!("No current express lane controller — no winning bid this round");
        }
        ctx.check_passed();

        Ok(())
    }

    /// Verify reserve price invariants: current >= minimal, both non-negative.
    async fn check_reserve_prices(
        &self,
        rpc: &RpcHarness,
        ctx: &mut ScenarioContext,
        addr: Address,
        layer: Layer,
    ) -> anyhow::Result<()> {
        let reserve_data = rpc.eth_call(layer, addr, selector::RESERVE_PRICE.to_vec()).await?;
        let reserve = decode_u256(&reserve_data);

        let min_data = rpc.eth_call(layer, addr, selector::MIN_RESERVE.to_vec()).await?;
        let min_reserve = decode_u256(&min_data);

        tracing::info!(current = %reserve, minimum = %min_reserve, "reserve prices");

        // Invariant: current reserve >= minimal reserve.
        if reserve < min_reserve {
            ctx.report(invariant::violation(
                InvariantClass::BalanceConsistency,
                layer,
                "Current reserve price below minimum",
                format!(
                    "getCurrentReservePrice()={} < getminimalReservePrice()={} — price invariant broken",
                    reserve, min_reserve
                ),
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        Ok(())
    }

    /// Check if the same address controls the express lane across multiple rounds
    /// (monopolization risk).
    async fn check_monopolization(
        &self,
        rpc: &RpcHarness,
        ctx: &mut ScenarioContext,
        addr: Address,
        layer: Layer,
    ) -> anyhow::Result<()> {
        // Get current round.
        let round_data = rpc.eth_call(layer, addr, selector::CURRENT_ROUND.to_vec()).await?;
        let current_round = decode_uint64(&round_data);

        if current_round < 3 {
            tracing::info!(round = current_round, "too few rounds for monopolization check");
            return Ok(());
        }

        // Check controllers for the last N rounds.
        let lookback = 5.min(current_round as usize);
        let mut controllers = Vec::new();

        for i in 0..lookback {
            let round = current_round - i as u64;
            let mut calldata = selector::CONTROLLER_BY_ROUND.to_vec();
            // ABI-encode uint256(round).
            let mut round_bytes = [0u8; 32];
            round_bytes[24..].copy_from_slice(&round.to_be_bytes());
            calldata.extend_from_slice(&round_bytes);

            match rpc.eth_call(layer, addr, calldata).await {
                Ok(data) => {
                    let ctrl = decode_address(&data);
                    controllers.push((round, ctrl));
                }
                Err(_) => break,
            }
        }

        if controllers.len() >= 3 {
            let non_zero: Vec<_> = controllers.iter()
                .filter(|(_, c)| *c != Address::ZERO)
                .collect();

            if non_zero.len() >= 3 {
                let first_ctrl = non_zero[0].1;
                let all_same = non_zero.iter().all(|(_, c)| *c == first_ctrl);

                if all_same {
                    ctx.report(invariant::violation(
                        InvariantClass::SequencerLiveness,
                        layer,
                        "Express lane monopolization detected",
                        format!(
                            "Same controller {} won {} consecutive rounds — \
                             potential auction manipulation or lack of competition",
                            first_ctrl, non_zero.len()
                        ),
                        ctx.rng.seed(),
                        0,
                    ));
                } else {
                    tracing::info!(
                        rounds = non_zero.len(),
                        "express lane controller rotated across rounds — healthy"
                    );
                    ctx.check_passed();
                }
            }
        }

        Ok(())
    }

    /// Verify the bid receiver is configured (not zero address).
    async fn check_bid_receiver(
        &self,
        rpc: &RpcHarness,
        ctx: &mut ScenarioContext,
        addr: Address,
        layer: Layer,
    ) -> anyhow::Result<()> {
        let data = rpc.eth_call(layer, addr, selector::BID_RECEIVER.to_vec()).await?;
        let receiver = decode_address(&data);
        tracing::info!(receiver = %receiver, "bid receiver");

        if receiver == Address::ZERO {
            ctx.report(invariant::violation(
                InvariantClass::BalanceConsistency,
                layer,
                "Bid receiver is zero address",
                "bidReceiver() returned 0x0 — auction revenue goes nowhere",
                ctx.rng.seed(),
                0,
            ));
        } else {
            ctx.check_passed();
        }

        Ok(())
    }
}

/// Decode a uint64 from ABI-encoded bytes (right-aligned in 32 bytes).
fn decode_uint64(data: &[u8]) -> u64 {
    if data.len() < 32 {
        return 0;
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[24..32]);
    u64::from_be_bytes(bytes)
}

/// Decode a U256 from ABI-encoded bytes.
fn decode_u256(data: &[u8]) -> U256 {
    if data.len() < 32 {
        return U256::ZERO;
    }
    U256::from_be_slice(&data[..32])
}

/// Decode an address from ABI-encoded bytes (right-aligned in 32 bytes).
fn decode_address(data: &[u8]) -> Address {
    if data.len() < 32 {
        return Address::ZERO;
    }
    Address::from_slice(&data[12..32])
}
