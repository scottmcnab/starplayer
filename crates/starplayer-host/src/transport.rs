//! The transport: a click-free stop, and the fade a song gets when it reaches its loop
//! point.
//!
//! Two gains, deliberately computed differently.
//!
//! * The **transport ramp** is a [`GainRamp`] over 64 frames. It exists so that Play and
//!   Stop are not a discontinuity in the waveform, which is a click on every speaker.
//! * The **song fade** is computed from an elapsed position on every frame rather than
//!   accumulated. A `GainRamp` is built for 64-frame transitions; over a five-second fade
//!   its integer increment rounds to zero, the gain holds at unity and then snaps to
//!   silence. Position-based is exact over any length and at any block size.

use starplayer::core::AtEnd;
use starplayer::dsp::GainRamp;

/// Frames the Play/Stop ramp takes. 64 is under one and a half milliseconds at 44.1 kHz —
/// inaudible as a level change, long enough that no speaker hears an edge.
pub const TRANSPORT_RAMP_FRAMES: u32 = 64;

/// The ramp's full-scale value, in the integer units [`GainRamp`] works in.
pub const TRANSPORT_GAIN_UNITY: i32 = 32_767;

/// The fade length used when a caller asks for a fade without naming one: ten seconds at
/// 48 kHz, which is what an export button means by "fade out".
pub const DEFAULT_FADE_FRAMES: u32 = 10 * 48_000;

/// Everything about *how loud the transport is* and *why*.
///
/// Lives on the audio side and is stepped once per output frame. Nothing here allocates or
/// branches on anything but its own state.
#[derive(Debug)]
pub struct Transport {
    gain: GainRamp,
    fade_frames: u32,
    fading: bool,
    fade_elapsed: u32,
    /// A typed `Command::Stop` waiting for the ramp to reach zero.
    pending_stop: bool,
    /// Whether that queued stop is the end of the song rather than a Stop the user asked
    /// for, and so must rewind to the top once it lands.
    pending_rewind: bool,
}

impl Default for Transport {
    fn default() -> Transport { Transport::stopped() }
}

impl Transport {
    /// A transport at unity, as an engine that starts playing wants it.
    pub fn playing() -> Transport { Transport::with_gain(TRANSPORT_GAIN_UNITY) }

    /// A transport at silence, as a host that waits for Play wants it.
    pub fn stopped() -> Transport { Transport::with_gain(0) }

    fn with_gain(gain: i32) -> Transport {
        Transport {
            gain: GainRamp::steady(gain),
            fade_frames: DEFAULT_FADE_FRAMES,
            fading: false,
            fade_elapsed: 0,
            pending_stop: false,
            pending_rewind: false,
        }
    }

    /// Glide up to unity. A Play during a fade or after the song ended is "again", not
    /// "louder", so the caller clears the fade first — [`Transport::take_restart`] says
    /// whether it has to.
    pub fn begin_play(&mut self) {
        self.pending_stop = false;
        self.gain.glide_to(TRANSPORT_GAIN_UNITY, TRANSPORT_RAMP_FRAMES);
    }

    /// Whether Play means "from the top": the song faded out, or it ended and stopped.
    /// Consumes the flag, and cancels a running fade.
    pub fn take_restart(&mut self) -> bool {
        let restart = self.fading || self.pending_rewind;
        self.fading = false;
        self.pending_rewind = false;
        restart
    }

    /// Ramp down over [`TRANSPORT_RAMP_FRAMES`] and queue the typed engine stop for when
    /// the ramp lands. The Stop button's whole body, shared with the end of a song so the
    /// two stop identically and through one code path.
    pub fn begin_stop(&mut self, rewind: bool) {
        self.gain.glide_to(0, TRANSPORT_RAMP_FRAMES);
        self.pending_stop = true;
        self.pending_rewind |= rewind;
    }

    /// Arm the song fade at its start.
    pub fn begin_fade(&mut self) {
        self.fading = true;
        self.fade_elapsed = 0;
    }

    /// Set the fade length. Zero means "leave the previous one alone", so a caller that
    /// only wants to change the mode does not have to restate it.
    pub fn set_fade_frames(&mut self, fade_frames: u32) {
        if fade_frames > 0 {
            self.fade_frames = fade_frames;
        }
    }

