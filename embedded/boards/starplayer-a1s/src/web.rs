//! The web control surface: a page, a JSON API, a WebSocket and module upload.
//!
//! # Who owns what
//!
//! The audio refill owns core 1 and touches nothing here. On core 0 the **control task**
//! (`main.rs`) is the sole owner of [`ControlHalf`], as it has been since M8-I5 — the web
//! workers never hold it. Everything crosses between them through three things in this
//! module, and only through them:
//!
//! * [`COMMANDS`] — a bounded channel of fire-and-forget transport commands (play, seek,
//!   volume, mute). A full channel drops the command rather than blocking a worker.
//! * [`Job`] — the four operations that have to answer the browser: load an upload,
//!   select a stored module, store the playing one, forget the WiFi credentials. One at a
//!   time, behind [`JOB_LOCK`], with the answer coming back on [`JOB_DONE`].
//! * [`STATUS`] and [`TELEMETRY`] — what the control task publishes for the workers to
//!   read, so a `GET /api/status` costs a memcpy rather than a round trip.
//!
//! [`Bridge`] is the control-task half: it owns the flash [`Store`], the two PSRAM image
//! buffers, and the answer to every [`Job`]. The control task calls [`Bridge::poll`] once
//! a tick and [`Bridge::publish`] at the display's own cadence.
//!
//! # Routing is flat, and that is not a style choice
//!
//! picoserve's `Router` is a cons list: every `.route()` wraps the previous router as its
//! fallback, so matching a path descends one monomorphised async frame *per registered
//! route*, the first-registered being the deepest. ampkeeper measured 7–12 KB of stack per
//! frame and had `GET /` descend past 38 KB with thirteen routes — straight through the
//! main stack into `.bss`, which on this chip is the WiFi driver's own statics. A flat
//! `match` on `(method, path)` in one [`PathRouterService`] costs one frame whatever the
//! endpoint count, and this firmware's main stack is smaller than ampkeeper's was.
//!
//! # The upload path, and why it is shaped like this
//!
//! `starplayer::load` decodes a module into the **heap**, which on this board is internal
//! DRAM and nothing else (see `psram.rs` for the atomics erratum that keeps PSRAM out of
//! the allocator). A decoded `PETRI.S3M` is 64 KB of PCM alone — more than the web
//! build's whole DRAM heap. So an uploaded module is not *kept* the way `load` produced
//! it:
//!
//! 1. the request body streams into the **PSRAM staging buffer** — the raw file never
//!    touches DRAM;
//! 2. if it is already a module image (`SPMI` magic — what `cargo xtask module-image`
//!    writes), it is copied straight into the free PSRAM image buffer;
//! 3. otherwise `starplayer::load` decodes it in DRAM, `Module::to_image` serialises the
//!    result, that image is copied into the free PSRAM image buffer, and both DRAM
//!    allocations are dropped;
//! 4. `Module::from_image` over the PSRAM image borrows the pattern blob and the PCM
//!    **in place**, so what stays in DRAM is the `Arc`, four small index vectors and the
//!    sequencer.
//!
//! Step 3 is the ceiling: its peak DRAM is the decoded module plus its image, both at
//! once, so a raw upload is limited by the heap even though the result is not. A
//! `.spmi` upload skips it entirely and is limited only by PSRAM, which is why the page
//! accepts both and why `cargo xtask module-image` is the answer for a large module.


use core::fmt::Write as _;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, with_timeout};
use esp_println::println;
use firmware_common::api::{self, HostState, Upload};
use firmware_common::{FixedStr, NowPlaying};
use picoserve::io::{Read, Write};
use picoserve::request::{Path, Request};
use picoserve::response::ws::{Message, SocketRx, SocketTx, WebSocketCallback, WebSocketUpgrade};
use picoserve::ResponseSent;
use picoserve::response::{Content, IntoResponse, ResponseWriter, StatusCode};
use picoserve::routing::{PathRouterService, RequestHandlerService};
use starplayer::core::{ChannelId, U0F16};
use starplayer::model::Module;
use starplayer::model::image::IMAGE_MAGIC;
use starplayer::rt::Arc;
use starplayer_host_embedded::ControlHalf;
use static_cell::StaticCell;

use crate::psram;
use crate::store::{SLOT_IMAGE_MAX_BYTES, SLOT_NAME_BYTES, Store};

/// How many connections the server handles at once.
///
/// Two, not four. Each worker's future is a `.bss` task-pool slot with its own TCP
/// buffers inside it, and on this chip every `.bss` byte is a byte the main stack does not
/// get (see `main.rs`'s `HEAP_BYTES`). Two is also research point 5's cap on WebSocket
/// clients, and it falls out of this rather than needing a counter of its own.
pub const WEB_TASK_POOL_SIZE: usize = 2;

/// TCP receive and transmit buffers, per worker.
const TCP_BUFFER_BYTES: usize = 1024;
/// picoserve's own request buffer, per worker: large enough for any header block a
/// browser sends.
const HTTP_BUFFER_BYTES: usize = 1024;
/// The largest JSON body this API answers with — the status, with sixteen channel rows.
///
/// Every byte here is counted twice in a worker's future (the scratch the encoder writes
/// into and the [`Body`] it is copied to), and a worker's future is `.bss`, so this is
/// sized to the largest real answer rather than rounded up: sixteen channel rows of
/// roughly seventy bytes plus a header of about two hundred.
const BODY_BYTES: usize = 1408;

/// The largest request body the upload endpoint accepts, which is the size of the PSRAM
/// staging buffer behind it.
pub const UPLOAD_MAX_BYTES: usize = psram::BUFFER_BYTES;

/// Free DRAM the control task insists on before it decodes a raw upload, over and above
/// three times the file's own size.
///
/// A stated estimate rather than a measurement: `starplayer::load` allocates the decoded
/// PCM (up to twice the file for 8-bit samples, plus pre-roll and guard frames), the
/// pattern blob and four index vectors, and `Module::to_image` then allocates all of that
/// again as one contiguous image. Refusing early is the difference between "that module
/// is too big for this board" and a heap-exhaustion panic, which on a device is a reset.
const UPLOAD_DRAM_MARGIN: usize = 24 * 1024;

