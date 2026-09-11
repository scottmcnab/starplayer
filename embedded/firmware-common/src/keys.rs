//! The key debounce and hold-repeat state machine — board-independent, so it is
//! host-tested (M8-I5 deliverable 1).
//!
//! [`KeyDebounce`] is fed `(now_ms, samples)` once per poll — the board's `keys.rs`
//! samples six `esp_hal::gpio::Input`s every 10 ms and calls [`KeyDebounce::poll`] — and
//! turns raw, bouncy GPIO levels into [`KeyEvent`]s a control task can act on. It knows
//! nothing about GPIO numbers, `embassy_sync` channels, or which of the six keys the
//! board wires where; that mapping lives in the board crate (`boards/starplayer-a1s`),
//! which is also where the five-key `lcd`-build variant lives (GPIO13/KEY2 is simply
//! never sampled as pressed there — see the board crate's `keys.rs`).
//!
//! # Debounce
//!
//! A raw sample only moves the reported state once **two consecutive polls agree** on
//! the new level (the task file's own debounce rule): a single bouncy sample is held as a
//! *candidate* and only becomes the stable state — and only then raises [`KeyEvent::Press`]
//! or [`KeyEvent::Release`] — when the very next poll repeats it. A sample that returns to
//! the current stable state cancels a pending candidate.
//!
//! # Hold and repeat
//!
//! While a key is held past [`HOLD_THRESHOLD_MS`], [`KeyEvent::Hold`] fires once
//! immediately and then again every [`HOLD_REPEAT_INTERVAL_MS`] for as long as the key
//! stays down — one mechanism for both the seek-repeat and the volume-repeat rows of the
//! task file's key map, which both name the same 200 ms cadence. A key with no hold
//! behaviour (KEY1 and KEY2 in the six-key map) still emits `Hold` events; the mapping
//! layer that turns events into `ControlHalf` calls simply ignores the ones it has no use
//! for.

/// Six physical key slots, KEY1 first — the board's own numbering
/// (`boards/starplayer-a1s/src/board.rs::PIN_KEYS`).
pub const KEY_COUNT: usize = 6;

/// How long a key must be held before [`KeyEvent::Hold`] starts firing.
pub const HOLD_THRESHOLD_MS: u32 = 1_000;

/// How often a held key repeats once past [`HOLD_THRESHOLD_MS`].
pub const HOLD_REPEAT_INTERVAL_MS: u32 = 200;

/// One of the six physical keys, independent of what it is mapped to.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// GPIO36.
    Key1 = 0,
    /// GPIO13 — also the display's MOSI when `lcd` is built; never pressed in that build.
    Key2 = 1,
    /// GPIO19.
    Key3 = 2,
    /// GPIO23.
    Key4 = 3,
    /// GPIO18.
    Key5 = 4,
    /// GPIO5.
    Key6 = 5,
}

impl Key {
    /// Every key, in slot order.
    pub const ALL: [Key; KEY_COUNT] = [Key::Key1, Key::Key2, Key::Key3, Key::Key4, Key::Key5, Key::Key6];

    /// The key's slot index, `0..KEY_COUNT`.
    pub const fn index(self) -> usize { self as usize }

    /// The key at `index`, or `None` past [`KEY_COUNT`].
    pub const fn from_index(index: usize) -> Option<Key> {
        match index {
            0 => Some(Key::Key1),
            1 => Some(Key::Key2),
            2 => Some(Key::Key3),
            3 => Some(Key::Key4),
            4 => Some(Key::Key5),
            5 => Some(Key::Key6),
            _ => None,
        }
    }
}

/// What one poll of a key produced.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    /// The key was just confirmed down.
    Press(Key),
    /// The key has been down for at least [`HOLD_THRESHOLD_MS`]; `hold_ms` is how long at
    /// the moment of this pulse. Repeats every [`HOLD_REPEAT_INTERVAL_MS`] while held.
    Hold(Key, u32),
    /// The key was just confirmed up.
    Release(Key),
}

impl KeyEvent {
    /// The key this event is about.
    pub const fn key(self) -> Key {
        match self {
            KeyEvent::Press(key) | KeyEvent::Hold(key, _) | KeyEvent::Release(key) => key,
        }
    }
}

/// The most events one [`KeyDebounce::poll`] call can produce — one per key.
pub const MAX_EVENTS_PER_POLL: usize = KEY_COUNT;

/// Up to [`MAX_EVENTS_PER_POLL`] events from one poll, oldest first.
///
/// A fixed-capacity array plus a count rather than `heapless::Vec`: this crate keeps its
/// view-model types `Copy` and dependency-free (see `now_playing.rs`'s `FixedStr`), and a
/// per-poll event batch is exactly the same shape of problem.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyEvents {
    events: [Option<KeyEvent>; MAX_EVENTS_PER_POLL],
    count: usize,
}

