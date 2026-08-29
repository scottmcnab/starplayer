//! The control plane: the SPSC command ring, the garbage channel, and the placeholder for
//! the module handle the two of them carry (architecture §8).

use alloc::boxed::Box;
use alloc::vec::Vec;

use starplayer_core::Command;
use starplayer_rt::{Arc, GarbageCollector, Producer};

/// Commands the ring holds before a control thread that outruns the audio thread starts
/// getting refusals.
///
/// 64 is generous: commands are sparse — load, seek, mute, master volume — and the audio
/// thread drains up to [`MAX_COMMANDS_PER_QUANTUM`] of them every 128 frames, which is
/// roughly 350 Hz at 44.1 kHz.
pub const DEFAULT_COMMAND_CAPACITY: usize = 64;

/// Retired module handles the garbage channel holds before the audio thread has to drop
/// one itself.
///
/// Small on purpose. Loading a module is a human-scale action; a host that has eight
/// retired modules outstanding has stopped collecting garbage entirely, and the warning
/// that then appears is the useful signal.
pub const DEFAULT_GARBAGE_CAPACITY: usize = 8;

/// Commands applied per render quantum.
///
/// The bound is the point: an unbounded drain would let a control thread that floods the
/// ring stall the audio callback for as long as it kept writing.
pub const MAX_COMMANDS_PER_QUANTUM: usize = 32;

/// What the engine plays: sample data, addressed by the mixer's offsets.
///
/// **A placeholder for `Arc<Module>`.** `Module` is `starplayer-model`'s, and it lands with
/// task B1 in parallel with this one, so the engine is generic over "something that can
/// hand out a PCM blob" instead of naming a type that does not exist yet. When `Module`
/// arrives it implements this trait and `Engine<.., Arc<Module>>` is the only change at any
/// call site — the command ring, the garbage channel and the handle swap are already the
/// shape they will keep.
///
/// The blanket implementation for [`Arc`] is what makes `Command::LoadModule(Arc<Module>)`
/// work, and [`Arc`] is `starplayer-rt`'s — `portable-atomic-util`'s, not `alloc`'s,
/// because `alloc::sync::Arc` does not exist on a target without compare-and-swap.
pub trait PcmSource {
    /// Every sample's frames, concatenated, each with guard frames appended
    /// (architecture §6).
    fn pcm(&self) -> &[i16];
}

/// "No module": an engine that has not been given one plays whatever `set_pcm` left it.
impl PcmSource for () {
    fn pcm(&self) -> &[i16] { &[] }
}

impl PcmSource for Vec<i16> {
    fn pcm(&self) -> &[i16] { self }
}

impl PcmSource for Box<[i16]> {
    fn pcm(&self) -> &[i16] { self }
}

impl<Source: PcmSource> PcmSource for Arc<Source> {
    fn pcm(&self) -> &[i16] { (**self).pcm() }
}

/// The control thread's end of the engine.
///
/// Send commands in, take retired modules out, and **drop them here** — never on the audio
/// thread, where `free()` takes the allocator's lock in the middle of a callback.
///
/// Both halves are wait-free and neither allocates. This type is `Send` whenever the module
/// handle is, which is the whole point: it goes to the UI thread, the worker that loads
/// modules, or a `postMessage` handler, while the [`Engine`](crate::Engine) goes to the
/// audio callback.
pub struct EngineHandle<Module> {
    commands: Producer<Command<Module>>,
    garbage: GarbageCollector<Module>,
}

impl<Module> EngineHandle<Module> {
    /// Build the control half around an already-split pair of rings.
    pub(crate) const fn new(commands: Producer<Command<Module>>, garbage: GarbageCollector<Module>) -> EngineHandle<Module> {
        EngineHandle { commands, garbage }
    }

    /// Queue `command` for the audio thread, or hand it back if the ring is full.
    ///
    /// Handing it back rather than dropping it matters for
    /// [`Command::LoadModule`](starplayer_core::Command::LoadModule): the caller keeps
    /// ownership of the module it just spent milliseconds decoding and can retry, rather
    /// than discovering later that it silently vanished.
    pub fn send(&mut self, command: Command<Module>) -> Result<(), Command<Module>> { self.commands.push(command) }

    /// Hand a freshly loaded module to the audio thread.
    pub fn load_module(&mut self, module: Module) -> Result<(), Module> {
        match self.send(Command::LoadModule(module)) {
            Ok(()) => Ok(()),
            Err(Command::LoadModule(module)) => Err(module),
            // Unreachable: the ring hands back exactly what was pushed.
            Err(_) => Ok(()),
        }
    }

    /// Take one retired module, so a caller can inspect it before it is dropped.
    pub fn collect_garbage(&mut self) -> Option<Module> { self.garbage.collect() }

    /// Drop every retired module waiting, and report how many that was.
    ///
    /// **Call this from the control thread, regularly.** It is the only place a retired
    /// module's memory is returned to the allocator.
    pub fn collect_all_garbage(&mut self) -> usize { self.garbage.collect_all() }

    /// How many retired modules are waiting to be dropped.
    pub fn pending_garbage(&self) -> usize { self.garbage.pending() }

    /// How many commands the audio thread has not applied yet.
    pub fn queued_commands(&self) -> usize { self.commands.len() }

    /// How many commands may be queued at once.
    pub fn command_capacity(&self) -> usize { self.commands.capacity() }
}

impl<Module> core::fmt::Debug for EngineHandle<Module> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("EngineHandle")
            .field("queued_commands", &self.queued_commands())
            .field("pending_garbage", &self.pending_garbage())
            .finish()
    }
}