/// How long a worker waits for the control task to answer a [`Job`].
const JOB_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the WebSocket pushes a telemetry frame: 10 Hz, the task file's figure.
const TELEMETRY_INTERVAL: Duration = Duration::from_millis(100);

/// How long a WebSocket write may take before the connection is given up on.
const WEBSOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// The gzipped page, compiled in. `cargo xtask assets` writes it into `embedded/assets`,
/// a git-ignored build directory, beside the module images.
static INDEX_HTML_GZ: &[u8] = include_bytes!("../../../assets/index.html.gz");

/// Response headers for the compiled-in page.
const GZIP_HEADERS: &[(&str, &str)] = &[("Content-Encoding", "gzip"), ("Cache-Control", "no-cache")];

// ---------------------------------------------------------------------------
// What crosses between the web workers and the control task
// ---------------------------------------------------------------------------

/// A transport command that needs no answer.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Play,
    Stop,
    SeekOrder(u16),
    SeekRow(u16),
    SeekFrame(u64),
    Volume(U0F16),
    Mute { channel: u16, muted: bool },
    /// One order forward or back from wherever the transport is *now*, which only the
    /// control task knows.
    Skip(i32),
    AtEnd(starplayer::core::AtEnd),
}

/// Capacity of the command channel. A WebSocket volume drag is the fastest producer, and
/// eight is several of the control task's 10 ms ticks' worth.
const COMMAND_CAPACITY: usize = 8;

static COMMANDS: Channel<CriticalSectionRawMutex, Command, COMMAND_CAPACITY> = Channel::new();

/// Queue a transport command. `false` means the channel was full and the command was
/// dropped — the same lossy-under-pressure trade the render half's own command ring makes.
pub fn send_command(command: Command) -> bool { COMMANDS.try_send(command).is_ok() }

/// An operation the browser is waiting on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// Decode (or borrow) these bytes and swap the result in. The slice points into the
    /// PSRAM staging buffer and stays valid because the caller holds [`STAGING`] until
    /// the answer comes back.
    LoadUpload(&'static [u8]),
    /// Play the compiled-in image (`0`) or a stored slot (`1..=slot_count`).
    Select(u8),
    /// Write the playing module's image into a slot. Pauses playback.
    Store(u8),
    /// Forget the WiFi credentials, so the next boot is the captive portal.
    ForgetWifi,
    /// Store the credentials waiting in [`PENDING_WIFI`]. The portal personality's
    /// `POST /save` is the only caller.
    SaveWifi,
}

/// What a [`Job`] came back with. The failure message goes into the HTTP response body,
/// so it is written for a person.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum JobOutcome {
    Done,
    Failed(&'static str),
}

/// The credentials [`Job::SaveWifi`] is to write. A separate slot rather than a payload
/// on the job, because a `Signal` carries `Copy` values and `WifiCredentials` is two
/// `heapless::String`s; [`JOB_LOCK`] is what keeps one save from overwriting another.
static PENDING_WIFI: Mutex<CriticalSectionRawMutex, Option<crate::store::WifiCredentials>> = Mutex::new(None);

static JOB_LOCK: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());
static JOB_REQUEST: Signal<CriticalSectionRawMutex, Job> = Signal::new();
static JOB_DONE: Signal<CriticalSectionRawMutex, JobOutcome> = Signal::new();

/// Hand `job` to the control task and wait for its answer.
async fn run_job(job: Job) -> JobOutcome {
    let _guard = JOB_LOCK.lock().await;
    JOB_DONE.reset();
    JOB_REQUEST.signal(job);
    match with_timeout(JOB_TIMEOUT, JOB_DONE.wait()).await {
        Ok(outcome) => outcome,
        Err(_) => JobOutcome::Failed("the control task did not answer in time"),
    }
}

/// Ask the control task to store `credentials` in the `config` partition, and arm the
/// reset that follows. Called by the captive portal's `POST /save`.
pub async fn request_save_wifi(credentials: crate::store::WifiCredentials) -> JobOutcome {
    *PENDING_WIFI.lock().await = Some(credentials);
    let outcome = run_job(Job::SaveWifi).await;
    if outcome == JobOutcome::Done {
        REBOOT_REQUESTED.signal(());
    }
    outcome
}

/// The upload staging buffer and the state machine that guards it.
///
/// Locked for the whole of a `POST /api/modules`, from the first body byte to the control
/// task's answer: that lock is what makes a second concurrent upload a 409 rather than
/// two writers in one buffer.
struct Staging {
    buffer: psram::Buffer,
    upload: Upload,
}

static STAGING: Mutex<CriticalSectionRawMutex, Option<Staging>> = Mutex::new(None);

/// What the control task publishes for the workers to render. `None` until the control
/// task's first tick.
struct Published {
    view: NowPlaying,
    host: HostState,
    format: &'static str,
    source: FixedStr<16>,
    module_id: u8,
}

static STATUS: Mutex<CriticalSectionRawMutex, Option<Published>> = Mutex::new(None);

/// The packed telemetry block, refreshed by the control task and sent verbatim by the
/// WebSocket. One copy, shared: a per-worker copy would be two more kilobytes of `.bss`,
/// and the send holds the lock rather than copying out of it.
struct Telemetry {
    bytes: [u8; api::TELEMETRY_MAX_BYTES],
    length: usize,
}

static TELEMETRY: Mutex<CriticalSectionRawMutex, Telemetry> =
    Mutex::new(Telemetry { bytes: [0; api::TELEMETRY_MAX_BYTES], length: 0 });

/// The compiled-in image plus every slot the partition holds.
const MODULE_LIST_CAPACITY: usize = 8;