impl KeyEvents {
    const EMPTY: KeyEvents = KeyEvents { events: [None; MAX_EVENTS_PER_POLL], count: 0 };

    fn push(&mut self, event: KeyEvent) {
        if self.count < self.events.len() {
            self.events[self.count] = Some(event);
            self.count += 1;
        }
    }

    /// The events produced, in order.
    pub fn as_slice(&self) -> &[Option<KeyEvent>] { &self.events[..self.count] }

    /// Iterate the events produced, in order.
    pub fn iter(&self) -> impl Iterator<Item = KeyEvent> + '_ { self.as_slice().iter().filter_map(|event| *event) }
}

#[derive(Copy, Clone, Debug, Default)]
struct KeyState {
    /// The debounced, reported state: `true` means pressed.
    stable: bool,
    /// A raw sample that disagrees with `stable` and is waiting for the next poll to
    /// confirm it.
    candidate: Option<bool>,
    /// When the key was last confirmed pressed, in the same clock `poll` is fed.
    pressed_at_ms: Option<u64>,
    /// When the last `Hold` pulse fired, for pacing the repeat.
    last_hold_ms: Option<u64>,
}

/// The six-key debounce, edge and hold-repeat state machine.
///
/// Fed with the same `(now_ms, samples)` pair every poll; carries no notion of what
/// "now" is beyond a monotonically non-decreasing millisecond counter, so a host test
/// can drive it with a synthetic clock with no board and no `embassy_time`.
#[derive(Copy, Clone, Debug)]
pub struct KeyDebounce {
    keys: [KeyState; KEY_COUNT],
}

impl KeyDebounce {
    /// Every key released, nothing pending.
    pub const fn new() -> KeyDebounce { KeyDebounce { keys: [KeyState { stable: false, candidate: None, pressed_at_ms: None, last_hold_ms: None }; KEY_COUNT] } }

    /// Feed one poll's raw samples — `true` means the pin reads as pressed — and the
    /// monotonic time it was taken at, in milliseconds. Returns the events this poll
    /// produced.
    pub fn poll(&mut self, now_ms: u64, samples: [bool; KEY_COUNT]) -> KeyEvents {
        let mut events = KeyEvents::EMPTY;
        for (index, (state, sample)) in self.keys.iter_mut().zip(samples).enumerate() {
            let Some(key) = Key::from_index(index) else { continue };

            if sample == state.stable {
                // Back where it was: any pending candidate was a bounce, not an edge.
                state.candidate = None;
            } else if state.candidate == Some(sample) {
                // Two consecutive agreeing samples: the edge is confirmed.
                state.candidate = None;
                state.stable = sample;
                if sample {
                    state.pressed_at_ms = Some(now_ms);
                    state.last_hold_ms = None;
                    events.push(KeyEvent::Press(key));
                } else {
                    state.pressed_at_ms = None;
                    state.last_hold_ms = None;
                    events.push(KeyEvent::Release(key));
                }
            } else {
                // The first sample to disagree with the stable state: wait for a repeat.
                state.candidate = Some(sample);
            }

            if state.stable && let Some(pressed_at_ms) = state.pressed_at_ms {
                let held_ms = now_ms.saturating_sub(pressed_at_ms);
                if held_ms >= u64::from(HOLD_THRESHOLD_MS) {
                    let due = match state.last_hold_ms {
                        None => true,
                        Some(last) => now_ms.saturating_sub(last) >= u64::from(HOLD_REPEAT_INTERVAL_MS),
                    };
                    if due {
                        state.last_hold_ms = Some(now_ms);
                        events.push(KeyEvent::Hold(key, held_ms.min(u64::from(u32::MAX)) as u32));
                    }
                }
            }
        }
        events
    }
}

impl Default for KeyDebounce {
    fn default() -> KeyDebounce { KeyDebounce::new() }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::*;

    fn all_released() -> [bool; KEY_COUNT] { [false; KEY_COUNT] }

    fn only(key: Key, pressed: bool, mut samples: [bool; KEY_COUNT]) -> [bool; KEY_COUNT] {
        samples[key.index()] = pressed;
        samples
    }

    #[test]
    fn a_single_bouncy_sample_does_not_move_the_state() {
        let mut debounce = KeyDebounce::new();
        // One poll reads pressed, the very next reads released again: never confirmed.
        let events = debounce.poll(0, only(Key::Key1, true, all_released()));
        assert_eq!(events.iter().count(), 0, "a lone sample is only a candidate");
        let events = debounce.poll(10, all_released());
        assert_eq!(events.iter().count(), 0, "the bounce cancels the candidate rather than confirming it");
    }

