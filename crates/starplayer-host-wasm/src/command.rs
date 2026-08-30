//! Fixed-capacity staging for typed commands decoded from the browser wire protocol.
//!
//! The `SharedArrayBuffer` ring crosses JavaScript realms. Its consumer calls
//! `enqueue_command`, which decodes each record to the public
//! `starplayer_core::Command` vocabulary and places it here. The Rust host drains this
//! queue immediately before `Engine::render`, preserving the engine's control-plane
//! shape without allocating in `process()`.

/// Records accepted between render quanta.
pub const COMMAND_RING_CAPACITY: usize = 64;

const CAPACITY_MASK: usize = COMMAND_RING_CAPACITY - 1;

/// The compact values carried across the JavaScript-to-wasm edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WireCommand {
    pub opcode: u8,
    pub argument: u32,
    pub extra: u32,
}

/// A bounded, allocation-free queue. It is SPSC at the browser boundary: the worklet's
/// command consumer is the only producer here and `Host::process` the only consumer.
pub struct CommandRing {
    slots: [WireCommand; COMMAND_RING_CAPACITY],
    write_index: usize,
    read_index: usize,
    dropped: u32,
}

impl CommandRing {
    pub const fn new() -> CommandRing {
        CommandRing {
            slots: [WireCommand { opcode: 0, argument: 0, extra: 0 }; COMMAND_RING_CAPACITY],
            write_index: 0,
            read_index: 0,
            dropped: 0,
        }
    }

    pub fn push(&mut self, command: WireCommand) -> bool {
        if self.write_index.wrapping_sub(self.read_index) >= COMMAND_RING_CAPACITY {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.slots[self.write_index & CAPACITY_MASK] = command;
        self.write_index = self.write_index.wrapping_add(1);
        true
    }

    pub fn pop(&mut self) -> Option<WireCommand> {
        if self.read_index == self.write_index {
            return None;
        }
        let command = self.slots[self.read_index & CAPACITY_MASK];
        self.read_index = self.read_index.wrapping_add(1);
        Some(command)
    }

    pub const fn dropped(&self) -> u32 { self.dropped }
}

impl Default for CommandRing {
    fn default() -> CommandRing { CommandRing::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_records_round_trip_without_allocation() {
        let mut ring = CommandRing::new();
        let command = WireCommand { opcode: 3, argument: 17, extra: 2 };
        assert!(ring.push(command));
        assert_eq!(ring.pop(), Some(command));
        assert_eq!(ring.pop(), None);
    }

    #[test]
    fn overflow_is_visible_and_preserves_queued_records() {
        let mut ring = CommandRing::new();
        for index in 0..COMMAND_RING_CAPACITY {
            assert!(ring.push(WireCommand { opcode: 1, argument: index as u32, extra: 0 }));
        }
        assert!(!ring.push(WireCommand::default()));
        assert_eq!(ring.dropped(), 1);
        assert_eq!(ring.pop().map(|command| command.argument), Some(0));
    }
}