    /// Take a running fade back from wherever it has got to.
    ///
    /// Choosing anything but [`AtEnd::FadeOut`] mid-fade is a toggle, not a one-way door:
    /// the gain picks up at the faded level and glides home over the usual 64 frames.
    pub fn cancel_fade(&mut self, at_end: AtEnd) {
        if at_end == AtEnd::FadeOut || !self.fading {
            return;
        }
        let faded = self.faded_gain(self.gain.current());
        self.fading = false;
        self.gain = GainRamp::steady(faded);
        self.gain.glide_to(TRANSPORT_GAIN_UNITY, TRANSPORT_RAMP_FRAMES);
    }

    /// Whether the song fade is running.
    pub const fn is_fading(&self) -> bool { self.fading }

    /// Whether a typed engine stop is waiting for the ramp.
    pub const fn stop_is_pending(&self) -> bool { self.pending_stop }

    /// Where the ramp is heading.
    pub fn target(&self) -> i32 { self.gain.target() }

    /// One frame's gain, in units of [`TRANSPORT_GAIN_UNITY`], advancing both the ramp and
    /// the fade.
    pub fn advance(&mut self) -> i32 {
        let ramped = self.gain.advance();
        let gain = self.faded_gain(ramped);
        if self.fading {
            self.fade_elapsed = self.fade_elapsed.saturating_add(1);
        }
        gain
    }

    /// `gain` scaled by where the song fade has got to: untouched before it starts, zero on
    /// its last frame, linear in between.
    fn faded_gain(&self, gain: i32) -> i32 {
        if !self.fading || self.fade_frames == 0 {
            return gain;
        }
        let remaining = self.fade_frames.saturating_sub(self.fade_elapsed) as i64;
        (gain as i64 * remaining / self.fade_frames as i64) as i32
    }

    /// Whether the fade has run its course, so the caller should stop and rewind.
    pub const fn fade_has_landed(&self) -> bool { self.fading && self.fade_elapsed >= self.fade_frames }

    /// Whether the queued engine stop is now due, because the ramp has finished.
    pub fn stop_has_landed(&self) -> bool { self.pending_stop && !self.gain.is_ramping() }

    /// Consume the queued stop and report whether it also asked for a rewind.
    pub fn take_stop(&mut self) -> bool {
        self.pending_stop = false;
        core::mem::take(&mut self.pending_rewind)
    }

    /// Consume the finished fade.
    pub fn take_fade(&mut self) { self.fading = false; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stop_ramps_down_and_only_then_reports_the_engine_stop_as_due() {
        let mut transport = Transport::playing();
        transport.begin_stop(false);
        assert!(transport.stop_is_pending());
        assert!(!transport.stop_has_landed(), "the ramp has not started yet");
        for _ in 0..TRANSPORT_RAMP_FRAMES { transport.advance(); }
        assert!(transport.stop_has_landed());
        assert!(!transport.take_stop(), "a Stop the user asked for keeps its place");
    }

    #[test]
    fn a_song_that_ended_rewinds_when_its_stop_lands() {
        let mut transport = Transport::playing();
        transport.begin_stop(true);
        for _ in 0..TRANSPORT_RAMP_FRAMES { transport.advance(); }
        assert!(transport.take_stop(), "a song that ended is over, not paused");
    }

    #[test]
    fn the_fade_is_exact_over_a_length_a_gain_ramp_could_not_express() {
        let mut transport = Transport::playing();
        transport.set_fade_frames(48_000 * 5);
        transport.begin_fade();
        assert_eq!(transport.advance(), TRANSPORT_GAIN_UNITY, "it starts at full scale");
        for _ in 1..48_000 * 5 / 2 { transport.advance(); }
        let halfway = transport.advance();
        assert!((halfway - TRANSPORT_GAIN_UNITY / 2).abs() < 4, "half way through it is at half gain: {halfway}");
        for _ in 0..48_000 * 5 { transport.advance(); }
        assert!(transport.fade_has_landed());
        assert_eq!(transport.advance(), 0, "and it reaches exact silence");
    }

    #[test]
    fn choosing_continue_mid_fade_glides_home_from_the_faded_level() {
        let mut transport = Transport::playing();
        transport.set_fade_frames(1_024);
        transport.begin_fade();
        for _ in 0..512 { transport.advance(); }
        transport.cancel_fade(AtEnd::Continue);
        assert!(!transport.is_fading());
        assert_eq!(transport.target(), TRANSPORT_GAIN_UNITY);
        assert!(!transport.stop_is_pending(), "taking a fade back queues no stop");
    }

    #[test]
    fn a_fade_is_not_cancelled_by_reasserting_fade_out() {
        let mut transport = Transport::playing();
        transport.begin_fade();
        transport.cancel_fade(AtEnd::FadeOut);
        assert!(transport.is_fading());
    }
}