/// One entry of the module list.
#[derive(Copy, Clone, Debug)]
struct SlotEntry {
    id: u8,
    name: FixedStr<SLOT_NAME_BYTES>,
    bytes: u32,
}

/// The module list, refreshed whenever a slot is written.
///
/// A cached list rather than a flash read per request: reading the `modules` partition is
/// a flash read, and a flash read from a web worker while the audio refill is running is
/// exactly the contention this firmware is trying not to create.
static MODULES: Mutex<CriticalSectionRawMutex, heapless::Vec<SlotEntry, MODULE_LIST_CAPACITY>> = Mutex::new(heapless::Vec::new());

/// Bumped every time a different module is swapped in, so the page can tell "the same
/// song, later" from "a different song".
static GENERATION: AtomicU32 = AtomicU32::new(1);

/// Set by `POST /api/reprovision` once the credentials are gone; `main`'s watcher reboots
/// a moment later, so the browser sees its answer first.
static REBOOT_REQUESTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Wait for a reboot request. `main` spawns this beside the web tasks.
pub async fn wait_for_reboot() { REBOOT_REQUESTED.wait().await }

/// Everything `main` builds for the web before the audio starts, handed to `play` as one
/// value so that the boot sequence's argument list stays readable.
pub struct Boot {
    pub bridge: Bridge,
    /// The stored credentials, or `None` when the device has never been provisioned.
    pub credentials: Option<crate::store::WifiCredentials>,
    pub wifi: esp_hal::peripherals::WIFI<'static>,
    /// The random seed embassy-net wants for its TCP initial sequence numbers.
    pub seed: u64,
}

// ---------------------------------------------------------------------------
// The control-task half
// ---------------------------------------------------------------------------

/// Everything the control task needs to answer the web: the flash, the two PSRAM image
/// buffers, and which of them the playing module is borrowing.
pub struct Bridge {
    store: Store,
    /// Ping-pong. `images[live]` is what the playing module borrows; the other is free
    /// for the next upload or slot read. `None` on a board with no PSRAM fitted, where
    /// the compiled-in module is the only one that can play.
    images: Option<[psram::Buffer; 2]>,
    live: usize,
    /// The image the playing module is borrowing, or `None` when what is playing is the
    /// compiled-in flash image and neither buffer is in use.
    live_image: Option<&'static [u8]>,
    source: FixedStr<16>,
    module_id: u8,
    format: &'static str,
}

impl Bridge {
    /// Claim the flash and three PSRAM buffers — staging plus the two image halves.
    ///
    /// A board with no PSRAM fitted still gets a `Bridge`: the page, the transport, the
    /// stored-module list and — this is the one that matters — the captive portal's
    /// `POST /save` all go through here, and a bridge that refused to exist would leave a
    /// device that cannot be provisioned at all. What it loses is upload and slot
    /// playback, which answer with a sentence saying so.
    pub fn new(store: Store, arena: &mut psram::Arena, format: &'static str) -> Bridge {
        let images = match (arena.claim(UPLOAD_MAX_BYTES), arena.claim(psram::BUFFER_BYTES), arena.claim(psram::BUFFER_BYTES)) {
            (Some(staging), Some(first), Some(second)) => {
                // `try_lock`, not `lock`: this runs before any executor polls, so an
                // await here would never complete.
                if let Ok(mut guard) = STAGING.try_lock() {
                    *guard = Some(Staging { buffer: staging, upload: Upload::new() });
                }
                Some([first, second])
            }
            _ => None,
        };
        let mut bridge = Bridge { store, images, live: 0, live_image: None, source: FixedStr::new("flash"), module_id: 0, format };
        bridge.refresh_modules();
        bridge
    }

    /// Whether this board can hold an uploaded or stored module at all.
    pub fn has_psram(&self) -> bool { self.images.is_some() }

    /// Re-read every slot header into [`MODULES`].
    fn refresh_modules(&mut self) {
        let mut entries: heapless::Vec<SlotEntry, MODULE_LIST_CAPACITY> = heapless::Vec::new();
        let _ = entries.push(SlotEntry { id: 0, name: FixedStr::new("PETRI"), bytes: crate::images::petri_s3m().len() as u32 });
        let last = self.store.slot_count().min(MODULE_LIST_CAPACITY as u8 - 1);
        for id in 1..=last {
            if let Ok(Some(header)) = self.store.slot_header(id) {
                let _ = entries.push(SlotEntry { id, name: header.name, bytes: header.bytes });
            }
        }
        if let Ok(mut guard) = MODULES.try_lock() {
            *guard = entries;
        }
    }

    /// Drain the command channel and answer at most one [`Job`]. Called once per control
    /// tick.
    pub async fn poll(&mut self, control: &mut ControlHalf) {
        while let Ok(command) = COMMANDS.try_receive() {
            apply_command(control, command);
        }
        if let Some(job) = JOB_REQUEST.try_take() {
            let outcome = self.run(control, job).await;
            JOB_DONE.signal(outcome);
        }
    }

    /// Publish what the workers render: the view model, the host's own state and the
    /// packed telemetry block.
    pub fn publish(&mut self, control: &mut ControlHalf, view: &NowPlaying) {
        let snapshot = *control.telemetry();
        let host = HostState {
            playing: control.is_playing(),
            fading: control.is_fading(),
            peak: control.peak(),
            output_frame: control.output_frame().0,
            pending_garbage: control.pending_garbage() as u16,
            module_generation: GENERATION.load(Ordering::Relaxed),
            retired_collected: 0,
            master_volume: control.master_volume(),
        };
        // `try_lock` throughout: telemetry is lossy by design, and the control task must
        // never wait behind a worker that is mid-send on a slow connection.
        if let Ok(mut status) = STATUS.try_lock() {
            *status = Some(Published { view: *view, host, format: self.format, source: self.source, module_id: self.module_id });
        }
        if let Ok(mut telemetry) = TELEMETRY.try_lock() {
            let Telemetry { bytes, length } = &mut *telemetry;
            *length = api::pack_telemetry(&snapshot, &host, bytes).unwrap_or(0);
        }
    }

