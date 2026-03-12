use rand::SeedableRng;
use rand_chacha::ChaCha20Rng;
use rand::Rng;

/// Deterministic PRNG — same seed = same chaos sequence = reproducible bugs.
/// Uses ChaCha20 for cryptographic-quality randomness with perfect determinism.
#[derive(Debug)]
pub struct DeterministicRng {
    seed: u64,
    rng: ChaCha20Rng,
    operations: u64,
}

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        let mut seed_bytes = [0u8; 32];
        seed_bytes[..8].copy_from_slice(&seed.to_le_bytes());
        Self {
            seed,
            rng: ChaCha20Rng::from_seed(seed_bytes),
            operations: 0,
        }
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn operations(&self) -> u64 {
        self.operations
    }

    pub fn next_u64(&mut self) -> u64 {
        self.operations += 1;
        self.rng.gen()
    }

    pub fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 0
    }

    /// Returns true with the given probability (0.0 – 1.0).
    pub fn chance(&mut self, probability: f64) -> bool {
        let threshold = (probability * u64::MAX as f64) as u64;
        self.next_u64() < threshold
    }

    /// Inclusive range [min, max].
    pub fn range(&mut self, min: u64, max: u64) -> u64 {
        if min >= max {
            return min;
        }
        min + (self.next_u64() % (max - min + 1))
    }

    /// Pick a random element from a non-empty slice.
    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        let idx = self.range(0, items.len() as u64 - 1) as usize;
        &items[idx]
    }

    /// Shuffle a slice in-place (Fisher-Yates).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        let len = items.len();
        for i in (1..len).rev() {
            let j = self.range(0, i as u64) as usize;
            items.swap(i, j);
        }
    }

    /// Fork a child PRNG with a derived seed (for sub-scenarios).
    pub fn fork(&mut self) -> Self {
        let child_seed = self.next_u64();
        Self::new(child_seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_same_seed_same_sequence() {
        let mut a = DeterministicRng::new(42);
        let mut b = DeterministicRng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn deterministic_different_seed_different_sequence() {
        let mut a = DeterministicRng::new(42);
        let mut b = DeterministicRng::new(43);
        let mut same = 0;
        for _ in 0..100 {
            if a.next_u64() == b.next_u64() {
                same += 1;
            }
        }
        assert!(same < 5, "too many collisions: {same}");
    }

    #[test]
    fn range_is_inclusive() {
        let mut rng = DeterministicRng::new(0);
        let mut seen_min = false;
        let mut seen_max = false;
        for _ in 0..10_000 {
            let v = rng.range(5, 10);
            assert!(v >= 5 && v <= 10);
            if v == 5 { seen_min = true; }
            if v == 10 { seen_max = true; }
        }
        assert!(seen_min && seen_max);
    }

    #[test]
    fn fork_produces_different_stream() {
        let mut parent = DeterministicRng::new(99);
        let mut child = parent.fork();
        let mut same = 0;
        for _ in 0..100 {
            if parent.next_u64() == child.next_u64() {
                same += 1;
            }
        }
        assert!(same < 5);
    }
}
