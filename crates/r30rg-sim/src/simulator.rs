use r30rg_core::prng::DeterministicRng;
use r30rg_core::time::SimulatedClock;
use crate::network::{SimulatedNetwork, NetworkConfig, NodeId};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};

/// Models a simplified rollup component in the simulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeRole {
    L1,
    Sequencer,
    BatchPoster,
    Validator,
    L3Sequencer,
}

/// Simulated cross-layer message.
#[derive(Debug, Clone)]
pub struct BridgeMessage {
    pub from_layer: u8,
    pub to_layer: u8,
    pub value: u64,
    pub created_tick: u64,
    pub deliver_tick: u64,
    pub is_retryable: bool,
    pub redeemed: bool,
}

/// Simulated node state.
#[derive(Debug)]
pub struct SimNode {
    pub id: NodeId,
    pub role: NodeRole,
    pub alive: bool,
    pub block_height: u64,
    pub last_batch_posted: u64,
    pub last_assertion: u64,
    pub pending_txs: u64,
    pub gas_price: u64,
    pub balance: u64,
    pub total_deposited: u64,
    pub total_withdrawn: u64,
}

impl SimNode {
    pub fn new(id: NodeId, role: NodeRole) -> Self {
        let initial_balance = match role {
            NodeRole::L1 => 1_000_000_000,
            NodeRole::Sequencer => 100_000_000,
            NodeRole::L3Sequencer => 10_000_000,
            _ => 50_000_000,
        };
        Self {
            id,
            role,
            alive: true,
            block_height: 0,
            last_batch_posted: 0,
            last_assertion: 0,
            pending_txs: 0,
            gas_price: 1_000,
            balance: initial_balance,
            total_deposited: 0,
            total_withdrawn: 0,
        }
    }

    pub fn tick(&mut self, rng: &mut DeterministicRng) {
        if !self.alive {
            return;
        }
        match self.role {
            NodeRole::L1 => {
                self.block_height += 1;
                // L1 gas price fluctuates.
                let delta = rng.range(0, 200) as i64 - 100;
                self.gas_price = (self.gas_price as i64 + delta).max(100) as u64;
            }
            NodeRole::Sequencer => {
                self.block_height += 1;
                let new_txs = rng.range(0, 20);
                self.pending_txs += new_txs;
                // L2 gas price follows L1 loosely.
                let jitter = rng.range(0, 50) as i64 - 25;
                self.gas_price = (self.gas_price as i64 + jitter).max(10) as u64;
            }
            NodeRole::BatchPoster => {
                if rng.chance(0.1) && self.pending_txs > 0 {
                    self.last_batch_posted = self.block_height;
                    let cost = self.pending_txs * 10;
                    self.balance = self.balance.saturating_sub(cost);
                    self.pending_txs = 0;
                }
                self.block_height += 1;
            }
            NodeRole::Validator => {
                if rng.chance(0.02) {
                    self.last_assertion = self.block_height;
                    let stake_cost = 100;
                    self.balance = self.balance.saturating_sub(stake_cost);
                }
                self.block_height += 1;
            }
            NodeRole::L3Sequencer => {
                self.block_height += 1;
                let new_txs = rng.range(0, 5);
                self.pending_txs += new_txs;
                let jitter = rng.range(0, 20) as i64 - 10;
                self.gas_price = (self.gas_price as i64 + jitter).max(10) as u64;
            }
        }
    }

    pub fn crash(&mut self) {
        self.alive = false;
    }

    pub fn restart(&mut self) {
        self.alive = true;
    }
}

/// Result of a simulation campaign (many seeds).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignResult {
    pub seeds_run: u64,
    pub seeds_passed: u64,
    pub seeds_failed: u64,
    pub total_ticks: u64,
    pub violations: Vec<SimViolation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimViolation {
    pub seed: u64,
    pub tick: u64,
    pub description: String,
}

/// The main deterministic simulator.
/// Same seed → same fault injection → same invariant checks → reproducible.
/// Models L1, L2 (sequencer + poster + validator), and L3 (Orbit sequencer)
/// with cross-layer bridge messaging and retryable ticket simulation.
pub struct Simulator {
    seed: u64,
    rng: DeterministicRng,
    clock: SimulatedClock,
    network: SimulatedNetwork,
    nodes: Vec<SimNode>,
    bridge_queue: VecDeque<BridgeMessage>,
    retryable_tickets: Vec<BridgeMessage>,
    violations: Vec<SimViolation>,
    seen_invariants: HashSet<String>,
    ticks_run: u64,
    total_l1_to_l2_value: u64,
    total_l2_to_l1_value: u64,
    total_l2_to_l3_value: u64,
}