    /// Answer one job.
    async fn run(&mut self, control: &mut ControlHalf, job: Job) -> JobOutcome {
        match job {
            Job::LoadUpload(body) => self.load_upload(control, body).await,
            Job::Select(id) => self.select(control, id).await,
            Job::Store(id) => self.store_current(control, id).await,
            Job::ForgetWifi => self.forget_wifi(control).await,
            Job::SaveWifi => self.save_wifi(control).await,
        }
    }

    /// The free image buffer — the one the playing module is not borrowing.
    fn free_image(&self) -> usize { if self.live_image.is_some() { 1 - self.live } else { self.live } }

    /// Make sure the module that was borrowing the buffer about to be overwritten has
    /// actually been dropped.
    ///
    /// A module built by `Module::from_image` borrows its pattern blob and its PCM out of
    /// a PSRAM half, and the outgoing one does not stop reading them the instant
    /// `ControlHalf::load` is called: the render half swaps on its next quantum and sends
    /// the old `Arc` back over the garbage channel, and
    /// [`ControlHalf::collect_garbage`](starplayer_host_embedded::ControlHalf::collect_garbage)
    /// is what finally drops it. In practice the control task's once-a-second collection
    /// has long since run by the time a second upload arrives — but "in practice" is not
    /// a guarantee, and the failure mode is a mixer reading PCM out from under itself, so
    /// this drains the channel explicitly and waits for it to stay empty.
    ///
    /// The bound is what makes this honest rather than a spin: ten tries at 10 ms is
    /// three hundred render quanta, and if something has still not come back the caller
    /// proceeds anyway, because the alternative is an endpoint that never answers. The
    /// only way that can happen is a render half that has stopped running, in which case
    /// nothing is reading the buffer either.
    async fn wait_for_retirement(&mut self, control: &mut ControlHalf) {
        for _ in 0..10 {
            let collected = control.collect_garbage();
            if collected == 0 && control.pending_garbage() == 0 {
                return;
            }
            Timer::after(Duration::from_millis(10)).await;
        }
    }

    /// One half of the ping-pong. Only reached once `images` is known to be `Some`.
    fn image_half(&mut self, slot: usize) -> &mut psram::Buffer {
        &mut self.images.as_mut().expect("the callers check `images` before claiming a half")[slot]
    }

    /// Build a module from an image already sitting in PSRAM half `slot`, and swap it in.
    fn adopt(&mut self, control: &mut ControlHalf, slot: usize, image: &'static [u8], source: &str, id: u8) -> JobOutcome {
        let module = match Module::from_image(image) {
            Ok(module) => module,
            Err(_) => return JobOutcome::Failed("that image would not borrow — regenerate it with cargo xtask module-image"),
        };
        self.format = format_name(module.header().format);
        if control.load(Arc::new(module)).is_err() {
            return JobOutcome::Failed("the command ring is full; try again in a moment");
        }
        self.live = slot;
        self.live_image = Some(image);
        self.module_id = id;
        self.source = FixedStr::new(source);
        GENERATION.fetch_add(1, Ordering::Relaxed);
        let _ = control.play();
        JobOutcome::Done
    }

    /// `POST /api/modules`: whatever the browser sent is in PSRAM; decide what it is.
    async fn load_upload(&mut self, control: &mut ControlHalf, body: &'static [u8]) -> JobOutcome {
        if self.images.is_none() {
            return JobOutcome::Failed("this board has no PSRAM, so it can only play the compiled-in module");
        }
        let slot = self.free_image();
        self.wait_for_retirement(control).await;

        if body.starts_with(&IMAGE_MAGIC) {
            // Already an image: PSRAM to PSRAM, no decode, no DRAM, any size that fits.
            // SAFETY: `slot` is the half the playing module is *not* borrowing
            // (`free_image`), so nothing reads what this overwrites, and `Bridge` is the
            // only owner of either half.
            let Some(image) = (unsafe { self.image_half(slot).fill(body) }) else {
                return JobOutcome::Failed("that image is larger than the PSRAM buffer");
            };
            return self.adopt(control, slot, image, "upload", 0);
        }

        // A raw module file: the DRAM-bounded path. See the module documentation.
        let needed = body.len().saturating_mul(3).saturating_add(UPLOAD_DRAM_MARGIN);
        if esp_alloc::HEAP.free() < needed {
            return JobOutcome::Failed("not enough free RAM to decode a module that size — upload a .spmi image instead");
        }
        let module = match starplayer::load(body) {
            Ok(module) => module,
            Err(_) => return JobOutcome::Failed("that is not a module this build can play"),
        };
        let image = module.to_image();
        drop(module);
        // SAFETY: as above — `slot` is the half nothing is borrowing.
        let borrowed = unsafe { self.image_half(slot).fill(&image) };
        drop(image);
        match borrowed {
            Some(borrowed) => self.adopt(control, slot, borrowed, "upload", 0),
            None => JobOutcome::Failed("the decoded module is larger than the PSRAM buffer"),
        }
    }

