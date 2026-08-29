//! The real-time command queue.
//!
//! `plans/product/01-technical-architecture.md` §8 puts every control-plane change on a
//! single-producer/single-consumer ring, and §1.2 drains it at the top of the render
//! loop. This is that ring, at spike scale: one variant, a fixed capacity, no allocation
//! and no way to panic.
//!
//! It is deliberately *not* the cross-thread transport. A wasm instance's linear memory
//! is not visible to the page unless the module is built with shared memory
//! (`-C target-feature=+atomics`), which the plain `cargo build` toolchain this project
//! pins does not do. The page therefore writes into a `SharedArrayBuffer` ring; the
//! worklet — which runs on the same thread as the wasm instance — drains that ring and
//! pushes what it found in here. This ring is what `process()` reads, so the shape the
//! engine sees in M1 is already the shape it will keep: *drain a Rust SPSC ring at the
//! top of every render pass*. Only the producer moves.

/// Ring capacity in commands. A power of two so the wrap is a mask. One quantum is
/// 2.7 ms at 48 kHz, and no human input device produces 64 events in 2.7 ms, so the ring
/// overflowing means something upstream is broken rather than merely busy.
pub const COMMAND_RING_CAPACITY: usize = 64;

const CAPACITY_MASK: usize = COMMAND_RING_CAPACITY - 1;

/// The spike's entire command vocabulary. M1 replaces this with the engine's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    SetFrequency(f32),
}

/// A bounded SPSC queue of [`Command`]s.
///
/// `push` is the producer side and `drain` the consumer side; neither allocates and
/// neither can panic. A full ring drops the oldest-arriving command and counts the drop
/// rather than blocking, because the alternative in an audio callback is a glitch.
pub struct CommandRing {
    slots: [Command; COMMAND_RING_CAPACITY],
    write_index: usize,
    read_index: usize,
    dropped: u32,
}

impl CommandRing {
    pub fn new() -> Self {
        Self {
            slots: [Command::SetFrequency(0.0); COMMAND_RING_CAPACITY],
            write_index: 0,
            read_index: 0,
            dropped: 0,
        }
    }

    /// Enqueues one command. Returns `false` — and counts a drop — if the ring is full.
    pub fn push(&mut self, command: Command) -> bool {
        if self.write_index.wrapping_sub(self.read_index) >= COMMAND_RING_CAPACITY {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.slots[self.write_index & CAPACITY_MASK] = command;
        self.write_index = self.write_index.wrapping_add(1);
        true
    }

    /// Removes and returns the next command, oldest first.
    pub fn pop(&mut self) -> Option<Command> {
        if self.read_index == self.write_index {
            return None;
        }
        let command = self.slots[self.read_index & CAPACITY_MASK];
        self.read_index = self.read_index.wrapping_add(1);
        Some(command)
    }

    /// Number of commands the ring has had to discard since init. Surfaced to the page:
    /// a non-zero value is the spike failing, not a curiosity.
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    pub fn is_empty(&self) -> bool {
        self.read_index == self.write_index
    }
}

impl Default for CommandRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_come_back_out_in_order() {
        let mut ring = CommandRing::new();
        assert!(ring.push(Command::SetFrequency(110.0)));
        assert!(ring.push(Command::SetFrequency(440.0)));
        assert_eq!(ring.pop(), Some(Command::SetFrequency(110.0)));
        assert_eq!(ring.pop(), Some(Command::SetFrequency(440.0)));
        assert_eq!(ring.pop(), None);
    }

    #[test]
    fn the_ring_wraps_without_losing_anything() {
        let mut ring = CommandRing::new();
        for round in 0..1000u32 {
            assert!(ring.push(Command::SetFrequency(round as f32)));
            assert_eq!(ring.pop(), Some(Command::SetFrequency(round as f32)));
        }
        assert_eq!(ring.dropped(), 0);
        assert!(ring.is_empty());
    }

    #[test]
    fn overflow_is_counted_rather_than_fatal() {
        let mut ring = CommandRing::new();
        for index in 0..COMMAND_RING_CAPACITY { assert!(ring.push(Command::SetFrequency(index as f32))); }
        assert!(!ring.push(Command::SetFrequency(999.0)), "the ring is full");
        assert_eq!(ring.dropped(), 1);
        assert_eq!(ring.pop(), Some(Command::SetFrequency(0.0)), "what was already queued survives");
    }
}