impl Simulator {
    pub fn new(seed: u64) -> Self {
        let rng = DeterministicRng::new(seed);
        let network = SimulatedNetwork::new(NetworkConfig::default());

        let nodes = vec![
            SimNode::new(0, NodeRole::L1),
            SimNode::new(1, NodeRole::Sequencer),
            SimNode::new(2, NodeRole::BatchPoster),
            SimNode::new(3, NodeRole::Validator),
            SimNode::new(4, NodeRole::L3Sequencer),
        ];

        Self {
            seed,
            rng,
            clock: SimulatedClock::new(),
            network,
            nodes,
            bridge_queue: VecDeque::new(),
            retryable_tickets: Vec::new(),
            violations: Vec::new(),
            seen_invariants: HashSet::new(),
            ticks_run: 0,
            total_l1_to_l2_value: 0,
            total_l2_to_l1_value: 0,
            total_l2_to_l3_value: 0,
        }
    }

    /// Run the simulation for `max_ticks`.
    pub fn run(&mut self, max_ticks: u64) -> SimResult {
        for _ in 0..max_ticks {
            self.clock.advance(1);
            self.ticks_run += 1;
            let tick = self.clock.now();

            // Tick all alive nodes.
            for node in &mut self.nodes {
                let mut fork = self.rng.fork();
                node.tick(&mut fork);
            }

            // Propagate pending txs: sequencer → batch poster.
            let seq_pending = self.nodes.iter()
                .find(|n| n.role == NodeRole::Sequencer && n.alive)
                .map(|n| n.pending_txs)
                .unwrap_or(0);
            if let Some(poster) = self.nodes.iter_mut().find(|n| n.role == NodeRole::BatchPoster && n.alive) {
                poster.pending_txs = seq_pending;
            }

            // Generate cross-layer bridge messages.
            self.maybe_bridge_message(tick);

            // Deliver pending bridge messages.
            self.deliver_bridge_messages(tick);

            // Process retryable tickets.
            self.process_retryables(tick);

            // Possibly inject faults.
            self.maybe_inject_fault();

            // Check invariants after every tick.
            self.check_invariants();

            // Deliver any pending network messages.
            let _msgs = self.network.deliver(tick);
        }

        SimResult {
            seed: self.seed,
            ticks: self.ticks_run,
            violations: self.violations.clone(),
            network_stats: self.network.stats(),
        }
    }

    fn maybe_bridge_message(&mut self, tick: u64) {
        // ~1% chance per tick of an L1→L2 deposit.
        if self.rng.chance(0.01) {
            let value = self.rng.range(100, 10000);
            let latency = self.rng.range(5, 30);
            let is_retryable = self.rng.chance(0.3);
            self.bridge_queue.push_back(BridgeMessage {
                from_layer: 1,
                to_layer: 2,
                value,
                created_tick: tick,
                deliver_tick: tick + latency,
                is_retryable,
                redeemed: false,
            });
            self.total_l1_to_l2_value += value;
        }

        // ~0.5% chance of L2→L1 withdrawal.
        if self.rng.chance(0.005) {
            let value = self.rng.range(50, 5000);
            let latency = self.rng.range(100, 500); // Challenge period.
            self.bridge_queue.push_back(BridgeMessage {
                from_layer: 2,
                to_layer: 1,
                value,
                created_tick: tick,
                deliver_tick: tick + latency,
                is_retryable: false,
                redeemed: false,
            });
            self.total_l2_to_l1_value += value;
        }

        // ~0.5% chance of L2→L3 deposit.
        if self.rng.chance(0.005) {
            let value = self.rng.range(10, 1000);
            let latency = self.rng.range(3, 15);
            self.bridge_queue.push_back(BridgeMessage {
                from_layer: 2,
                to_layer: 3,
                value,
                created_tick: tick,
                deliver_tick: tick + latency,
                is_retryable: self.rng.chance(0.2),
                redeemed: false,
            });
            self.total_l2_to_l3_value += value;
        }
    }

