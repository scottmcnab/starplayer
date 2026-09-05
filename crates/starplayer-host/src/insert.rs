//! [`HostInsertControl`] and [`InsertLayout`] — the host's half of the insert control
//! plane (M7-H7), the surface [`crate::Player::install_insert`] and its siblings are built
//! on.
//!
//! # Why a runtime enum, and not a type parameter
//!
//! [`starplayer_engine::insert::InsertHandle`] is generic over `Sample: DspSample`, because
//! master-plan decision 2 puts one effect body on both mix paths. Which path a `Player` is
//! actually running is a runtime choice — [`MixerMode`](starplayer::engine::MixerMode), read
//! from a CLI flag or a saved preference — so the handle a `Player` holds cannot be a
//! compile-time type parameter without making `Player` itself generic over it, which would
//! turn every one of its methods, and every test that builds one, into a second thing to
//! monomorphise. [`HostInsertControl`] is the same device `crate::engine::EngineArm` already
//! uses for the engine itself: a small enum with exactly the arms a host builds, matched
//! once per call rather than once per instantiation.
//!
//! # Where building happens
//!
//! [`HostInsertControl::install`] calls [`starplayer::dsp::build_insert`] itself, **on
//! whichever thread calls it** — the control thread for a native host, a worklet message
//! task for the browser host — never inside a render callback or the insert command ring's
//! own drain. Building an effect allocates its delay lines (H3, H4), which is exactly why
//! the insert graph is a command ring in the first place (architecture §8).

use std::collections::HashMap;
use std::vec::Vec;

use starplayer::dsp::{InsertKind, ParamId, build_insert};
use starplayer::engine::{InsertCommand, InsertHandle, InsertTarget};

use crate::backend::HostError;

/// The control side of one engine's insert graph, whichever mix path the engine's arm
/// turned out to be built on.
///
/// [`crate::engine::HostEngine::build`] hands one of these out beside the engine itself,
/// built from whichever `Path::Mono` the requested [`MixerMode`](starplayer::engine::MixerMode)
/// resolved to.
pub enum HostInsertControl {
    /// The engine's arm mixes in `f32` — the float path.
    Float(InsertHandle<f32>),
    /// The engine's arm mixes in `i32` — the fixed path.
    Fixed(InsertHandle<i32>),
}

impl HostInsertControl {
    /// Build `kind` at `sample_rate_hz` **here** and queue it for installation in `slot` of
    /// `target`'s chain, retiring whatever was there.
    pub fn install(&mut self, target: InsertTarget, slot: u8, kind: InsertKind, sample_rate_hz: u32) -> Result<(), HostError> {
        let installed = match self {
            HostInsertControl::Float(handle) => handle.install(target, slot, build_insert::<f32>(kind, sample_rate_hz)).is_ok(),
            HostInsertControl::Fixed(handle) => handle.install(target, slot, build_insert::<i32>(kind, sample_rate_hz)).is_ok(),
        };
        if installed { Ok(()) } else { Err(HostError::InsertQueueFull) }
    }

    /// Take whatever is in `slot` of `target`'s chain out and retire it.
    pub fn remove(&mut self, target: InsertTarget, slot: u8) -> Result<(), HostError> {
        self.send(|| InsertCommand::Remove { target, slot }, || InsertCommand::Remove { target, slot })
    }

    /// Set one parameter of the effect in `slot` of `target`'s chain.
    pub fn set_param(&mut self, target: InsertTarget, slot: u8, param: ParamId, value: i32) -> Result<(), HostError> {
        self.send(|| InsertCommand::SetParam { target, slot, param, value }, || InsertCommand::SetParam { target, slot, param, value })
    }

    /// Skip, or stop skipping, the effect in `slot` of `target`'s chain.
    pub fn bypass(&mut self, target: InsertTarget, slot: u8, bypassed: bool) -> Result<(), HostError> {
        self.send(|| InsertCommand::Bypass { target, slot, bypassed }, || InsertCommand::Bypass { target, slot, bypassed })
    }

    /// Clear every delay line and envelope in every chain, keeping parameters — what a
    /// seek sends.
    pub fn reset_all(&mut self) -> Result<(), HostError> { self.send(|| InsertCommand::ResetAll, || InsertCommand::ResetAll) }

    fn send(
        &mut self,
        float_command: impl FnOnce() -> InsertCommand<f32>,
        fixed_command: impl FnOnce() -> InsertCommand<i32>,
    ) -> Result<(), HostError> {
        let sent = match self {
            HostInsertControl::Float(handle) => handle.send(float_command()).is_ok(),
            HostInsertControl::Fixed(handle) => handle.send(fixed_command()).is_ok(),
        };
        if sent { Ok(()) } else { Err(HostError::InsertQueueFull) }
    }

