//! The insert control plane: what a chain is, and how an effect gets into one
//! (M7 master-plan decision 4, H1 deliverable 6).
//!
//! # Why an effect arrives by command
//!
//! Building an effect allocates — its delay lines, its coefficient sets — and allocation
//! inside `render()` is banned (design goal 5, proved by
//! `crates/starplayer-offline/tests/render_allocation.rs`). So the host builds the effect
//! on its own thread, boxes it, and pushes it onto an SPSC ring; the engine drains that
//! ring at the top of a quantum with the other commands. An effect that leaves a chain is
//! **retired** over a garbage channel rather than dropped, for exactly the reason a
//! retired `Arc<Module>` is: `free()` takes the allocator's lock, and taking a lock in the
//! middle of an audio callback is the stall the whole control plane exists to prevent
//! (architecture §8).
//!
//! # The topology is fixed
//!
//! [`MAX_INSERTS_PER_CHAIN`] ordered slots per channel bus, and one more chain on the
//! master bus ahead of the master volume and the limiter. Slot order is processing order.
//! This is not a node graph, deliberately: master-plan decision 3.
//!
//! # Bypass is the engine's bit, not the effect's
//!
//! [`InsertCommand::Bypass`] sets a bit the engine reads before calling
//! [`Insert::process`], so bypassing is one branch rather than something every effect has
//! to implement identically. A bypassed effect is still reset and still receives its
//! parameters, so un-bypassing it does not step into a stale state.

use alloc::boxed::Box;

use starplayer_core::ChannelId;
use starplayer_dsp::{DspSample, Insert, ParamId, Stereo};
use starplayer_rt::{GarbageCollector, Producer};

/// Ordered insert slots on one bus. Slot order is processing order.
pub const MAX_INSERTS_PER_CHAIN: usize = 4;

/// Insert commands the ring holds before a control thread that outruns the audio thread
/// starts getting refusals.
///
/// Sixteen is generous for the same reason [`DEFAULT_COMMAND_CAPACITY`](crate::DEFAULT_COMMAND_CAPACITY)
/// is: installing an effect is a human-scale action, and even a slider being dragged
/// produces a `SetParam` per UI frame, two orders of magnitude slower than the drain.
pub const DEFAULT_INSERT_COMMAND_CAPACITY: usize = 16;

/// Retired inserts the garbage channel holds before the audio thread has to drop one
/// itself.
pub const DEFAULT_INSERT_GARBAGE_CAPACITY: usize = 16;

/// Which bus an insert command is about.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum InsertTarget {
    /// One control lane's bus. A lane the engine does not have is ignored.
    Channel(ChannelId),
    /// The master bus, ahead of the master volume and the limiter.
    Master,
}

/// One change to the insert graph, sent from the control thread.
pub enum InsertCommand<Sample: DspSample> {
    /// Put `insert` in `slot`, retiring whatever was there.
    Install {
        /// Which bus.
        target: InsertTarget,
        /// Which slot, `0 ..= MAX_INSERTS_PER_CHAIN - 1`.
        slot: u8,
        /// The effect, built off the audio thread.
        insert: Box<dyn Insert<Sample>>,
    },
    /// Take whatever is in `slot` out and retire it.
    Remove {
        /// Which bus.
        target: InsertTarget,
        /// Which slot.
        slot: u8,
    },
    /// Set one parameter of the effect in `slot`. A bypassed effect still receives it.
    SetParam {
        /// Which bus.
        target: InsertTarget,
        /// Which slot.
        slot: u8,
        /// Which parameter.
        param: ParamId,
        /// The new value, in the parameter's own fixed unit.
        value: i32,
    },
    /// Skip or stop skipping the effect in `slot`.
    Bypass {
        /// Which bus.
        target: InsertTarget,
        /// Which slot.
        slot: u8,
        /// Whether the engine skips it.
        bypassed: bool,
    },
    /// Clear every delay line and envelope in every chain, keeping the parameters. What a
    /// host sends around a seek.
    ResetAll,
}

impl<Sample: DspSample> core::fmt::Debug for InsertCommand<Sample> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            InsertCommand::Install { target, slot, .. } => formatter.debug_struct("Install").field("target", target).field("slot", slot).finish_non_exhaustive(),
            InsertCommand::Remove { target, slot } => formatter.debug_struct("Remove").field("target", target).field("slot", slot).finish(),
            InsertCommand::SetParam { target, slot, param, value } => {
                formatter.debug_struct("SetParam").field("target", target).field("slot", slot).field("param", param).field("value", value).finish()
            }
            InsertCommand::Bypass { target, slot, bypassed } => {
                formatter.debug_struct("Bypass").field("target", target).field("slot", slot).field("bypassed", bypassed).finish()
            }
            InsertCommand::ResetAll => formatter.write_str("ResetAll"),
        }
    }
}

/// The control thread's end of the insert graph.
///
/// Send commands in, take retired inserts out, and **drop them here** — never on the audio
/// thread. `Send` whenever the sample type is, which is always: the point of the handle is
/// that it goes to the UI thread while the [`Engine`](crate::Engine) goes to the callback.
pub struct InsertHandle<Sample: DspSample> {
    commands: Producer<InsertCommand<Sample>>,
    garbage: GarbageCollector<Box<dyn Insert<Sample>>>,
}