    /// `POST /api/modules/select`: the compiled-in image, or one read out of a slot.
    async fn select(&mut self, control: &mut ControlHalf, id: u8) -> JobOutcome {
        if id == 0 {
            let module = match Module::from_image(crate::images::petri_s3m()) {
                Ok(module) => module,
                Err(_) => return JobOutcome::Failed("the compiled-in image would not borrow"),
            };
            self.format = format_name(module.header().format);
            if control.load(Arc::new(module)).is_err() {
                return JobOutcome::Failed("the command ring is full; try again in a moment");
            }
            self.live_image = None;
            self.module_id = 0;
            self.source = FixedStr::new("flash");
            GENERATION.fetch_add(1, Ordering::Relaxed);
            let _ = control.play();
            return JobOutcome::Done;
        }

        if self.images.is_none() {
            return JobOutcome::Failed("this board has no PSRAM to read a stored module into");
        }
        let slot = self.free_image();
        // A long flash read contends for the flash bus with core 1's own instruction
        // fetches, so the transport stops around it — for audio quality, not for safety
        // (see `store.rs`).
        self.pause_playback(control).await;
        self.wait_for_retirement(control).await;
        // SAFETY: `slot` is the half the playing module is not borrowing, and
        // `wait_for_retirement` has dropped whatever module was.
        let destination = unsafe { self.image_half(slot).as_mut() };
        let read = self.store.read_slot(id, destination);
        let length = match read {
            Ok(length) => length,
            Err(_) => {
                let _ = control.play();
                return JobOutcome::Failed("that slot is empty or would not read");
            }
        };
        // SAFETY: the same half, and `length` of its bytes were just written by the read.
        let Some(image) = (unsafe { self.image_half(slot).view(length) }) else {
            let _ = control.play();
            return JobOutcome::Failed("the slot image is larger than the PSRAM buffer");
        };
        let label = slot_label(id);
        self.adopt(control, slot, image, label.as_str(), id)
    }

    /// `POST /api/modules/store`: write the playing module's image into a slot.
    ///
    /// Only a module that *has* an image can be stored — an uploaded one, or one already
    /// read out of another slot. The compiled-in module is in flash already and answers
    /// with a refusal rather than copying itself.
    async fn store_current(&mut self, control: &mut ControlHalf, id: u8) -> JobOutcome {
        let Some(image) = self.live_image else {
            return JobOutcome::Failed("the module playing is the compiled-in one; upload a module first");
        };
        if image.len() as u32 > SLOT_IMAGE_MAX_BYTES {
            return JobOutcome::Failed("that module is larger than a flash slot");
        }
        let title = control.module().map(|module| module.header().title.as_ref()).unwrap_or("module");
        let title = FixedStr::<SLOT_NAME_BYTES>::new(title);

        self.pause_playback(control).await;
        let written = self.store.write_slot(id, title.as_str(), image);
        let _ = control.play();
        match written {
            Ok(()) => {
                self.refresh_modules();
                JobOutcome::Done
            }
            Err(_) => JobOutcome::Failed("the flash write failed"),
        }
    }

    /// Forget the stored credentials. `main`'s reboot watcher restarts the board a moment
    /// later, and the next boot is the captive portal.
    async fn forget_wifi(&mut self, control: &mut ControlHalf) -> JobOutcome {
        self.pause_playback(control).await;
        let erased = self.store.erase_wifi().await;
        let _ = control.play();
        match erased {
            Ok(()) => {
                REBOOT_REQUESTED.signal(());
                JobOutcome::Done
            }
            Err(_) => JobOutcome::Failed("the credentials could not be erased"),
        }
    }

    /// Write the credentials the portal left in [`PENDING_WIFI`].
    async fn save_wifi(&mut self, control: &mut ControlHalf) -> JobOutcome {
        let Some(credentials) = PENDING_WIFI.lock().await.take() else {
            return JobOutcome::Failed("no credentials were waiting to be saved");
        };
        self.pause_playback(control).await;
        let saved = self.store.save_wifi(&credentials).await;
        let _ = control.play();
        match saved {
            Ok(()) => JobOutcome::Done,
            Err(_) => JobOutcome::Failed("the credentials could not be written to flash"),
        }
    }

    /// Stop the transport and let the ramp and the DMA ring drain to silence before a
    /// flash operation.
    ///
    /// This is the flash-cache pause. esp-storage parks core 1 around each erase and each
    /// write chunk, so the refill is not running and the DMA ring plays whatever it holds
    /// — which after this pause is silence. Without it, a 90 KB slot write is several
    /// seconds of a loudly repeated 23 ms fragment rather than a gap. 150 ms covers the
    /// transport's 64-frame ramp and the ring's own 23 ms depth several times over.
    async fn pause_playback(&mut self, control: &mut ControlHalf) {
        let _ = control.stop();
        Timer::after(Duration::from_millis(150)).await;
    }
}

/// A module format's name, for the status JSON.
///
/// `ModuleFormat` has no `name()` of its own — the engine has never needed one, because
/// `Debug` serves every caller in the workspace — and a `&'static str` is what the status
/// encoder borrows, so the mapping lives here rather than being invented in the model
/// crate for one consumer.
pub fn format_name(format: starplayer::model::ModuleFormat) -> &'static str {
    use starplayer::model::ModuleFormat;
    match format {
        ModuleFormat::S3m => "S3M",
        ModuleFormat::Mod => "MOD",
        ModuleFormat::Mtm => "MTM",
        ModuleFormat::Xm => "XM",
        ModuleFormat::It => "IT",
    }
}

/// A slot's label, as the status endpoint reports it.
fn slot_label(id: u8) -> FixedStr<16> {
    let mut label = heapless::String::<16>::new();
    let _ = write!(label, "slot {id}");
    FixedStr::new(label.as_str())
}

