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
//!   volume, mute). The control-task boundary clamps volume to the A1S's 1/4 maximum. A
//!   full channel drops the command rather than blocking a worker.
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
//! Raw module decoding cannot use the global heap on this board: it is internal DRAM and
//! the WiFi build deliberately leaves only a small reserve there (see `psram.rs` for the
//! atomics erratum that keeps PSRAM out of the allocator). The upload path therefore owns
//! fixed PSRAM storage from end to end:
//!
//! 1. the request body streams into the **PSRAM staging buffer** — the raw file never
//!    touches DRAM;
//! 2. an existing module image (`SPMI` magic — what `cargo xtask module-image` writes) is
//!    copied to the free image buffer in bounded chunks;
//! 3. any of the five raw formats is incrementally decoded straight into that image
//!    buffer, using the separate 256 KiB PSRAM workspace and yielding between bounded
//!    input/PCM steps;
//! 4. `Module::try_from_image` over the PSRAM image borrows the pattern blob and the PCM
//!    **in place**, so what stays in DRAM is the `Arc`, four small index vectors and the
//!    sequencer.


use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_executor::Spawner;
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::channel::Channel;
use embassy_sync::mutex::{Mutex, MutexGuard};
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
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
use starplayer::model::{Module, ModuleBuilder, ModuleFormat, ModuleHeader};
use starplayer::model::PatternId;
use starplayer::model::image::IMAGE_MAGIC;
use starplayer::rt::Arc;
use starplayer::{DecodeBudget, ImageDecodeError, ImageDecodeStatus, ModuleImageDecoder};
use starplayer_host_embedded::ControlHalf;
use static_cell::StaticCell;

use crate::psram;
use crate::store::{SLOT_IMAGE_MAX_BYTES, SLOT_NAME_BYTES, Store};

/// How many connections the server handles at once.
///
/// Two, not four. Each worker's large future and its TCP buffers live in a boot-time
/// PSRAM arena claim, while its pointer-sized task proxy and Embassy header remain in
/// internal DRAM. Two is also research point 5's cap on WebSocket clients, and it falls
/// out of this rather than needing a counter of its own.
pub const WEB_WORKER_COUNT: usize = 2;

/// Fixed web-player capacity. Scope rings and deep telemetry history are disabled for
/// this personality so these eight channels and voices fit the same internal-RAM budget.
pub const PLAYBACK_CHANNEL_CAPACITY: usize = 8;
pub const PLAYBACK_VOICE_CAPACITY: usize = 8;

/// TCP receive and transmit buffers, per worker.
const TCP_BUFFER_BYTES: usize = 1024;
/// picoserve's own request buffer, per worker: large enough for any header block a
/// browser sends.
const HTTP_BUFFER_BYTES: usize = 1024;
/// The largest JSON body this API answers with — the status, with sixteen channel rows.
///
/// This is sized to the largest real answer rather than rounded up: sixteen channel rows
/// of roughly seventy bytes plus a header of about two hundred. Each worker owns one of
/// these buffers in its PSRAM-resident future and lends it to [`Body`] while writing.
const BODY_BYTES: usize = 1408;

/// Internal heap that must still be free after image adoption and playback preparation.
const INTERNAL_HEAP_HEADROOM_BYTES: usize = 8 * 1024;

/// Decoder work handed to one incremental step before yielding to the executor.
const DECODE_INPUT_BYTES_PER_STEP: usize = 4096;
const DECODE_PCM_FRAMES_PER_STEP: usize = 1024;

/// How long a worker waits for the control task to answer a [`Job`].
const JOB_TIMEOUT: Duration = Duration::from_secs(30);
/// Control-side deadline, deliberately shorter than the HTTP wait. The gap guarantees a
/// timed-out request cannot race a late playback commit on the same cooperative executor.
const JOB_CONTROL_TIMEOUT: Duration = Duration::from_secs(25);

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
enum Job {
    /// Decode (or borrow) the initialized prefix of this staging buffer and swap the
    /// result in. Ownership crosses with the job, so an HTTP timeout cannot free the
    /// buffer for reuse while the control task still has a late request queued.
    LoadUpload { staging: Staging, length: usize },
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
static JOB_REQUEST: Signal<CriticalSectionRawMutex, JobRequest> = Signal::new();
static JOB_DONE: Signal<CriticalSectionRawMutex, JobReply> = Signal::new();
static JOB_BUSY: AtomicBool = AtomicBool::new(false);
static NEXT_JOB_ID: AtomicU32 = AtomicU32::new(1);
static CANCELLED_JOB_ID: AtomicU32 = AtomicU32::new(0);

struct JobRequest {
    id: u32,
    deadline: Instant,
    job: Job,
}

#[derive(Copy, Clone)]
struct JobReply {
    id: u32,
    outcome: JobOutcome,
}

struct QueuedJob {
    id: u32,
    completed: bool,
}

impl Drop for QueuedJob {
    fn drop(&mut self) {
        if !self.completed {
            CANCELLED_JOB_ID.store(self.id, Ordering::Release);
        }
    }
}

#[derive(Copy, Clone)]
struct JobContext {
    id: u32,
    deadline: Instant,
}

impl JobContext {
    fn cancelled(self) -> bool {
        CANCELLED_JOB_ID.load(Ordering::Acquire) == self.id || Instant::now() >= self.deadline
    }
}

fn queue_job(job: Job) -> Result<QueuedJob, Job> {
    if JOB_BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err(job);
    }
    let mut id = NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        id = NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed);
    }
    JOB_DONE.reset();
    JOB_REQUEST.signal(JobRequest { id, deadline: Instant::now() + JOB_CONTROL_TIMEOUT, job });
    Ok(QueuedJob { id, completed: false })
}