    /// Drop every retired insert waiting, and report how many that was. **Call this
    /// regularly**, exactly as [`crate::Player::collect_garbage`] does for modules.
    pub fn collect_all_garbage(&mut self) -> usize {
        match self {
            HostInsertControl::Float(handle) => handle.collect_all_garbage(),
            HostInsertControl::Fixed(handle) => handle.collect_all_garbage(),
        }
    }
}

/// One effect the host believes is installed, from the host's own point of view.
///
/// Never read back from the engine: the insert control ring is one-way, so what a
/// parameter's smoothed value has settled to inside the audio thread is not something a
/// caller can ask for. This is the host's own record of what it last *asked* to be true —
/// which is what [`Player::set_mixer_mode`](crate::Player::set_mixer_mode) replays into a
/// freshly built engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledInsert {
    /// Which effect.
    pub kind: InsertKind,
    /// Whether the host last asked the engine to skip this effect.
    pub bypassed: bool,
    params: Vec<(ParamId, i32)>,
}

impl InstalledInsert {
    fn fresh(kind: InsertKind) -> InstalledInsert { InstalledInsert { kind, bypassed: false, params: Vec::new() } }

    /// The last value this host sent for `param`, or `None` if it never has — meaning the
    /// effect is still at that parameter's own default.
    pub fn param(&self, param: ParamId) -> Option<i32> { self.params.iter().find(|(id, _)| *id == param).map(|(_, value)| *value) }

    /// Every parameter this host has explicitly set, in no particular order.
    pub fn params(&self) -> &[(ParamId, i32)] { &self.params }

    fn set_param(&mut self, param: ParamId, value: i32) {
        match self.params.iter_mut().find(|(id, _)| *id == param) {
            Some(entry) => entry.1 = value,
            None => self.params.push((param, value)),
        }
    }
}

/// What the host believes is installed, per target and slot ([`Player::inserts`](crate::Player::inserts)).
///
/// Rebuilt across a [`Player::set_mixer_mode`](crate::Player::set_mixer_mode): the freshly
/// opened engine starts with an empty insert graph, and `Player` replays every entry here
/// into it — the answer research point 1 in `plans/engine/M7-task-H7-host-wiring.md` gives.
#[derive(Clone, Debug, Default)]
pub struct InsertLayout {
    installed: HashMap<(InsertTarget, u8), InstalledInsert>,
}

impl InsertLayout {
    /// What the host believes is in `slot` of `target`'s chain, if anything.
    pub fn get(&self, target: InsertTarget, slot: u8) -> Option<&InstalledInsert> { self.installed.get(&(target, slot)) }

    /// Every installed entry, target and slot alongside it — for a caller replaying the
    /// layout into a rebuilt engine, or a UI drawing every occupied slot.
    pub fn iter(&self) -> impl Iterator<Item = (InsertTarget, u8, &InstalledInsert)> {
        self.installed.iter().map(|(&(target, slot), insert)| (target, slot, insert))
    }

    pub(crate) fn record_install(&mut self, target: InsertTarget, slot: u8, kind: InsertKind) {
        self.installed.insert((target, slot), InstalledInsert::fresh(kind));
    }

    pub(crate) fn record_remove(&mut self, target: InsertTarget, slot: u8) { self.installed.remove(&(target, slot)); }

    pub(crate) fn record_param(&mut self, target: InsertTarget, slot: u8, param: ParamId, value: i32) {
        if let Some(entry) = self.installed.get_mut(&(target, slot)) {
            entry.set_param(param, value);
        }
    }

    pub(crate) fn record_bypassed(&mut self, target: InsertTarget, slot: u8, bypassed: bool) {
        if let Some(entry) = self.installed.get_mut(&(target, slot)) {
            entry.bypassed = bypassed;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_layout_knows_nothing() {
        let layout = InsertLayout::default();
        assert!(layout.get(InsertTarget::Master, 0).is_none());
        assert_eq!(layout.iter().count(), 0);
    }

    #[test]
    fn install_then_set_param_then_remove_round_trips() {
        let mut layout = InsertLayout::default();
        let target = InsertTarget::Channel(starplayer::core::ChannelId(0));
        layout.record_install(target, 1, InsertKind::Reverb);
        assert_eq!(layout.get(target, 1).map(|insert| insert.kind), Some(InsertKind::Reverb));
        assert_eq!(layout.get(target, 1).and_then(|insert| insert.param(ParamId(0))), None);

        layout.record_param(target, 1, ParamId(0), 60);
        assert_eq!(layout.get(target, 1).and_then(|insert| insert.param(ParamId(0))), Some(60));
        layout.record_param(target, 1, ParamId(0), 70);
        assert_eq!(layout.get(target, 1).and_then(|insert| insert.param(ParamId(0))), Some(70), "a second set overwrites, not appends");

        layout.record_bypassed(target, 1, true);
        assert!(layout.get(target, 1).unwrap().bypassed);

        layout.record_remove(target, 1);
        assert!(layout.get(target, 1).is_none());
    }
}