/// Apply one fire-and-forget command.
fn apply_command(control: &mut ControlHalf, command: Command) {
    match command {
        Command::Play => {
            let _ = control.play();
        }
        Command::Stop => {
            let _ = control.stop();
        }
        Command::SeekOrder(order) => {
            let _ = control.seek_order(order);
        }
        Command::SeekRow(row) => {
            let _ = control.seek_row(row);
        }
        Command::SeekFrame(frame) => {
            let _ = control.seek_frame(frame);
        }
        Command::Volume(level) => {
            let _ = control.set_master_volume(level);
        }
        Command::Mute { channel, muted } => {
            let _ = control.mute(ChannelId(channel), muted);
        }
        Command::Skip(delta) => {
            let order = control.telemetry().transport.order;
            let target = i64::from(order) + i64::from(delta);
            let _ = control.seek_order(target.clamp(0, i64::from(u16::MAX)) as u16);
        }
        Command::AtEnd(at_end) => {
            let _ = control.set_at_end(at_end);
        }
    }
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// A response body built in a fixed buffer.
///
/// Pre-serialised into bytes rather than handed to picoserve as a `serde::Serialize`, for
/// the reason ampkeeper found the hard way: the response machinery monomorphises per body
/// type and each instantiation is kilobytes of stack frame. One concrete body type is one
/// instantiation whatever the endpoint.
struct Body {
    bytes: heapless::Vec<u8, BODY_BYTES>,
    content_type: &'static str,
}

impl Body {
    /// An empty JSON body, filled to capacity so that [`Body::scratch`] can be written
    /// into directly.
    ///
    /// Serialising into the body's own buffer rather than into a local array and copying
    /// is not tidiness: a worker's future lives in `.bss`, and the local array would be a
    /// second [`BODY_BYTES`] in it for every endpoint that answers with JSON.
    fn json() -> Body {
        let mut bytes = heapless::Vec::new();
        let _ = bytes.resize_default(BODY_BYTES);
        Body { bytes, content_type: "application/json" }
    }

    /// The buffer an encoder writes into.
    fn scratch(&mut self) -> &mut [u8] { &mut self.bytes }

    /// Keep the first `length` bytes — what the encoder actually wrote.
    fn truncate(&mut self, length: usize) { self.bytes.truncate(length); }
}

impl Content for Body {
    fn content_type(&self) -> &'static str { self.content_type }
    fn content_length(&self) -> usize { self.bytes.len() }
    async fn write_content<W: Write>(self, mut writer: W) -> Result<(), W::Error> { writer.write_all(&self.bytes).await }
}

/// A plain-text answer — every failure this API reports is a sentence, not a code.
fn text(status: StatusCode, message: &str) -> (StatusCode, Body) {
    let mut body = Body { bytes: heapless::Vec::new(), content_type: "text/plain; charset=utf-8" };
    let _ = body.bytes.extend_from_slice(message.as_bytes());
    (status, body)
}

/// Turn a job's answer into a response: 204 on success, 409 with the reason on failure.
fn job_response(outcome: JobOutcome) -> (StatusCode, Body) {
    match outcome {
        JobOutcome::Done => text(StatusCode::NO_CONTENT, ""),
        JobOutcome::Failed(message) => text(StatusCode::CONFLICT, message),
    }
}

/// `GET /api/status`.
async fn status_response() -> (StatusCode, Body) {
    let guard = STATUS.lock().await;
    let Some(published) = guard.as_ref() else {
        return text(StatusCode::SERVICE_UNAVAILABLE, "the player has not published a snapshot yet");
    };
    let status = api::Status::new(&published.view, &published.host, published.format, published.source.as_str());
    let mut body = Body::json();
    match api::write_status_json(body.scratch(), &status) {
        Some(length) => {
            body.truncate(length);
            (StatusCode::OK, body)
        }
        None => text(StatusCode::INTERNAL_SERVER_ERROR, "the status did not fit its buffer"),
    }
}

/// `GET /api/modules`.
async fn modules_response() -> (StatusCode, Body) {
    let current = STATUS.lock().await.as_ref().map(|published| published.module_id).unwrap_or(0);
    let entries = MODULES.lock().await;
    let mut list: heapless::Vec<api::ModuleEntry, MODULE_LIST_CAPACITY> = heapless::Vec::new();
    for entry in entries.iter() {
        let _ = list.push(api::ModuleEntry {
            id: entry.id,
            name: entry.name.as_str(),
            bytes: entry.bytes,
            source: if entry.id == 0 { "flash" } else { "slot" },
            current: entry.id == current,
        });
    }
    let mut body = Body::json();
    match api::write_modules_json(body.scratch(), &list) {
        Some(length) => {
            body.truncate(length);
            (StatusCode::OK, body)
        }
        None => text(StatusCode::INTERNAL_SERVER_ERROR, "the module list did not fit its buffer"),
    }
}

/// `POST /api/modules`: stream the body into PSRAM, then ask the control task to adopt it.
///
/// The staging lock is held for the whole call — that, and nothing else, is what makes a
/// second concurrent upload a 409.
async fn upload_response<R: Read>(request: &mut Request<'_, R>) -> (StatusCode, Body) {
    let Ok(mut guard) = STAGING.try_lock() else {
        return text(StatusCode::CONFLICT, "another upload is already in flight");
    };
    let Some(staging) = guard.as_mut() else {
        return text(StatusCode::INSUFFICIENT_STORAGE, "this board has no PSRAM to stage an upload in");
    };

    let capacity = staging.buffer.capacity();
    let body = request.body_connection.body();
    let expected = body.content_length();
    if let Err(error) = staging.upload.reserve(expected, capacity) {
        staging.upload.abort();
        return match error {
            api::UploadError::TooLarge => text(StatusCode::PAYLOAD_TOO_LARGE, "that module is larger than the upload buffer"),
            api::UploadError::Empty => text(StatusCode::BAD_REQUEST, "the request body was empty"),
            _ => text(StatusCode::CONFLICT, "another upload is already in flight"),
        };
    }

    // SAFETY: the staging buffer is never borrowed by a `Module` — the control task
    // copies out of it into an image half — and `STAGING`'s lock is held, so this is the
    // only writer.
    let destination = unsafe { staging.buffer.as_mut() };
    let mut reader = body.reader();
    let mut written = 0usize;
    while written < expected {
        match reader.read(&mut destination[written..expected]).await {
            Ok(0) => break,
            Ok(read) => {
                if staging.upload.append(read).is_err() {
                    staging.upload.abort();
                    return text(StatusCode::BAD_REQUEST, "the body was longer than its Content-Length");
                }
                written += read;
            }
            Err(_) => {
                staging.upload.abort();
                return text(StatusCode::BAD_REQUEST, "the upload was cut short");
            }
        }
    }
    let Ok(length) = staging.upload.complete() else {
        staging.upload.abort();
        return text(StatusCode::BAD_REQUEST, "the upload was cut short");
    };

    // SAFETY: `length` bytes were just written through the `as_mut` borrow above, which
    // has ended. The view stays valid while this function holds `STAGING`, which it does
    // until after the control task has answered.
    let Some(view) = (unsafe { staging.buffer.view(length) }) else {
        return text(StatusCode::INTERNAL_SERVER_ERROR, "the staged upload could not be read back");
    };
    match run_job(Job::LoadUpload(view)).await {
        JobOutcome::Done => text(StatusCode::CREATED, "playing"),
        JobOutcome::Failed(message) => text(StatusCode::CONFLICT, message),
    }
}

// ---------------------------------------------------------------------------
// The router
// ---------------------------------------------------------------------------

/// Every route, as one `match` that produces one response type.
///
/// Two things keep this flat, and both are measured rather than stylistic. picoserve's
/// `Router` is a cons list, so a `.route()` chain costs one monomorphised async frame per
/// registered route — that is why there is a single [`PathRouterService`] instead. And
/// `IntoResponse::write_to` is monomorphised per *response* type, with each instantiation
/// several kilobytes of the worker's future; the first draft of this file called it from
/// fourteen match arms and the resulting task pool was 48 KiB **per worker**, which does
/// not fit in this chip's DRAM beside the WiFi driver. So every JSON and text answer is
/// the same [`Body`] type, the request body is extracted once as bytes and decoded
/// synchronously, and `write_to` appears exactly three times below.
struct FlatRoutes;

impl PathRouterService<()> for FlatRoutes {
    async fn call_path_router_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self, state: &(), _path_parameters: (), path: Path<'_>, mut request: Request<'_, R>, response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let method = request.parts.method();
        let path = path.encoded();

        // The three routes that do not answer with a `Body`, each with its own
        // `write_to` instantiation and no others.
        if (method, path) == ("GET", "/") {
            return picoserve::response::File::with_content_type_and_headers(picoserve::response::File::MIME_HTML, INDEX_HTML_GZ, GZIP_HEADERS)
                .call_request_handler_service(state, (), request, response_writer)
                .await;
        }
        if (method, path) == ("GET", "/ws") {
            let upgrade = picoserve::from_request!(state, request, response_writer, WebSocketUpgrade);
            let response = upgrade.on_upgrade(TelemetrySocket);
            return response.write_to(request.body_connection.finalize().await?, response_writer).await;
        }
        if (method, path) == ("POST", "/api/modules") {
            let response = upload_response(&mut request).await;
            return response.write_to(request.body_connection.finalize().await?, response_writer).await;
        }

        // Everything else: one body extraction, one decode, one response type.
        let body = picoserve::from_request!(state, request, response_writer, &[u8]);
        let response = dispatch(method, path, body).await;
        response.write_to(request.body_connection.finalize().await?, response_writer).await
    }
}