    fn deliver_bridge_messages(&mut self, tick: u64) {
        let mut delivered = Vec::new();
        let mut remaining = VecDeque::new();

        while let Some(msg) = self.bridge_queue.pop_front() {
            if tick >= msg.deliver_tick {
                if msg.is_retryable {
                    // Goes into retryable queue — needs explicit redemption.
                    self.retryable_tickets.push(msg);
                } else {
                    delivered.push(msg);
                }
            } else {
                remaining.push_back(msg);
            }
        }
        self.bridge_queue = remaining;

        // Credit delivered messages.
        for msg in &delivered {
            self.credit_bridge_delivery(msg);
        }
    }

    fn credit_bridge_delivery(&mut self, msg: &BridgeMessage) {
        let target_role = match msg.to_layer {
            1 => NodeRole::L1,
            2 => NodeRole::Sequencer,
            3 => NodeRole::L3Sequencer,
            _ => return,
        };
        if let Some(node) = self.nodes.iter_mut().find(|n| n.role == target_role) {
            node.total_deposited += msg.value;
        }
    }

    fn process_retryables(&mut self, tick: u64) {
        // Auto-redeem retryable tickets after some delay.
        for ticket in &mut self.retryable_tickets {
            if !ticket.redeemed && tick >= ticket.deliver_tick + 5 {
                // Check if target layer sequencer is alive.
                let target_role = match ticket.to_layer {
                    2 => NodeRole::Sequencer,
                    3 => NodeRole::L3Sequencer,
                    _ => continue,
                };
                let target_alive = self.nodes.iter()
                    .find(|n| n.role == target_role)
                    .map(|n| n.alive)
                    .unwrap_or(false);
                if target_alive {
                    ticket.redeemed = true;
                    if let Some(node) = self.nodes.iter_mut().find(|n| n.role == target_role) {
                        node.total_deposited += ticket.value;
                    }
                }
            }
        }
    }

    fn maybe_inject_fault(&mut self) {
        // ~0.1% chance per tick of crashing a random non-L1 node.
        if self.rng.chance(0.001) {
            let non_l1: Vec<usize> = self
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| n.role != NodeRole::L1 && n.alive)
                .map(|(i, _)| i)
                .collect();
            if !non_l1.is_empty() {
                let idx = *self.rng.pick(&non_l1);
                self.nodes[idx].crash();

                // Schedule a restart 10–100 ticks later.
                let restart_delay = self.rng.range(10, 100);
                self.clock
                    .schedule(self.clock.now() + restart_delay, idx as u64);
            }
        }

        // ~0.05% chance of a network partition.
        if self.rng.chance(0.0005) {
            let a = self.rng.range(0, self.nodes.len() as u64 - 1) as u32;
            let b = self.rng.range(0, self.nodes.len() as u64 - 1) as u32;
            if a != b {
                self.network.partition(&[a], &[b]);
                let heal_delay = self.rng.range(20, 200);
                self.clock
                    .schedule(self.clock.now() + heal_delay, 1000 + a as u64 * 100 + b as u64);
            }
        }

