//! Six push-buttons: `esp_hal::gpio::Input`s sampled every 10 ms and fed into
//! [`firmware_common::keys::KeyDebounce`] — the host-tested debounce, edge and
//! hold-repeat state machine. Always built (M8-I5 deliverable 1).
//!
//! The `lcd` build never samples KEY2/GPIO13 as pressed, because GPIO13 is the display's
//! MOSI there (`board.rs`'s pin table): [`Keys`] simply has no `Input` in that slot, so
//! [`Keys::sample`] reports it permanently released and [`firmware_common::keys`] never
//! emits a `KeyEvent` for it. `main.rs`'s key-to-command mapping is what actually
//! implements the five-key variant's "KEY1 long-press = stop" (the task file's key map),
//! by giving KEY1's `Hold` event a meaning `lcd` builds only.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::gpio::{Input, InputConfig, Pull};
#[cfg(not(feature = "lcd"))]
use esp_hal::peripherals::GPIO13;
use esp_hal::peripherals::{GPIO5, GPIO18, GPIO19, GPIO23, GPIO36};
use firmware_common::keys::KEY_COUNT;
use firmware_common::{KeyDebounce, KeyEvent};

/// How often the six keys are sampled.
///
/// 10 ms, the task file's own figure for "musical controls" (ampkeeper's
/// `BUTTON_POLL_INTERVAL` is 100 ms for a hold-tracking factory-reset gesture; this board
/// wants the [`firmware_common::keys::HOLD_REPEAT_INTERVAL_MS`] 200 ms seek/volume repeat
/// to feel like twenty samples, not two).
pub const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// How many [`KeyEvent`]s the channel between [`keys_task`] and the control task holds.
/// Six keys, all pressed in the same 10 ms poll, is the worst case in one message; this
/// is deliberately a few polls deep so a control task that is briefly busy (a display
/// redraw) does not lose an edge.
pub const KEY_EVENT_CHANNEL_CAPACITY: usize = 8;

/// The channel type [`keys_task`] publishes onto and the control task drains — a type
/// alias because `#[embassy_executor::task]` functions must be monomorphic, so the
/// capacity has to be a concrete constant rather than a generic parameter.
pub type KeyEventChannel = Channel<CriticalSectionRawMutex, KeyEvent, KEY_EVENT_CHANNEL_CAPACITY>;
/// The sending half `main.rs` hands to [`keys_task`].
pub type KeyEventSender = Sender<'static, CriticalSectionRawMutex, KeyEvent, KEY_EVENT_CHANNEL_CAPACITY>;
/// The receiving half the control task drains every poll.
pub type KeyEventReceiver = Receiver<'static, CriticalSectionRawMutex, KeyEvent, KEY_EVENT_CHANNEL_CAPACITY>;

/// The six push-buttons as GPIO inputs. `KEY2`'s slot is `None` in the `lcd` build.
pub struct Keys {
    inputs: [Option<Input<'static>>; KEY_COUNT],
}

impl Keys {
    /// Claim KEY1, KEY3, KEY4, KEY5 and KEY6 — every key except KEY2/GPIO13, which the
    /// `lcd` build's display owns as MOSI instead.
    #[cfg(feature = "lcd")]
    pub fn take(key1: GPIO36<'static>, key3: GPIO19<'static>, key4: GPIO23<'static>, key5: GPIO18<'static>, key6: GPIO5<'static>) -> Keys {
        let config = InputConfig::default().with_pull(Pull::Up);
        Keys {
            inputs: [
                Some(Input::new(key1, config)),
                None,
                Some(Input::new(key3, config)),
                Some(Input::new(key4, config)),
                Some(Input::new(key5, config)),
                Some(Input::new(key6, config)),
            ],
        }
    }

    /// Claim all six keys.
    #[cfg(not(feature = "lcd"))]
    pub fn take(
        key1: GPIO36<'static>, key2: GPIO13<'static>, key3: GPIO19<'static>, key4: GPIO23<'static>, key5: GPIO18<'static>,
        key6: GPIO5<'static>,
    ) -> Keys {
        let config = InputConfig::default().with_pull(Pull::Up);
        Keys {
            inputs: [
                Some(Input::new(key1, config)),
                Some(Input::new(key2, config)),
                Some(Input::new(key3, config)),
                Some(Input::new(key4, config)),
                Some(Input::new(key5, config)),
                Some(Input::new(key6, config)),
            ],
        }
    }

    /// Sample every key. `true` means the pin currently reads pressed — active low, the
    /// same convention `board.rs` uses for the headphone-detect pin: each button pulls
    /// its GPIO to ground when held, with a pull-up (internal where the pin supports one,
    /// external on the board otherwise — GPIO36/KEY1 is input-only with no internal pull,
    /// M8-I5 research point 4; `InputConfig::with_pull` costs nothing and is ignored
    /// where the silicon cannot honour it, the same reasoning `board.rs`'s
    /// `headphone_detect` already relies on).
    fn sample(&self) -> [bool; KEY_COUNT] { core::array::from_fn(|index| self.inputs[index].as_ref().is_some_and(Input::is_low)) }

    /// Whether one key reads pressed right now.
    ///
    /// For the boot-time gestures that run before [`keys_task`] exists — M8-I6's
    /// re-provision hold is the only one. A key that this build does not claim (KEY2
    /// under `lcd`, whose GPIO is the display's MOSI) always reads released.
    #[cfg(feature = "web")]
    pub fn is_pressed(&self, key: firmware_common::Key) -> bool {
        self.inputs[key as usize].as_ref().is_some_and(Input::is_low)
    }
}

/// Wait up to `hold` for `key` to be held down continuously, starting now.
///
/// Returns `false` immediately when the key is not already down, so a boot that nobody is
/// touching costs one GPIO read rather than five seconds. Used for M8-I6's re-provision
/// gesture, which is checked once, before the keys task starts.
#[cfg(feature = "web")]
pub async fn held_at_boot(keys: &Keys, key: firmware_common::Key, hold: Duration) -> bool {
    if !keys.is_pressed(key) {
        return false;
    }
    let deadline = Instant::now() + hold;
    while Instant::now() < deadline {
        Timer::after(POLL_INTERVAL).await;
        if !keys.is_pressed(key) {
            return false;
        }
    }
    true
}

/// Poll the six keys every [`POLL_INTERVAL`] and publish [`KeyEvent`]s onto `sender`.
///
/// All the debounce, edge and hold-repeat logic lives in
/// [`firmware_common::keys::KeyDebounce`], which is host-tested; this task is only the
/// GPIO sampling and the real clock that feeds it.
#[embassy_executor::task]
pub async fn keys_task(keys: Keys, sender: KeyEventSender) {
    let mut debounce = KeyDebounce::new();
    let start = Instant::now();
    loop {
        Timer::after(POLL_INTERVAL).await;
        let now_ms = Instant::now().duration_since(start).as_millis();
        for event in debounce.poll(now_ms, keys.sample()).iter() {
            // A full channel drops the event rather than blocking this 10 ms poll loop on
            // a control task that has fallen behind — the same lossy-under-pressure trade
            // the render half's own command ring makes.
            let _ = sender.try_send(event);
        }
    }
}