async fn wait_for_job(mut queued: QueuedJob) -> JobOutcome {
    match with_timeout(JOB_TIMEOUT, JOB_DONE.wait()).await {
        Ok(reply) if reply.id == queued.id => {
            queued.completed = true;
            reply.outcome
        }
        Ok(_) => JobOutcome::Failed("the player returned an obsolete request result; try again"),
        Err(_) => JobOutcome::Failed("the player is still preparing that request; try again"),
    }
}

/// Hand `job` to the control task and wait for its answer.
async fn run_job(job: Job) -> JobOutcome {
    let _guard = JOB_LOCK.lock().await;
    match queue_job(job) {
        Ok(queued) => wait_for_job(queued).await,
        Err(_) => JobOutcome::Failed("the player is still finishing an earlier request; try again"),
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

/// Dynamic capacity of each of the three equal PSRAM module buffers. Written once before
/// the executor starts and read by `GET /api/upload-limits`.
static MODULE_BUFFER_BYTES: AtomicU32 = AtomicU32::new(0);

/// Set by `POST /api/reprovision` once the credentials are gone; `main`'s watcher reboots
/// a moment later, so the browser sees its answer first.
static REBOOT_REQUESTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Wait for a reboot request. `main` spawns this beside the web tasks.
pub async fn wait_for_reboot() { REBOOT_REQUESTED.wait().await }

/// Everything `main` builds for the web before the audio starts, handed to `play` as one
/// value so that the boot sequence's argument list stays readable.
pub struct Boot {
    pub bridge: Bridge,
    /// What remains after the upload and image buffers. Only the selected network
    /// personality claims its picoserve worker futures from it.
    pub psram_arena: psram::Arena,
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
    /// Stable internal-RAM ownership tokens paired with the two image buffers. A token
    /// is replaced in place only when its strong count proves the control queue, render
    /// engine and retired source have all released it.
    modules: Option<[Arc<Module>; 2]>,
    /// Reusable decoder scratch, present exactly when the three dynamic module buffers
    /// were successfully claimed.
    workspace: Option<psram::Buffer>,
    /// Shared capacity of staging and both image buffers. Zero without usable PSRAM.
    buffer_bytes: usize,
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
        let layout = firmware_common::PsramLayout::calculate(arena.remaining(), 0, psram::DECODER_WORKSPACE_BYTES);
        let (mut workspace, images, mut buffer_bytes) = match layout.and_then(|layout| {
            Some((arena.claim(layout.workspace_bytes)?, arena.claim(layout.buffer_bytes)?, arena.claim(layout.buffer_bytes)?, arena.claim(layout.buffer_bytes)?, layout.buffer_bytes))
        }) {
            Some((workspace, staging, first, second, buffer_bytes)) => {
                // `try_lock`, not `lock`: this runs before any executor polls, so an
                // await here would never complete.
                if let Ok(mut guard) = STAGING.try_lock() {
                    *guard = Some(Staging { buffer: staging, upload: Upload::new() });
                }
                (Some(workspace), Some([first, second]), buffer_bytes)
            }
            None => (None, None, 0),
        };
        let modules = if images.is_some() {
            match (Module::try_from_image(crate::images::boot_module()), empty_module()) {
                (Ok(first), Some(second)) => Some([Arc::new(first), Arc::new(second)]),
                _ => None,
            }
        } else {
            None
        };
        if modules.is_none() {
            workspace = None;
            buffer_bytes = 0;
            if let Ok(mut guard) = STAGING.try_lock() {
                *guard = None;
            }
        }
        let mut bridge = Bridge { store, images, modules, workspace, buffer_bytes, live: 0, live_image: None, source: FixedStr::new("flash"), module_id: 0, format };
        MODULE_BUFFER_BYTES.store(buffer_bytes.min(u32::MAX as usize) as u32, Ordering::Relaxed);
        bridge.refresh_modules();
        bridge
    }

    /// Whether this board can hold an uploaded or stored module at all.
    pub fn has_psram(&self) -> bool { self.images.is_some() && self.modules.is_some() }

    /// The preallocated token used for initial flash playback. Cloning a token does not
    /// allocate, and keeping the original here is what later proves every other owner
    /// has retired.
    pub fn initial_module(&self) -> Option<Arc<Module>> {
        self.modules.as_ref().and_then(|modules| modules.get(self.live)).map(Arc::clone)
    }

    /// Install the compiled-in module with its timeline tables in the otherwise-empty
    /// first image buffer. The module itself keeps borrowing flash; only the immutable
    /// scan tables use PSRAM.
    pub fn load_initial(&mut self, control: &mut ControlHalf, module: Arc<Module>) -> Result<(), &'static str> {
        if !self.has_psram() {
            return Err("PSRAM playback storage is unavailable");
        }
        let order_count = module.orders().len();
        let mark_capacity = maximum_timeline_marks(&module).ok_or("the linked module's playback timeline is too large")?;
        // SAFETY: no previous module has used either image buffer during boot. The tables
        // become read-only as soon as the prepared source is committed.
        let (marks, order_marks) = unsafe {
            self.image_half(self.live).initialize_tail_tables::<Option<u32>, starplayer::engine::RowMark>(
                0, order_count, mark_capacity,
            )
        }
        .ok_or("the linked module's playback timeline does not fit in PSRAM")?;
        let prepared = control
            .try_prepare_load_in(module, marks, order_marks)
            .map_err(|_| "there is not enough internal RAM to prepare the linked module")?;
        control.commit_prepared_load(prepared).map_err(|_| "the command ring rejected the linked module")
    }

    /// Re-read every slot header into [`MODULES`].
    fn refresh_modules(&mut self) {
        let mut entries: heapless::Vec<SlotEntry, MODULE_LIST_CAPACITY> = heapless::Vec::new();
        let _ = entries.push(SlotEntry { id: 0, name: FixedStr::new(crate::images::BOOT_MODULE_NAME), bytes: crate::images::boot_module().len() as u32 });
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
        if let Some(request) = JOB_REQUEST.try_take() {
            let context = JobContext { id: request.id, deadline: request.deadline };
            let outcome = self.run(control, request.job, context).await;
            JOB_DONE.signal(JobReply { id: request.id, outcome });
            JOB_BUSY.store(false, Ordering::Release);
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
    async fn run(&mut self, control: &mut ControlHalf, job: Job, context: JobContext) -> JobOutcome {
        if context.cancelled() {
            if let Job::LoadUpload { staging, .. } = job {
                *STAGING.lock().await = Some(staging);
            }
            return JobOutcome::Failed("the request was canceled before preparation began");
        }
        match job {
            Job::LoadUpload { staging, length } => {
                let outcome = match unsafe { staging.buffer.view(length) } {
                    Some(body) => self.load_upload(control, body, context).await,
                    None => JobOutcome::Failed("the staged upload could not be read back"),
                };
                *STAGING.lock().await = Some(staging);
                outcome
            }
            Job::Select(id) => self.select(control, id, context).await,
            Job::Store(id) => self.store_current(control, id).await,
            Job::ForgetWifi => self.forget_wifi(control).await,
            Job::SaveWifi => self.save_wifi(control).await,
        }
    }

    /// The free image buffer — the one the playing module is not borrowing.
    fn free_image(&self) -> usize { 1 - self.live }

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
    /// The bound is what makes this honest rather than a spin: one hundred tries at 10 ms
    /// covers hundreds of render quanta. If ownership has still not returned, the request
    /// fails busy and this buffer stays untouched.
    async fn wait_for_retirement(&mut self, control: &mut ControlHalf, slot: usize, context: JobContext) -> bool {
        for _ in 0..100 {
            if context.cancelled() {
                return false;
            }
            let released = self.modules.as_ref().and_then(|modules| modules.get(slot)).is_some_and(|module| control.module_is_released(module));
            if released {
                return true;
            }
            Timer::after(Duration::from_millis(10)).await;
        }
        false
    }

    /// One half of the ping-pong. Only reached once `images` is known to be `Some`.
    fn image_half(&mut self, slot: usize) -> &mut psram::Buffer {
        &mut self.images.as_mut().expect("the callers check `images` before claiming a half")[slot]
    }

    /// Drop the released inactive module's DRAM metadata before decoding its replacement.
    /// The empty module owns no allocated elements and keeps the preallocated Arc token in
    /// place, ready for `Arc::get_mut` adoption.
    fn clear_module_slot(&mut self, slot: usize) -> bool {
        let Some(empty) = empty_module() else { return false };
        let Some(token) = self.modules.as_mut().and_then(|modules| modules.get_mut(slot)).and_then(Arc::get_mut) else {
            return false;
        };
        *token = empty;
        true
    }

    /// Build a module from an image already sitting in PSRAM half `slot`, and swap it in.
    fn adopt(
        &mut self, control: &mut ControlHalf, slot: usize, image: &'static [u8], source: &str, id: u8, context: JobContext,
    ) -> JobOutcome {
        if context.cancelled() {
            return JobOutcome::Failed("module preparation was canceled");
        }
        let module = match Module::try_from_image(image) {
            Ok(module) => module,
            Err(starplayer::core::Error::Resource(message)) => return JobOutcome::Failed(message),
            Err(_) => return JobOutcome::Failed("the module image is invalid or incompatible with this firmware"),
        };
        if module.header().channel_count as usize > control.channel_capacity() {
            return JobOutcome::Failed("the module has more than this firmware's 8-channel playback limit");
        }
        let format = format_name(module.header().format);
        let Some(token) = self.modules.as_mut().and_then(|modules| modules.get_mut(slot)).and_then(Arc::get_mut) else {
            return JobOutcome::Failed("the previous module is still being released; try again");
        };
        *token = module;
        let Some(module) = self.modules.as_ref().and_then(|modules| modules.get(slot)).map(Arc::clone) else {
            return JobOutcome::Failed("the PSRAM module slot is unavailable");
        };
        let order_count = module.orders().len();
        let Some(mark_capacity) = maximum_timeline_marks(&module) else {
            drop(module);
            self.clear_module_slot(slot);
            return JobOutcome::Failed("the module's playback timeline is too large");
        };
        let timeline_prefix = if source == "flash" { 0 } else { image.len() };
        // SAFETY: retirement was confirmed before this image or its tail was written;
        // these initialized tables stay read-only in the scan/source until the same token
        // is unique again.
        let timeline_tables = unsafe {
            self.image_half(slot).initialize_tail_tables::<Option<u32>, starplayer::engine::RowMark>(
                timeline_prefix, order_count, mark_capacity,
            )
        };
        let Some((marks, order_marks)) = timeline_tables else {
            drop(module);
            self.clear_module_slot(slot);
            return JobOutcome::Failed("the decoded image leaves insufficient PSRAM for its playback timeline");
        };
        let prepared = match control.try_prepare_load_in(module, marks, order_marks) {
            Ok(prepared) => prepared,
            Err(starplayer_host_embedded::Error::Module(starplayer::core::Error::Resource(message))) => {
                self.clear_module_slot(slot);
                return JobOutcome::Failed(message);
            }
            Err(_) => {
                self.clear_module_slot(slot);
                return JobOutcome::Failed("the module could not be prepared for playback");
            }
        };
        if context.cancelled() {
            drop(prepared);
            self.clear_module_slot(slot);
            return JobOutcome::Failed("module preparation was canceled");
        }
        if esp_alloc::HEAP.free() < INTERNAL_HEAP_HEADROOM_BYTES {
            drop(prepared);
            self.clear_module_slot(slot);
            return JobOutcome::Failed("module metadata would leave less than 8 KiB of internal RAM free");
        }
        if control.commit_prepared_load(prepared).is_err() {
            self.clear_module_slot(slot);
            return JobOutcome::Failed("the command ring is full; try again in a moment");
        }
        self.format = format;
        self.live = slot;
        self.live_image = if source == "flash" { None } else { Some(image) };
        self.module_id = id;
        self.source = FixedStr::new(source);
        GENERATION.fetch_add(1, Ordering::Relaxed);
        let _ = control.play();
        JobOutcome::Done
    }

    /// `POST /api/modules`: whatever the browser sent is in PSRAM; decide what it is.
    async fn load_upload(&mut self, control: &mut ControlHalf, body: &'static [u8], context: JobContext) -> JobOutcome {
        if !self.has_psram() {
            return JobOutcome::Failed("this board has no PSRAM, so it can only play the compiled-in module");
        }
        let slot = self.free_image();
        if !self.wait_for_retirement(control, slot, context).await {
            return JobOutcome::Failed("the previous module is still being released; try again");
        }
        if !self.clear_module_slot(slot) {
            return JobOutcome::Failed("the previous module is still being released; try again");
        }
        println!("HEAP upload before conversion: {}", esp_alloc::HEAP.stats());

        if body.starts_with(&IMAGE_MAGIC) {
            if body.len() > self.buffer_bytes {
                return JobOutcome::Failed("that image is larger than the PSRAM buffer");
            }
            // SAFETY: retirement and `clear_module_slot` proved nothing still borrows
            // this image half. The staging body is a disjoint arena claim.
            let destination = unsafe { self.image_half(slot).as_mut() };
            for (source, destination) in body.chunks(DECODE_INPUT_BYTES_PER_STEP).zip(destination.chunks_mut(DECODE_INPUT_BYTES_PER_STEP)) {
                if context.cancelled() {
                    return JobOutcome::Failed("module preparation was canceled");
                }
                destination[..source.len()].copy_from_slice(source);
                embassy_futures::yield_now().await;
            }
            let image: &'static [u8] = &destination[..body.len()];
            println!("HEAP upload after conversion: {}", esp_alloc::HEAP.stats());
            let outcome = self.adopt(control, slot, image, "upload", 0, context);
            println!("HEAP upload after adoption: {}", esp_alloc::HEAP.stats());
            return outcome;
        }

        // SAFETY: as in the SPMI branch. Workspace is a separate boot-time claim and no
        // other job can run while this one owns JOB_BUSY.
        let destination = unsafe { self.image_half(slot).as_mut() };
        let Some(workspace) = self.workspace.as_mut() else {
            return JobOutcome::Failed("this board has no PSRAM decoder workspace");
        };
        let workspace = unsafe { workspace.as_mut() };
        let image_length = {
            let mut decoder = match ModuleImageDecoder::new(body, destination, workspace) {
                Ok(decoder) => decoder,
                Err(error) => return JobOutcome::Failed(decode_error_message(error)),
            };
            loop {
                if context.cancelled() {
                    return JobOutcome::Failed("module preparation was canceled");
                }
                match decoder.step(DecodeBudget {
                    max_input_bytes: DECODE_INPUT_BYTES_PER_STEP,
                    max_pcm_frames: DECODE_PCM_FRAMES_PER_STEP,
                }) {
                    Ok(ImageDecodeStatus::Pending) => embassy_futures::yield_now().await,
                    Ok(ImageDecodeStatus::Complete { image_length }) => break image_length,
                    Err(error) => return JobOutcome::Failed(decode_error_message(error)),
                }
            }
        };
        let image: &'static [u8] = &destination[..image_length];
        println!("HEAP upload after conversion: {}", esp_alloc::HEAP.stats());
        let outcome = self.adopt(control, slot, image, "upload", 0, context);
        println!("HEAP upload after adoption: {}", esp_alloc::HEAP.stats());
        outcome
    }

    /// `POST /api/modules/select`: the compiled-in image, or one read out of a slot.
    async fn select(&mut self, control: &mut ControlHalf, id: u8, context: JobContext) -> JobOutcome {
        if id == 0 {
            let slot = self.free_image();
            if !self.wait_for_retirement(control, slot, context).await {
                return JobOutcome::Failed("the previous module is still being released; try again");
            }
            if !self.clear_module_slot(slot) {
                return JobOutcome::Failed("the previous module is still being released; try again");
            }
            return self.adopt(control, slot, crate::images::boot_module(), "flash", 0, context);
        }

        if !self.has_psram() {
            return JobOutcome::Failed("this board has no PSRAM to read a stored module into");
        }
        let slot = self.free_image();
        let was_playing = control.is_playing();
        // A long flash read contends for the flash bus with core 1's own instruction
        // fetches, so the transport stops around it — for audio quality, not for safety
        // (see `store.rs`).
        self.pause_playback(control).await;
        if !self.wait_for_retirement(control, slot, context).await {
            if was_playing { let _ = control.play(); }
            return JobOutcome::Failed("the previous module is still being released; try again");
        }
        if !self.clear_module_slot(slot) {
            if was_playing { let _ = control.play(); }
            return JobOutcome::Failed("the previous module is still being released; try again");
        }
        // SAFETY: `slot` is the half the playing module is not borrowing, and
        // `wait_for_retirement` has dropped whatever module was.
        let destination = unsafe { self.image_half(slot).as_mut() };
        let read = self.store.read_slot(id, destination);
        let length = match read {
            Ok(length) => length,
            Err(_) => {
                if was_playing { let _ = control.play(); }
                return JobOutcome::Failed("that slot is empty or would not read");
            }
        };
        // SAFETY: the same half, and `length` of its bytes were just written by the read.
        let Some(image) = (unsafe { self.image_half(slot).view(length) }) else {
            if was_playing { let _ = control.play(); }
            return JobOutcome::Failed("the slot image is larger than the PSRAM buffer");
        };
        let label = slot_label(id);
        let outcome = self.adopt(control, slot, image, label.as_str(), id, context);
        if !matches!(outcome, JobOutcome::Done) && was_playing {
            let _ = control.play();
        }
        outcome
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
    /// seconds of a loudly repeated 17 ms fragment rather than a gap. 150 ms covers the
    /// transport's 64-frame ramp and the ring's own 17 ms depth several times over.
    async fn pause_playback(&mut self, control: &mut ControlHalf) {
        let _ = control.stop();
        Timer::after(Duration::from_millis(150)).await;
    }
}

/// A valid allocation-free value for an inactive preallocated Arc token.
fn empty_module() -> Option<Module> {
    let mut builder = ModuleBuilder::new();
    builder.set_header(ModuleHeader::new(ModuleFormat::S3m, 1));
    builder.build().ok()
}

fn decode_error_message(error: ImageDecodeError) -> &'static str {
    match error {
        ImageDecodeError::DestinationTooSmall { .. } => "the decoded module is larger than the PSRAM image buffer",
        ImageDecodeError::WorkspaceTooSmall { .. } => "the module needs more than the 256 KiB decoder workspace",
        ImageDecodeError::BudgetTooSmall { .. } => "the firmware's decoder work budget is too small",
        ImageDecodeError::Module(starplayer::core::Error::Resource(message)) => message,
        ImageDecodeError::Module(_) => "that is not a valid module this build can play",
    }
}

/// Upper bound on distinct row visits before a timeline repeats: every row of every
/// playable order. Duplicate pattern orders count independently because their order
/// number is part of a row mark.
fn maximum_timeline_marks(module: &Module) -> Option<usize> {
    let mut rows = 0usize;
    for &order in module.orders() {
        let Some(pattern) = module.pattern(PatternId(order)) else { continue };
        rows = rows.checked_add(pattern.rows() as usize)?;
    }
    Some(rows)
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
            let _ = control.set_master_volume(firmware_common::cap_master_volume(level, crate::MAX_MASTER_VOLUME));
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

/// One worker's response bytes, stored with that worker's future in PSRAM.
type ResponseBuffer = Mutex<NoopRawMutex, [u8; BODY_BYTES]>;

/// A response body borrowed from one worker's fixed buffer.
///
/// Pre-serialised into bytes rather than handed to picoserve as a `serde::Serialize`, for
/// the reason ampkeeper found the hard way: the response machinery monomorphises per body
/// type and each instantiation is kilobytes of stack frame. One concrete body type is one
/// instantiation whatever the endpoint. The large byte array itself stays in the worker's
/// PSRAM-resident future: passing a `heapless::Vec` by value made generated picoserve poll
/// functions reserve tens of kilobytes of core-0 stack while moving async state.
struct Body<'a> {
    buffer: MutexGuard<'a, NoopRawMutex, [u8; BODY_BYTES]>,
    length: usize,
    content_type: &'static str,
}

impl<'a> Body<'a> {
    /// Lock one worker's response buffer for the response's lifetime.
    async fn new(response_buffer: &'a ResponseBuffer, content_type: &'static str) -> Body<'a> {
        Body { buffer: response_buffer.lock().await, length: 0, content_type }
    }

    /// The buffer an encoder writes into.
    fn scratch(&mut self) -> &mut [u8] { &mut *self.buffer }

    /// Keep the first `length` bytes — what the encoder actually wrote.
    fn truncate(&mut self, length: usize) { self.length = length; }
}

impl Content for Body<'_> {
    fn content_type(&self) -> &'static str { self.content_type }
    fn content_length(&self) -> usize { self.length }
    async fn write_content<W: Write>(self, mut writer: W) -> Result<(), W::Error> { writer.write_all(&self.buffer[..self.length]).await }
}