        // Process scheduled events (restarts, partition heals).
        while let Some(ev) = self.clock.next_due_event() {
            if ev.id < 1000 {
                let idx = ev.id as usize;
                if idx < self.nodes.len() {
                    self.nodes[idx].restart();
                }
            } else {
                self.network.heal_all();
            }
        }
    }

    /// Record a violation only if this invariant class hasn't already fired for this seed.
    fn record_violation(&mut self, key: &str, tick: u64, description: String) {
        if self.seen_invariants.insert(key.to_string()) {
            self.violations.push(SimViolation {
                seed: self.seed,
                tick,
                description,
            });
        }
    }

    fn check_invariants(&mut self) {
        let tick = self.clock.now();

        // Invariant 1: L1 must always produce blocks.
        if let Some(l1) = self.nodes.iter().find(|n| n.role == NodeRole::L1) {
            if l1.block_height == 0 && tick > 10 {
                self.record_violation(
                    "l1_stalled",
                    tick,
                    "L1 has not produced any blocks".into(),
                );
            }
        }

        // Invariant 2: Sequencer should not stall permanently.
        let l1_height = self.nodes.iter()
            .find(|n| n.role == NodeRole::L1)
            .map(|n| n.block_height)
            .unwrap_or(0);
        let seq_height = self.nodes.iter()
            .find(|n| n.role == NodeRole::Sequencer)
            .map(|n| n.block_height)
            .unwrap_or(0);
        if l1_height > 100 && seq_height == 0 {
            self.record_violation(
                "seq_stalled",
                tick,
                format!("Sequencer produced 0 blocks while L1 is at {}", l1_height),
            );
        }

        // Invariant 3: Batch poster should post within 200 ticks.
        if tick > 200 {
            if let Some(poster) = self.nodes.iter().find(|n| n.role == NodeRole::BatchPoster) {
                if poster.alive && poster.last_batch_posted == 0 {
                    self.record_violation(
                        "poster_never_posted",
                        tick,
                        "Batch poster has never posted a batch".into(),
                    );
                }
            }
        }

        // Invariant 4: Validator should assert within 500 ticks.
        if tick > 500 {
            if let Some(val) = self.nodes.iter().find(|n| n.role == NodeRole::Validator) {
                if val.alive && val.last_assertion == 0 {
                    self.record_violation(
                        "validator_never_asserted",
                        tick,
                        "Validator has never posted an assertion".into(),
                    );
                }
            }
        }

        // Invariant 5: L3 sequencer should produce blocks if alive.
        if tick > 100 {
            if let Some(l3) = self.nodes.iter().find(|n| n.role == NodeRole::L3Sequencer) {
                if l3.alive && l3.block_height == 0 {
                    self.record_violation(
                        "l3_stalled",
                        tick,
                        "L3 sequencer has not produced any blocks".into(),
                    );
                }
            }
        }

        // Invariant 6: Gas prices must never go to zero (broken fee mechanism).
        let zero_gas: Vec<_> = self.nodes.iter()
            .filter(|n| n.alive && n.gas_price == 0)
            .map(|n| n.role)
            .collect();
        for role in zero_gas {
            let key = format!("zero_gas_{:?}", role);
            self.record_violation(
                &key,
                tick,
                format!("{:?} gas price dropped to 0", role),
            );
        }

        // Invariant 7: Retryable tickets should not stay unredeemed too long.
        if tick > 200 {
            let stuck = self.retryable_tickets.iter()
                .filter(|t| !t.redeemed && tick > t.deliver_tick + 100)
                .count();
            if stuck > 5 {
                self.record_violation(
                    "retryables_stuck",
                    tick,
                    format!("{} retryable tickets stuck unredeemed for >100 ticks", stuck),
                );
            }
        }

        // Invariant 8: Bridge accounting — no value created from nothing.
        // Total deposited across all nodes should not exceed total bridge value originated.
        let total_deposited: u64 = self.nodes.iter().map(|n| n.total_deposited).sum();
        let total_sent = self.total_l1_to_l2_value
            + self.total_l2_to_l1_value
            + self.total_l2_to_l3_value;
        if total_deposited > total_sent {
            self.record_violation(
                "bridge_inflation",
                tick,
                format!(
                    "Bridge inflation: deposited {} > sent {}",
                    total_deposited, total_sent
                ),
            );
        }
    }
}

pub struct SimResult {
    pub seed: u64,
    pub ticks: u64,
    pub violations: Vec<SimViolation>,
    pub network_stats: crate::network::NetworkStats,
}

impl SimResult {
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Run a campaign of many seeds — the TigerBeetle approach.
pub fn run_campaign(num_seeds: u64, ticks_per_seed: u64, starting_seed: u64) -> CampaignResult {
    let mut result = CampaignResult {
        seeds_run: 0,
        seeds_passed: 0,
        seeds_failed: 0,
        total_ticks: 0,
        violations: Vec::new(),
    };

    for i in 0..num_seeds {
        let seed = starting_seed + i;
        let mut sim = Simulator::new(seed);
        let sr = sim.run(ticks_per_seed);

        result.seeds_run += 1;
        result.total_ticks += sr.ticks;

        if sr.passed() {
            result.seeds_passed += 1;
        } else {
            result.seeds_failed += 1;
            // Cap violations per seed to avoid memory explosion.
            for v in sr.violations.into_iter().take(10) {
                result.violations.push(v);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_same_seed_same_result() {
        let mut a = Simulator::new(1337);
        let mut b = Simulator::new(1337);
        let ra = a.run(1000);
        let rb = b.run(1000);
        assert_eq!(ra.violations.len(), rb.violations.len());
        assert_eq!(ra.ticks, rb.ticks);
    }

    #[test]
    fn campaign_runs_multiple_seeds() {
        let result = run_campaign(100, 500, 0);
        assert_eq!(result.seeds_run, 100);
        assert_eq!(result.total_ticks, 100 * 500);
    }
}
