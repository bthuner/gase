//! Rewind: a ring buffer of recent save states.
//!
//! Every few frames the frontend pushes a snapshot. While the player holds
//! the rewind key, snapshots are popped and loaded, newest first. A save
//! state is roughly 150 KiB, so ten seconds of history cost a few tens of
//! megabytes.

use std::collections::VecDeque;

use crate::Genesis;

/// History of recent states.
#[derive(Debug, Clone)]
pub struct Rewind {
    snapshots: VecDeque<Vec<u8>>,
    capacity: usize,
    interval: u32,
    countdown: u32,
}

impl Rewind {
    /// Keep `seconds` of history, snapshotting every `interval` frames.
    #[must_use]
    pub fn new(seconds: u32, interval: u32, frame_rate: f64) -> Self {
        let interval = interval.max(1);
        let capacity = (f64::from(seconds) * frame_rate / f64::from(interval)).ceil() as usize;
        Self { snapshots: VecDeque::with_capacity(capacity), capacity: capacity.max(1), interval, countdown: 0 }
    }

    /// Call once per emulated frame; takes a snapshot every `interval` frames.
    pub fn record(&mut self, genesis: &Genesis) {
        if self.countdown > 0 {
            self.countdown -= 1;
            return;
        }
        self.countdown = self.interval - 1;
        // Reuse the oldest buffer's allocation when full.
        if self.snapshots.len() == self.capacity {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(genesis.save_state());
    }

    /// Step back to the most recent snapshot. Returns false when history is
    /// exhausted.
    pub fn step_back(&mut self, genesis: &mut Genesis) -> bool {
        match self.snapshots.pop_back() {
            Some(state) => {
                // A state produced by this very console cannot fail to load.
                let _ = genesis.load_state(&state);
                self.countdown = 0;
                true
            }
            None => false,
        }
    }

    /// Forget all history (e.g. after loading a save state).
    pub fn clear(&mut self) {
        self.snapshots.clear();
        self.countdown = 0;
    }

    /// Number of snapshots held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    /// Is the history empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }
}
