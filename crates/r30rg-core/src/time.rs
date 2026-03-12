use std::collections::BinaryHeap;
use std::cmp::Reverse;

/// Simulated time source — fully controlled, compressible.
/// In simulation mode, "sleeping" is free: we just advance the clock.
#[derive(Debug)]
pub struct SimulatedClock {
    tick: u64,
    events: BinaryHeap<Reverse<ScheduledEvent>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScheduledEvent {
    pub tick: u64,
    pub id: u64,
}

impl SimulatedClock {
    pub fn new() -> Self {
        Self {
            tick: 0,
            events: BinaryHeap::new(),
        }
    }

    pub fn now(&self) -> u64 {
        self.tick
    }

    pub fn advance(&mut self, ticks: u64) {
        self.tick += ticks;
    }

    pub fn advance_to(&mut self, tick: u64) {
        assert!(tick >= self.tick, "cannot go backwards in time");
        self.tick = tick;
    }

    pub fn schedule(&mut self, at_tick: u64, event_id: u64) {
        self.events.push(Reverse(ScheduledEvent {
            tick: at_tick,
            id: event_id,
        }));
    }

    /// Pop the next event that is due at or before current tick.
    pub fn next_due_event(&mut self) -> Option<ScheduledEvent> {
        if let Some(Reverse(ev)) = self.events.peek() {
            if ev.tick <= self.tick {
                return self.events.pop().map(|r| r.0);
            }
        }
        None
    }

    /// Peek at when the next event fires (if any).
    pub fn next_event_tick(&self) -> Option<u64> {
        self.events.peek().map(|r| r.0.tick)
    }

    /// Advance to the next event and pop it.
    pub fn advance_to_next_event(&mut self) -> Option<ScheduledEvent> {
        if let Some(tick) = self.next_event_tick() {
            self.advance_to(tick);
            self.next_due_event()
        } else {
            None
        }
    }

    pub fn pending_events(&self) -> usize {
        self.events.len()
    }
}

impl Default for SimulatedClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_fire_in_order() {
        let mut clock = SimulatedClock::new();
        clock.schedule(10, 1);
        clock.schedule(5, 2);
        clock.schedule(15, 3);

        let e1 = clock.advance_to_next_event().unwrap();
        assert_eq!(e1.tick, 5);
        assert_eq!(e1.id, 2);

        let e2 = clock.advance_to_next_event().unwrap();
        assert_eq!(e2.tick, 10);
        assert_eq!(e2.id, 1);

        let e3 = clock.advance_to_next_event().unwrap();
        assert_eq!(e3.tick, 15);
        assert_eq!(e3.id, 3);

        assert!(clock.advance_to_next_event().is_none());
    }
}