/// A plain-text answer — every failure this API reports is a sentence, not a code.
async fn text<'a>(response_buffer: &'a ResponseBuffer, status: StatusCode, message: &str) -> (StatusCode, Body<'a>) {
    let mut body = Body::new(response_buffer, "text/plain; charset=utf-8").await;
    let length = message.len().min(BODY_BYTES);
    body.buffer[..length].copy_from_slice(&message.as_bytes()[..length]);
    body.length = length;
    (status, body)
}

/// Turn a job's answer into a response: 204 on success, 409 with the reason on failure.
async fn job_response(response_buffer: &ResponseBuffer, outcome: JobOutcome) -> (StatusCode, Body<'_>) {
    match outcome {
        JobOutcome::Done => text(response_buffer, StatusCode::NO_CONTENT, "").await,
        JobOutcome::Failed(message) => text(response_buffer, StatusCode::CONFLICT, message).await,
    }
}

/// `GET /api/status`.
async fn status_response(response_buffer: &ResponseBuffer) -> (StatusCode, Body<'_>) {
    let guard = STATUS.lock().await;
    let Some(published) = guard.as_ref() else {
        return text(response_buffer, StatusCode::SERVICE_UNAVAILABLE, "the player has not published a snapshot yet").await;
    };
    let status = api::Status::new(&published.view, &published.host, published.format, published.source.as_str());
    let mut body = Body::new(response_buffer, "application/json").await;
    match api::write_status_json(body.scratch(), &status) {
        Some(length) => {
            body.truncate(length);
            (StatusCode::OK, body)
        }
        None => {
            drop(body);
            text(response_buffer, StatusCode::INTERNAL_SERVER_ERROR, "the status did not fit its buffer").await
        }
    }
}