/// Every endpoint whose answer is a [`Body`].
///
/// `body` is the request body, already read into picoserve's own buffer — every one of
/// these is a few dozen bytes of JSON, so decoding it here with `serde_json_core` rather
/// than through picoserve's `Json` extractor costs nothing and saves one monomorphised
/// extractor future per request type.
async fn dispatch(method: &str, path: &str, body: &[u8]) -> (StatusCode, Body) {
    match (method, path) {
        ("GET", "/api/status") => status_response().await,
        ("GET", "/api/modules") => modules_response().await,
        ("POST", "/api/play") => accepted(send_command(Command::Play)),
        ("POST", "/api/stop") => accepted(send_command(Command::Stop)),
        ("POST", "/api/next") => accepted(send_command(Command::Skip(1))),
        ("POST", "/api/previous") => accepted(send_command(Command::Skip(-1))),
        ("POST", "/api/seek") => match decode::<api::SeekRequest>(body) {
            Some(request) => accepted(send_command(Command::SeekOrder(request.order))),
            None => malformed(),
        },
        ("POST", "/api/volume") => match decode::<api::VolumeRequest>(body) {
            Some(request) => accepted(send_command(Command::Volume(U0F16::from_bits(request.level)))),
            None => malformed(),
        },
        ("POST", "/api/mute") => match decode::<api::MuteRequest>(body) {
            Some(request) => accepted(send_command(Command::Mute { channel: request.channel, muted: request.muted })),
            None => malformed(),
        },
        ("POST", "/api/modules/select") => match decode::<api::SlotRequest>(body) {
            Some(request) => job_response(run_job(Job::Select(request.id)).await),
            None => malformed(),
        },
        ("POST", "/api/modules/store") => match decode::<api::SlotRequest>(body) {
            Some(request) => job_response(run_job(Job::Store(request.id)).await),
            None => malformed(),
        },
        ("POST", "/api/reprovision") => job_response(run_job(Job::ForgetWifi).await),
        _ => text(StatusCode::NOT_FOUND, "no such endpoint"),
    }
}

/// Decode one small JSON request body.
fn decode<'a, T: serde::Deserialize<'a>>(body: &'a [u8]) -> Option<T> {
    serde_json_core::from_slice::<T>(body).ok().map(|(value, _)| value)
}

/// The answer to a body that would not decode.
fn malformed() -> (StatusCode, Body) { text(StatusCode::BAD_REQUEST, "that request body is not the JSON this endpoint expects") }

/// 204 when the command was queued, 503 when the channel was full.
fn accepted(queued: bool) -> (StatusCode, Body) {
    if queued { text(StatusCode::NO_CONTENT, "") } else { text(StatusCode::SERVICE_UNAVAILABLE, "the player is busy; try again") }
}

/// The router, for `picoserve::Server::new`.
struct Application;

impl picoserve::AppBuilder for Application {
    type PathRouter = impl picoserve::routing::PathRouter;

    fn build_app(self) -> picoserve::Router<Self::PathRouter> { picoserve::Router::new().nest_service("", FlatRoutes) }
}