    #[test]
    fn two_consecutive_agreeing_samples_confirm_a_press_and_a_release() {
        let mut debounce = KeyDebounce::new();
        assert_eq!(debounce.poll(0, only(Key::Key3, true, all_released())).iter().count(), 0);
        let events = debounce.poll(10, only(Key::Key3, true, all_released()));
        assert_eq!(events.iter().collect::<Vec<_>>(), alloc::vec![KeyEvent::Press(Key::Key3)]);

        assert_eq!(debounce.poll(20, all_released()).iter().count(), 0);
        let events = debounce.poll(30, all_released());
        assert_eq!(events.iter().collect::<Vec<_>>(), alloc::vec![KeyEvent::Release(Key::Key3)]);
    }

    #[test]
    fn holding_past_the_threshold_fires_once_and_then_repeats_every_interval() {
        let mut debounce = KeyDebounce::new();
        debounce.poll(0, only(Key::Key5, true, all_released()));
        debounce.poll(10, only(Key::Key5, true, all_released())); // Press confirmed at t=10.

        // Nothing between the press and the threshold.
        for t in (20..1_000).step_by(50) {
            let events = debounce.poll(t, only(Key::Key5, true, all_released()));
            assert!(events.iter().all(|event| !matches!(event, KeyEvent::Hold(_, _))), "no hold before {HOLD_THRESHOLD_MS} ms (t={t})");
        }

        // The threshold itself (measured from the press at t=10) fires the first pulse.
        let held_at_threshold = 10 + u64::from(HOLD_THRESHOLD_MS);
        let events = debounce.poll(held_at_threshold, only(Key::Key5, true, all_released()));
        assert!(events.iter().any(|event| matches!(event, KeyEvent::Hold(Key::Key5, ms) if ms == HOLD_THRESHOLD_MS)));

        // The next pulse is exactly one interval later, not before.
        let too_soon = held_at_threshold + u64::from(HOLD_REPEAT_INTERVAL_MS) - 1;
        let events = debounce.poll(too_soon, only(Key::Key5, true, all_released()));
        assert!(events.iter().all(|event| !matches!(event, KeyEvent::Hold(_, _))));

        let next_pulse = held_at_threshold + u64::from(HOLD_REPEAT_INTERVAL_MS);
        let events = debounce.poll(next_pulse, only(Key::Key5, true, all_released()));
        assert!(events.iter().any(|event| matches!(event, KeyEvent::Hold(Key::Key5, _))));
    }

    #[test]
    fn releasing_stops_the_repeat_and_a_fresh_press_restarts_the_threshold() {
        let mut debounce = KeyDebounce::new();
        debounce.poll(0, only(Key::Key4, true, all_released()));
        debounce.poll(10, only(Key::Key4, true, all_released()));
        debounce.poll(10 + u64::from(HOLD_THRESHOLD_MS), only(Key::Key4, true, all_released()));

        debounce.poll(2_000, all_released());
        let events = debounce.poll(2_010, all_released());
        assert!(events.iter().any(|event| event == KeyEvent::Release(Key::Key4)));

        // A fresh press must wait out the full threshold again, not resume mid-hold.
        debounce.poll(2_020, only(Key::Key4, true, all_released()));
        debounce.poll(2_030, only(Key::Key4, true, all_released()));
        let events = debounce.poll(2_030 + u64::from(HOLD_THRESHOLD_MS) - 1, only(Key::Key4, true, all_released()));
        assert!(events.iter().all(|event| !matches!(event, KeyEvent::Hold(_, _))));
    }

    #[test]
    fn six_keys_debounce_independently_in_the_same_poll() {
        let mut debounce = KeyDebounce::new();
        let mut samples = all_released();
        samples[Key::Key1.index()] = true;
        samples[Key::Key6.index()] = true;
        debounce.poll(0, samples);
        let events = debounce.poll(10, samples);
        let pressed: Vec<KeyEvent> = events.iter().collect();
        assert!(pressed.contains(&KeyEvent::Press(Key::Key1)));
        assert!(pressed.contains(&KeyEvent::Press(Key::Key6)));
        assert_eq!(pressed.len(), 2, "the other four keys stayed released and raised nothing");
    }

    #[test]
    fn a_key_slot_that_never_samples_pressed_never_events_the_five_key_lcd_build() {
        // Mirrors the board crate's `lcd` build, which never sets samples[Key2] true.
        let mut debounce = KeyDebounce::new();
        for t in (0..5_000).step_by(10) {
            let events = debounce.poll(t, all_released());
            assert_eq!(events.iter().count(), 0);
        }
    }
}