impl<Sample: DspSample> InsertHandle<Sample> {
    /// Build the control half around an already-split pair of rings.
    pub(crate) const fn new(
        commands: Producer<InsertCommand<Sample>>,
        garbage: GarbageCollector<Box<dyn Insert<Sample>>>,
    ) -> InsertHandle<Sample> {
        InsertHandle { commands, garbage }
    }

    /// Queue `command` for the audio thread, or hand it back if the ring is full.
    ///
    /// Handing it back matters for [`InsertCommand::Install`]: the caller keeps ownership
    /// of the effect it just built rather than discovering later that it vanished.
    pub fn send(&mut self, command: InsertCommand<Sample>) -> Result<(), InsertCommand<Sample>> { self.commands.push(command) }

    /// Install a freshly built effect, or hand it back if the ring is full.
    pub fn install(&mut self, target: InsertTarget, slot: u8, insert: Box<dyn Insert<Sample>>) -> Result<(), Box<dyn Insert<Sample>>> {
        match self.send(InsertCommand::Install { target, slot, insert }) {
            Ok(()) => Ok(()),
            Err(InsertCommand::Install { insert, .. }) => Err(insert),
            // Unreachable: the ring hands back exactly what was pushed.
            Err(_) => Ok(()),
        }
    }

    /// Take one retired insert, so a caller can inspect it before it is dropped.
    pub fn collect_garbage(&mut self) -> Option<Box<dyn Insert<Sample>>> { self.garbage.collect() }

    /// Drop every retired insert waiting, and report how many that was.
    ///
    /// **Call this from the control thread, regularly.** It is the only place a retired
    /// effect's delay lines are returned to the allocator.
    pub fn collect_all_garbage(&mut self) -> usize { self.garbage.collect_all() }

    /// How many retired inserts are waiting to be dropped.
    pub fn pending_garbage(&self) -> usize { self.garbage.pending() }

    /// How many commands the audio thread has not applied yet.
    pub fn queued_commands(&self) -> usize { self.commands.len() }

    /// How many commands may be queued at once.
    pub fn command_capacity(&self) -> usize { self.commands.capacity() }
}

impl<Sample: DspSample> core::fmt::Debug for InsertHandle<Sample> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("InsertHandle")
            .field("queued_commands", &self.queued_commands())
            .field("pending_garbage", &self.pending_garbage())
            .finish()
    }
}

/// One bus's ordered insert slots, and the bypass bit the engine reads for each.
pub struct InsertChain<Sample: DspSample> {
    slots: [Option<Box<dyn Insert<Sample>>>; MAX_INSERTS_PER_CHAIN],
    bypassed: [bool; MAX_INSERTS_PER_CHAIN],
}

impl<Sample: DspSample> Default for InsertChain<Sample> {
    fn default() -> InsertChain<Sample> {
        InsertChain { slots: [const { None }; MAX_INSERTS_PER_CHAIN], bypassed: [false; MAX_INSERTS_PER_CHAIN] }
    }
}

impl<Sample: DspSample> InsertChain<Sample> {
    /// Whether anything at all is installed. The engine skips a chain that has nothing in
    /// it rather than walking four empty slots per bus per quantum.
    pub fn is_empty(&self) -> bool { self.slots.iter().all(Option::is_none) }

    /// Run every installed, un-bypassed effect over one whole DSP block, in slot order.
    pub fn process(&mut self, block: &mut [Stereo<Sample>]) {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if self.bypassed.get(index).copied().unwrap_or(false) {
                continue;
            }
            if let Some(insert) = slot.as_mut() {
                insert.process(block);
            }
        }
    }

    /// Put `insert` in `slot` and hand back whatever it replaced, or hand `insert` straight
    /// back if there is no such slot.
    pub fn install(&mut self, slot: u8, insert: Box<dyn Insert<Sample>>) -> Option<Box<dyn Insert<Sample>>> {
        match self.slots.get_mut(slot as usize) {
            Some(entry) => entry.replace(insert),
            None => Some(insert),
        }
    }

    /// Take the effect in `slot` out.
    pub fn remove(&mut self, slot: u8) -> Option<Box<dyn Insert<Sample>>> { self.slots.get_mut(slot as usize)?.take() }

    /// Set one parameter of the effect in `slot`, if there is one.
    pub fn set_param(&mut self, slot: u8, param: ParamId, value: i32) {
        if let Some(Some(insert)) = self.slots.get_mut(slot as usize) {
            insert.set_param(param, value);
        }
    }

    /// Set the bypass bit for `slot`.
    pub fn set_bypassed(&mut self, slot: u8, bypassed: bool) {
        if let Some(bit) = self.bypassed.get_mut(slot as usize) {
            *bit = bypassed;
        }
    }

    /// Whether `slot` is bypassed.
    pub fn is_bypassed(&self, slot: u8) -> bool { self.bypassed.get(slot as usize).copied().unwrap_or(false) }

    /// Whether `slot` holds an effect.
    pub fn is_occupied(&self, slot: u8) -> bool { matches!(self.slots.get(slot as usize), Some(Some(_))) }

    /// The effect in `slot`, for a caller that wants to read a parameter back.
    pub fn get(&self, slot: u8) -> Option<&dyn Insert<Sample>> { self.slots.get(slot as usize)?.as_deref() }

    /// Reset every installed effect, bypassed or not.
    pub fn reset(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            slot.reset();
        }
    }
}

impl<Sample: DspSample> core::fmt::Debug for InsertChain<Sample> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let occupied = self.slots.iter().filter(|slot| slot.is_some()).count();
        formatter.debug_struct("InsertChain").field("occupied", &occupied).finish()
    }
}