/// `GET /api/modules`.
async fn modules_response(response_buffer: &ResponseBuffer) -> (StatusCode, Body<'_>) {
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
    let mut body = Body::new(response_buffer, "application/json").await;
    match api::write_modules_json(body.scratch(), &list) {
        Some(length) => {
            body.truncate(length);
            (StatusCode::OK, body)
        }
        None => {
            drop(body);
            text(response_buffer, StatusCode::INTERNAL_SERVER_ERROR, "the module list did not fit its buffer").await
        }
    }
}

/// `GET /api/upload-limits`.
async fn upload_limits_response(response_buffer: &ResponseBuffer) -> (StatusCode, Body<'_>) {
    let buffer_bytes = MODULE_BUFFER_BYTES.load(Ordering::Relaxed) as usize;
    let limits = api::UploadLimits {
        max_upload_bytes: buffer_bytes,
        max_image_bytes: buffer_bytes,
        max_stored_image_bytes: SLOT_IMAGE_MAX_BYTES,
    };
    let mut body = Body::new(response_buffer, "application/json").await;
    match api::write_upload_limits_json(body.scratch(), &limits) {
        Some(length) => {
            body.truncate(length);
            (StatusCode::OK, body)
        }
        None => {
            drop(body);
            text(response_buffer, StatusCode::INTERNAL_SERVER_ERROR, "the upload limits did not fit their buffer").await
        }
    }
}

