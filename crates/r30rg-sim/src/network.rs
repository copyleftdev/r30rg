use r30rg_core::prng::DeterministicRng;
use std::collections::{HashSet, VecDeque};

/// Simulated network layer — models message passing between rollup components
/// with controllable latency, loss, reordering, and partitions.
///
/// All decisions are made via the deterministic PRNG: same seed = same network fate.
#[derive(Debug)]
pub struct SimulatedNetwork {
    config: NetworkConfig,
    in_flight: VecDeque<InFlightMessage>,
    partitions: HashSet<(NodeId, NodeId)>,
    delivered: u64,
    dropped: u64,
    duplicated: u64,
}

pub type NodeId = u32;

#[derive(Debug, Clone)]
pub struct NetworkConfig {
    pub message_loss_rate: f64,
    pub duplication_rate: f64,
    pub reorder_rate: f64,
    pub min_latency_ticks: u64,
    pub max_latency_ticks: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            message_loss_rate: 0.01,
            duplication_rate: 0.001,
            reorder_rate: 0.05,
            min_latency_ticks: 1,
            max_latency_ticks: 50,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InFlightMessage {
    pub from: NodeId,
    pub to: NodeId,
    pub payload: Vec<u8>,
    pub deliver_at_tick: u64,
}

impl SimulatedNetwork {
    pub fn new(config: NetworkConfig) -> Self {
        Self {
            config,
            in_flight: VecDeque::new(),
            partitions: HashSet::new(),
            delivered: 0,
            dropped: 0,
            duplicated: 0,
        }
    }

    /// Send a message through the simulated network.
    pub fn send(
        &mut self,
        rng: &mut DeterministicRng,
        current_tick: u64,
        from: NodeId,
        to: NodeId,
        payload: Vec<u8>,
    ) {
        // Partitioned? Silent drop.
        if self.is_partitioned(from, to) {
            self.dropped += 1;
            return;
        }

        // Random loss.
        if rng.chance(self.config.message_loss_rate) {
            self.dropped += 1;
            return;
        }

        let latency = rng.range(self.config.min_latency_ticks, self.config.max_latency_ticks);
        let deliver_at = current_tick + latency;

        self.in_flight.push_back(InFlightMessage {
            from,
            to,
            payload: payload.clone(),
            deliver_at_tick: deliver_at,
        });

        // Possible duplication.
        if rng.chance(self.config.duplication_rate) {
            let extra_latency = rng.range(self.config.min_latency_ticks, self.config.max_latency_ticks);
            self.in_flight.push_back(InFlightMessage {
                from,
                to,
                payload,
                deliver_at_tick: deliver_at + extra_latency,
            });
            self.duplicated += 1;
        }
    }

    /// Collect all messages due for delivery at or before `tick`.
    pub fn deliver(&mut self, tick: u64) -> Vec<InFlightMessage> {
        let mut ready = Vec::new();
        let mut remaining = VecDeque::new();

        for msg in self.in_flight.drain(..) {
            if msg.deliver_at_tick <= tick {
                self.delivered += 1;
                ready.push(msg);
            } else {
                remaining.push_back(msg);
            }
        }

        self.in_flight = remaining;
        ready
    }

    /// Inject a network partition between two sets of nodes.
    pub fn partition(&mut self, group_a: &[NodeId], group_b: &[NodeId]) {
        for &a in group_a {
            for &b in group_b {
                self.partitions.insert((a, b));
                self.partitions.insert((b, a));
            }
        }
    }

    /// Heal all partitions.
    pub fn heal_all(&mut self) {
        self.partitions.clear();
    }

    /// Heal a specific partition.
    pub fn heal(&mut self, group_a: &[NodeId], group_b: &[NodeId]) {
        for &a in group_a {
            for &b in group_b {
                self.partitions.remove(&(a, b));
                self.partitions.remove(&(b, a));
            }
        }
    }

    fn is_partitioned(&self, a: NodeId, b: NodeId) -> bool {
        self.partitions.contains(&(a, b))
    }

    pub fn stats(&self) -> NetworkStats {
        NetworkStats {
            in_flight: self.in_flight.len() as u64,
            delivered: self.delivered,
            dropped: self.dropped,
            duplicated: self.duplicated,
            active_partitions: self.partitions.len() as u64 / 2,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NetworkStats {
    pub in_flight: u64,
    pub delivered: u64,
    pub dropped: u64,
    pub duplicated: u64,
    pub active_partitions: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_drops_messages() {
        let mut net = SimulatedNetwork::new(NetworkConfig {
            message_loss_rate: 0.0,
            ..Default::default()
        });
        let mut rng = DeterministicRng::new(42);

        net.partition(&[0], &[1]);
        net.send(&mut rng, 0, 0, 1, vec![1, 2, 3]);

        let msgs = net.deliver(1000);
        assert!(msgs.is_empty(), "partitioned message should be dropped");
        assert_eq!(net.stats().dropped, 1);
    }

    #[test]
    fn messages_deliver_after_latency() {
        let mut net = SimulatedNetwork::new(NetworkConfig {
            message_loss_rate: 0.0,
            min_latency_ticks: 10,
            max_latency_ticks: 10,
            ..Default::default()
        });
        let mut rng = DeterministicRng::new(42);

        net.send(&mut rng, 0, 0, 1, vec![42]);

        let early = net.deliver(5);
        assert!(early.is_empty());

        let on_time = net.deliver(10);
        assert_eq!(on_time.len(), 1);
        assert_eq!(on_time[0].payload, vec![42]);
    }
}