// ---------------------------------------------------------------------------
// The WebSocket
// ---------------------------------------------------------------------------

/// Pushes the packed telemetry block at [`TELEMETRY_INTERVAL`] and accepts nine-byte
/// [`WireCommand`](firmware_common::api::WireCommand) frames.
///
/// One `await` covers both directions: a send-only loop would never notice the browser
/// closing the connection, and a receive-only loop would never send. The outbound timer
/// is the cancel-safe signal `next_message` takes.
struct TelemetrySocket;

impl WebSocketCallback for TelemetrySocket {
    async fn run<R: Read, W: Write<Error = R::Error>>(self, mut rx: SocketRx<R>, mut tx: SocketTx<W>) -> Result<(), W::Error> {
        // 128 bytes: a command frame is nine, and RFC 6455 caps a control frame's payload
        // at 125. Nothing this page sends is larger, and a frame that is gets the
        // connection closed rather than a bigger buffer.
        let mut receive_buffer = [0u8; 128];
        let close_reason = loop {
            match rx.next_message(&mut receive_buffer, Timer::after(TELEMETRY_INTERVAL)).await {
                Ok(picoserve::futures::Either::First(Ok(Message::Binary(frame)))) => {
                    if let Some(command) = api::decode_wire_command(frame) {
                        apply_wire_command(command);
                    }
                }
                Ok(picoserve::futures::Either::First(Ok(Message::Ping(data)))) => tx.send_pong(data).await?,
                Ok(picoserve::futures::Either::First(Ok(Message::Close(_)))) => break None,
                Ok(picoserve::futures::Either::First(Ok(_))) => {}
                Ok(picoserve::futures::Either::First(Err(_))) => break Some((1002u16, "protocol error")),
                Ok(picoserve::futures::Either::Second(())) => {
                    // Sent straight out of the shared block, under its lock, rather than
                    // copied into a per-worker buffer: the control task publishes with
                    // `try_lock` and simply skips a frame it cannot take, so a slow
                    // client costs staleness rather than blocking the player.
                    let telemetry = TELEMETRY.lock().await;
                    if telemetry.length > 0 {
                        let send = tx.send_binary(&telemetry.bytes[..telemetry.length]);
                        match with_timeout(WEBSOCKET_WRITE_TIMEOUT, send).await {
                            Ok(Ok(())) => {}
                            Ok(Err(error)) => return Err(error),
                            Err(_) => break Some((1011, "send timeout")),
                        }
                    }
                }
                Err(error) => return Err(error),
            }
        };
        tx.close(close_reason).await
    }
}

/// Turn one wire command into a queued [`Command`].
fn apply_wire_command(command: api::WireCommand) {
    use firmware_common::api::opcode;
    let queued = match command.opcode {
        opcode::PLAY => send_command(Command::Play),
        opcode::STOP => send_command(Command::Stop),
        opcode::SEEK_ORDER => send_command(Command::SeekOrder(command.argument as u16)),
        opcode::SEEK_ROW => send_command(Command::SeekRow(command.argument as u16)),
        opcode::SEEK_FRAME => send_command(Command::SeekFrame(u64::from(command.argument))),
        opcode::MASTER_VOLUME => send_command(Command::Volume(U0F16::from_bits(command.argument as u16))),
        opcode::MUTE_CHANNEL => send_command(Command::Mute { channel: command.argument as u16, muted: command.extra != 0 }),
        opcode::AT_END => match api::at_end_from_wire(command.argument) {
            Some(at_end) => send_command(Command::AtEnd(at_end)),
            None => false,
        },
        _ => false,
    };
    let _ = queued;
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

/// Spawn the web workers.
pub fn start(spawner: Spawner, stack: Stack<'static>) -> Result<(), &'static str> {
    static CONFIG: StaticCell<picoserve::Config> = StaticCell::new();
    let config: &'static picoserve::Config = CONFIG.init(
        picoserve::Config::new(picoserve::Timeouts {
            start_read_request: Duration::from_secs(5),
            persistent_start_read_request: Duration::from_secs(2),
            // Generous, because a module upload is one long request body over a link that
            // may be slow; the default three seconds would abort a phone on poor WiFi
            // part-way through a 200 KB file.
            read_request: Duration::from_secs(20),
            write: Duration::from_secs(5),
        })
        .keep_connection_alive(),
    );
    for id in 0..WEB_TASK_POOL_SIZE {
        spawner.spawn(web_task(id, stack, config).map_err(|_| "a web worker would not spawn")?);
    }
    Ok(())
}

/// One connection's worth of server.
#[embassy_executor::task(pool_size = WEB_TASK_POOL_SIZE)]
async fn web_task(id: usize, stack: Stack<'static>, config: &'static picoserve::Config) {
    let mut receive_buffer = [0u8; TCP_BUFFER_BYTES];
    let mut transmit_buffer = [0u8; TCP_BUFFER_BYTES];
    let mut http_buffer = [0u8; HTTP_BUFFER_BYTES];
    let application = picoserve::AppBuilder::build_app(Application);

    loop {
        if !stack.is_link_up() {
            Timer::after(Duration::from_millis(500)).await;
            continue;
        }
        let mut socket = TcpSocket::new(stack, &mut receive_buffer, &mut transmit_buffer);
        // A bounded accept: a listener left in SYN_RCVD retransmits into the radio's own
        // transmit queue, and two workers doing that indefinitely is how a small queue
        // gets exhausted. `abort`, not `drop` — dropping only schedules the close.
        match with_timeout(Duration::from_secs(30), socket.accept(crate::net::HTTP_PORT)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => continue,
            Err(_) => {
                socket.abort();
                continue;
            }
        }
        socket.set_timeout(Some(Duration::from_secs(45)));
        socket.set_keep_alive(Some(Duration::from_secs(30)));

        if let Err(error) = picoserve::Server::new(&application, config, &mut http_buffer).serve(socket).await {
            println!("WEB worker {id}: {error:?}");
        }
    }
}