/// `POST /api/modules`: stream the body into PSRAM, then ask the control task to adopt it.
///
/// The staging lock is held for the whole call — that, and nothing else, is what makes a
/// second concurrent upload a 409.
async fn upload_response<'a, R: Read>(request: &mut Request<'_, R>, response_buffer: &'a ResponseBuffer) -> (StatusCode, Body<'a>) {
    let Ok(mut guard) = STAGING.try_lock() else {
        return text(response_buffer, StatusCode::CONFLICT, "another upload is already in flight").await;
    };
    let Some(staging) = guard.as_mut() else {
        return if MODULE_BUFFER_BYTES.load(Ordering::Relaxed) == 0 {
            text(response_buffer, StatusCode::INSUFFICIENT_STORAGE, "this board has no PSRAM to stage an upload in").await
        } else {
            text(response_buffer, StatusCode::CONFLICT, "the previous upload is still being prepared").await
        };
    };

    let capacity = staging.buffer.capacity();
    let body = request.body_connection.body();
    let expected = body.content_length();
    // A canceled HTTP future drops this mutex guard but cannot run explicit cleanup. Since
    // taking the guard proves no body reader or queued job owns this staging value, any
    // surviving InFlight state belongs to that abandoned request and is safe to reset.
    staging.upload.abort();
    if let Err(error) = staging.upload.reserve(expected, capacity) {
        staging.upload.abort();
        return match error {
            api::UploadError::TooLarge => text(response_buffer, StatusCode::PAYLOAD_TOO_LARGE, "that module is larger than the upload buffer").await,
            api::UploadError::Empty => text(response_buffer, StatusCode::BAD_REQUEST, "the request body was empty").await,
            _ => text(response_buffer, StatusCode::CONFLICT, "another upload is already in flight").await,
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
                    return text(response_buffer, StatusCode::BAD_REQUEST, "the body was longer than its Content-Length").await;
                }
                written += read;
            }
            Err(_) => {
                staging.upload.abort();
                return text(response_buffer, StatusCode::BAD_REQUEST, "the upload was cut short").await;
            }
        }
    }
    let Ok(length) = staging.upload.complete() else {
        staging.upload.abort();
        return text(response_buffer, StatusCode::BAD_REQUEST, "the upload was cut short").await;
    };

    // Acquire the job serializer before moving the staging owner. If this HTTP future is
    // canceled while waiting, the buffer is still present in `STAGING`.
    let job_guard = JOB_LOCK.lock().await;
    if JOB_BUSY.load(Ordering::Acquire) {
        return text(response_buffer, StatusCode::CONFLICT, "the player is still finishing an earlier request; try again").await;
    }

    // Move the complete staging state into the job before releasing the mutex. A timed
    // out or canceled HTTP future therefore leaves `STAGING` empty until the control
    // task has finished every last borrow and restores the buffer itself.
    let Some(staging) = guard.take() else {
        return text(response_buffer, StatusCode::INTERNAL_SERVER_ERROR, "the staged upload was lost").await;
    };
    drop(guard);
    let queued = match queue_job(Job::LoadUpload { staging, length }) {
        Ok(queued) => queued,
        Err(Job::LoadUpload { staging, .. }) => {
            *STAGING.lock().await = Some(staging);
            return text(response_buffer, StatusCode::CONFLICT, "the player is still finishing an earlier request; try again").await;
        }
        Err(_) => return text(response_buffer, StatusCode::INTERNAL_SERVER_ERROR, "the upload job changed shape").await,
    };
    let outcome = wait_for_job(queued).await;
    drop(job_guard);
    match outcome {
        JobOutcome::Done => text(response_buffer, StatusCode::CREATED, "playing").await,
        JobOutcome::Failed(message) => text(response_buffer, StatusCode::CONFLICT, message).await,
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
/// fourteen match arms and the resulting task pool was 48 KiB **per worker**, which did
/// not fit in this chip's DRAM beside the WiFi driver. Keeping one compact response type
/// still bounds each worker's PSRAM claim and request-path depth. So every JSON and text
/// answer is the same [`Body`] type, the request body is extracted once as bytes and
/// decoded synchronously, and `write_to` appears exactly three times below.
struct FlatRoutes;

impl PathRouterService<ResponseBuffer> for FlatRoutes {
    async fn call_path_router_service<R: Read, W: ResponseWriter<Error = R::Error>>(
        &self, response_buffer: &ResponseBuffer, _path_parameters: (), path: Path<'_>, mut request: Request<'_, R>, response_writer: W,
    ) -> Result<ResponseSent, W::Error> {
        let method = request.parts.method();
        let path = path.encoded();

        // The three routes that do not answer with a `Body`, each with its own
        // `write_to` instantiation and no others.
        if (method, path) == ("GET", "/") {
            return picoserve::response::File::with_content_type_and_headers(picoserve::response::File::MIME_HTML, INDEX_HTML_GZ, GZIP_HEADERS)
                .call_request_handler_service(&(), (), request, response_writer)
                .await;
        }
        if (method, path) == ("GET", "/ws") {
            let upgrade = picoserve::from_request!(response_buffer, request, response_writer, WebSocketUpgrade);
            let response = upgrade.on_upgrade(TelemetrySocket);
            return response.write_to(request.body_connection.finalize().await?, response_writer).await;
        }
        if (method, path) == ("POST", "/api/modules") {
            let response = upload_response(&mut request, response_buffer).await;
            return response.write_to(request.body_connection.finalize().await?, response_writer).await;
        }

        // Everything else: one body extraction, one decode, one response type.
        let body = picoserve::from_request!(response_buffer, request, response_writer, &[u8]);
        let response = dispatch(response_buffer, method, path, body).await;
        response.write_to(request.body_connection.finalize().await?, response_writer).await
    }
}

/// Every endpoint whose answer is a [`Body`].
///
/// `body` is the request body, already read into picoserve's own buffer — every one of
/// these is a few dozen bytes of JSON, so decoding it here with `serde_json_core` rather
/// than through picoserve's `Json` extractor costs nothing and saves one monomorphised
/// extractor future per request type.
async fn dispatch<'a>(response_buffer: &'a ResponseBuffer, method: &str, path: &str, body: &[u8]) -> (StatusCode, Body<'a>) {
    match (method, path) {
        ("GET", "/api/status") => status_response(response_buffer).await,
        ("GET", "/api/modules") => modules_response(response_buffer).await,
        ("GET", "/api/upload-limits") => upload_limits_response(response_buffer).await,
        ("POST", "/api/play") => accepted(response_buffer, send_command(Command::Play)).await,
        ("POST", "/api/stop") => accepted(response_buffer, send_command(Command::Stop)).await,
        ("POST", "/api/next") => accepted(response_buffer, send_command(Command::Skip(1))).await,
        ("POST", "/api/previous") => accepted(response_buffer, send_command(Command::Skip(-1))).await,
        ("POST", "/api/seek") => match decode::<api::SeekRequest>(body) {
            Some(request) => accepted(response_buffer, send_command(Command::SeekOrder(request.order))).await,
            None => malformed(response_buffer).await,
        },
        ("POST", "/api/volume") => match decode::<api::VolumeRequest>(body) {
            Some(request) => accepted(response_buffer, send_command(Command::Volume(U0F16::from_bits(request.level)))).await,
            None => malformed(response_buffer).await,
        },
        ("POST", "/api/mute") => match decode::<api::MuteRequest>(body) {
            Some(request) => accepted(response_buffer, send_command(Command::Mute { channel: request.channel, muted: request.muted })).await,
            None => malformed(response_buffer).await,
        },
        ("POST", "/api/modules/select") => match decode::<api::SlotRequest>(body) {
            Some(request) => job_response(response_buffer, run_job(Job::Select(request.id)).await).await,
            None => malformed(response_buffer).await,
        },
        ("POST", "/api/modules/store") => match decode::<api::SlotRequest>(body) {
            Some(request) => job_response(response_buffer, run_job(Job::Store(request.id)).await).await,
            None => malformed(response_buffer).await,
        },
        ("POST", "/api/reprovision") => job_response(response_buffer, run_job(Job::ForgetWifi).await).await,
        _ => text(response_buffer, StatusCode::NOT_FOUND, "no such endpoint").await,
    }
}

/// Decode one small JSON request body.
fn decode<'a, T: serde::Deserialize<'a>>(body: &'a [u8]) -> Option<T> {
    serde_json_core::from_slice::<T>(body).ok().map(|(value, _)| value)
}

/// The answer to a body that would not decode.
async fn malformed(response_buffer: &ResponseBuffer) -> (StatusCode, Body<'_>) {
    text(response_buffer, StatusCode::BAD_REQUEST, "that request body is not the JSON this endpoint expects").await
}

/// 204 when the command was queued, 503 when the channel was full.
async fn accepted(response_buffer: &ResponseBuffer, queued: bool) -> (StatusCode, Body<'_>) {
    if queued {
        text(response_buffer, StatusCode::NO_CONTENT, "").await
    } else {
        text(response_buffer, StatusCode::SERVICE_UNAVAILABLE, "the player is busy; try again").await
    }
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

/// Spawn the web workers with their future bodies in PSRAM.
pub fn start(spawner: Spawner, stack: Stack<'static>, arena: &mut psram::Arena) -> Result<(), &'static str> {
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
    let remaining_before = arena.remaining();
    for id in 0..WEB_WORKER_COUNT {
        crate::psram_task::spawn_in_psram(arena, &spawner, move || web_task(id, stack, config)).map_err(|error| match error {
            crate::psram_task::SpawnError::OutOfPsram => "not enough PSRAM for the web worker futures",
            crate::psram_task::SpawnError::TaskStorageBusy => "a web worker task header was unexpectedly busy",
        })?;
    }
    println!("PSRAM web workers claimed {} bytes, {} left unclaimed", remaining_before - arena.remaining(), arena.remaining());
    Ok(())
}

/// One connection's worth of server.
async fn web_task(id: usize, stack: Stack<'static>, config: &'static picoserve::Config) {
    let mut receive_buffer = [0u8; TCP_BUFFER_BYTES];
    let mut transmit_buffer = [0u8; TCP_BUFFER_BYTES];
    let mut http_buffer = [0u8; HTTP_BUFFER_BYTES];
    let response_buffer = ResponseBuffer::new([0u8; BODY_BYTES]);
    let application = picoserve::Router::<_, ResponseBuffer>::new().nest_service("", FlatRoutes).with_state(&response_buffer);

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
