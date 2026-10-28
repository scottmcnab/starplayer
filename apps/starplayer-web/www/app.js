import { createBackgroundOutput, backgroundPlaybackDefault } from './background-output.js';
import { createMediaSession } from './media-session.js';
import { createPlaylistControls } from './playlist-controls.js';
import { createPlaybackController } from './playback-controller.js';
import { createPlaylistPlayback } from './playlist-playback.js';
import { createLibraryView } from './library-view.js';
import { createPlaylist, libraryLocator, wirePlaylistList } from './playlist.js';
import { wireLibraryControls } from './library-controls.js';
import initLoader, * as Loader from './starplayer_web.js';

const Ring = globalThis.StarPlayerRing;
const HOST_WASM_URL = 'starplayer_host_wasm_bg.wasm';
const WORKLET_URL = 'starplayer-worklet.js';
const PROCESSOR_NAME = 'starplayer-player';
const MIXER_STORAGE_KEY = 'starplayer.output-and-mixer.v1';
const DEFAULT_MIXER_MODE = 0x0000_0202;
const SONG_FADE_SECONDS = 5;
/// How long the engine must sit idle, with no load pending, before the background keep-alive pauses.
const KEEP_ALIVE_IDLE_MS = 1000;
/// How long after a seek, reload or rebuild the Media Session position is read back from telemetry.
const MEDIA_POSITION_SETTLE_MS = 250;
const TIME_DISPLAY_STORAGE_KEY = 'starplayer.time-display.v1';
const PATTERN_WINDOW_ROWS = 13;
const PATTERN_CELL_BYTES = 5;
// One pixel column per tap bucket, so the trace is drawn rather than resampled. 132
// buckets is 528 output frames, about 11 ms at 48 kHz — a little over four render quanta.
const SCOPE_CANVAS_WIDTH = 132;
const SCOPE_CANVAS_HEIGHT = 26;

// The English effect names are the engine's own table, read once from the page-side wasm
// instance rather than transcribed here. `EffectDisplay.name` is a `&'static str` that
// cannot cross the wasm boundary in a packed snapshot, so the table crosses instead — one
// string at start-up, and each channel's effect column stays two scalar words in the
// snapshot instead of a string that would have to be copied per tick.
const EFFECT_NAMES = new Map();
const NOTE_NAMES = ['C-', 'C#', 'D-', 'D#', 'E-', 'F-', 'F#', 'G-', 'G#', 'A-', 'A#', 'B-'];

const byId = (id) => document.getElementById(id);
const elements = {
    backgroundPlayback: byId('background-playback'), backgroundNote: byId('background-playback-note'),
    startAudio: byId('start-audio'), filePicker: byId('file-picker'), fixturePicker: byId('fixture-picker'),
    loadFixture: byId('load-fixture'), urlForm: byId('url-form'), url: byId('url'), dropZone: byId('drop-zone'),
    error: byId('error'), message: byId('message'), transportChip: byId('transport-chip'), title: byId('title'),
    moduleDetail: byId('module-detail'), order: byId('order'), pattern: byId('pattern'), row: byId('row'),
    speed: byId('speed'), bpm: byId('bpm'), previous: byId('previous'), play: byId('play'), stop: byId('stop'),
    pause: byId('pause'), overlapEnable: byId('note-overlap-enable'), uniqueNotes: byId('unique-notes-across-channels'), referenceNote: byId('reference-note'), stopPreview: byId('stop-preview'), allSoundOff: byId('all-sound-off'), inputDestination: byId('input-destination'),
    next: byId('next'), seekOrder: byId('seek-order'), volume: byId('volume'), volumeValue: byId('volume-value'),
    progress: byId('progress'), elapsed: byId('elapsed'), duration: byId('duration'), repeat: byId('repeat'),
    voices: byId('voices'), masterPeak: byId('master-peak'), channelBody: byId('channel-body'), patternTable: byId('pattern-table'),
    instrumentCount: byId('instrument-count'), instrumentList: byId('instrument-list'), instrumentSelectAll: byId('instrument-select-all'), commandTransport: byId('command-transport'),
    telemetryTransport: byId('telemetry-transport'), scopeTransport: byId('scope-transport'),
    quantum: byId('quantum'), memory: byId('memory'),
    dropped: byId('dropped'), retired: byId('retired'), health: byId('health'), forceFallback: byId('force-fallback'),
    archivePicker: byId('archive-picker'), archivePickerName: byId('archive-picker-name'), archiveEntries: byId('archive-entries'),
    archiveLoad: byId('archive-load'), archiveCancel: byId('archive-cancel'),
    archiveTracks: byId('archive-tracks'), loadArchiveTrack: byId('load-archive-track'),
    engineMode: byId('engine-mode'), outputStatus: byId('output-status'), sampleRateStatus: byId('sample-rate-status'),
    contextState: byId('context-state'), baseLatency: byId('base-latency'), outputLatency: byId('output-latency'),
    channelStatus: byId('channel-status'), maxChannelCount: byId('max-channel-count'), sinkStatus: byId('sink-status'),
    workletRate: byId('worklet-rate'), outputEngineMode: byId('output-engine-mode'), sampleRate: byId('sample-rate'),
    applySampleRate: byId('apply-sample-rate'), outputDeviceLabel: byId('output-device-label'), outputDevice: byId('output-device'),
    applyOutputDevice: byId('apply-output-device'), chooseOutputDevice: byId('choose-output-device'), outputDeviceNote: byId('output-device-note'),
    outputChannels: byId('output-channels'), applyChannels: byId('apply-channels'), mixerPath: byId('mixer-path'),
    mixerInterpolator: byId('mixer-interpolator'), mixerDepth: byId('mixer-depth'), mixerDither: byId('mixer-dither'),
    applyMixer: byId('apply-mixer'), modHeadphonePanning: byId('mod-headphone-panning'),
    // Load-time sample enhancement (M10-W4). The checkboxes themselves are built into this
    // container from the wasm host's own `enhancements_json()`.
    enhancementOptions: byId('enhancement-options'),
    // The disclosure every load option lives behind, its one-line summary, and the
    // experimental tracker-speed bias.
    loadOptionsDisclosure: byId('load-options-disclosure'), loadOptionsSummary: byId('load-options-summary'),
    speedAdjust: byId('speed-adjust'),
    // Insert effects (M7-H7).
    effectsTarget: byId('effects-target'), effectsSlots: byId('effects-slots'), effectsReset: byId('effects-reset'),
    // Live input (task E6).
    liveInputStatus: byId('live-input-status'), keyboardEnable: byId('keyboard-enable'), eventLead: byId('event-lead'),
    midiInput: byId('midi-input'), enableMidi: byId('enable-midi'), midiNote: byId('midi-note'),
    keyboardOctave: byId('keyboard-octave'), keyboardInstrument: byId('keyboard-instrument'), midiEvents: byId('midi-events'),
};

const state = {
    library: null,
    context: null,
    node: null,
    startPromise: null,
    commandRing: null,
    telemetry: null,
    scope: null,
    latest: null,
    mediaWasPlaying: false,
    mediaPositionDue: null,
    keepAliveIdleSince: null,
    /// The newest scope window on the `postMessage` fallback, where it rides the batched
    /// telemetry message rather than shared memory.
    latestScope: null,
    /// The refresh already drawn, so an animation frame that beat the worklet redraws
    /// nothing.
    drawnScopeGeneration: -1,
    fallbackCommands: [],
    pendingLoads: new Map(),
    pendingStates: new Map(),
    sessionClock: 0n,
    buildClock: 0n,
    interactiveReady: false,
    sessionSettingsRevision: 0,
    lastTransportOpcode: null,
    transportRevision: 0,
    liveRecoveryPending: false,
    selectedInstrument: null,
    instrumentTableInstruments: null,
    instrumentRows: [],
    letNotesRing: [],
    // Page intent survives installations; applied policy is scoped to an audio identity.
    overlapEnabled: false,
    uniqueNotesAcrossChannels: false,
    policyRevision: 0,
    appliedPolicies: null,
    policyEdits: [],
    policyIntentRevisions: {},
    previewInstrument: null,
    previewLooping: false,
    referenceNote: 60,
    nextRequestId: 1,
    metadata: null,
    channelRows: [],
    patternRows: [],
    patternChannelCount: 0,
    lastPatternKey: '',
    activationMemoryBytes: 0,
    loadCount: 0,
    retiredSeen: 0,
    currentModuleBytes: null,
    wasmModule: null,
    wasmBytes: null,
    workletSampleRate: null,
    requestedSampleRate: '',
    rateReport: '',
    activeModeWire: DEFAULT_MIXER_MODE,
    outputChannelCount: 2,
    outputDevices: new Map(),
    archiveChoices: [],
    retainedArchive: null,
    archivePickerResolve: null,
    archivePreviousFocus: null,
    moduleRevision: 0,
    // M10-W4: the load options actually active in the currently loaded module — not
    // necessarily what the checkboxes show right now, since a change is only applied once
    // its reload finishes. `enhancementFlags` is the applied word `enhancements_json()`'s
    // bits describe; `sinc4x` may have been silently narrowed by the host's frame budget
    // (`build_enhancement`), which is why `moduleLoaded`/`loadOptionsApplied` also carry
    // the factor that actually ran.
    activeLoadOptions: { headphoneFriendlyModPanning: false, enhancementFlags: 0, speedAdjust: 0 },
    loadOptionsReloadInProgress: false,
    loadOptionsReloadRequested: null,
    /// Parsed from the worklet's first `ready` message: `enhancements_json()`, in
    /// `CATALOGUE` order — `[{bit, id, label, description}]`. Never transcribed by hand,
    /// so the load panel cannot drift from what `enhancementFlags` actually accepts.
    enhancements: [],
    /// Whether the enhancement checkboxes have been built yet (`state.enhancements`
    /// arrives with the worklet's `ready` message, which can race the panel's own first
    /// render — the same race `effectsSlotsBuilt` guards against).
    enhancementCheckboxesBuilt: false,
    /// `enhancementFlags` read from `localStorage` before the checkboxes existed to carry
    /// it, applied to them the moment `ensureEnhancementCheckboxes` builds them.
    pendingEnhancementFlags: 0,
    scrubbing: false,
    showRemaining: readShowRemaining(),
    pendingSeekFrame: null,
    pendingSeekSequence: 0,
    // ── live input (task E6) ────────────────────────────────────────────────────────
    /// Whether the worklet is jamming live input over the module.
    liveInputActive: false,
    /// A `midiInput` message already sent and not yet answered, so a fast double-click
    /// does not interleave two installs on the port's FIFO.
    liveInputPending: false,
    pendingInserts: new Map(),
    /// The lead the worklet reported, in frames and milliseconds.
    eventLeadFrames: 0,
    eventLeadMillis: 0,
    /// `navigator.requestMIDIAccess`'s answer, once it has been asked for.
    midiAccess: null,
    /// The port the selector currently has open, so it can be closed when another is
    /// chosen.
    midiPort: null,
    /// The keyboard's own octave and program, and which physical keys are down — so a
    /// note-off goes to the note the key started, even after the octave has moved.
    keyboardOctave: 4,
    keyboardInstrument: 0,
    keysDown: new Map(),
    eventsSent: 0,
    // ── insert effects (M7-H7) ──────────────────────────────────────────────────────
    /// Every effect this build can install, parsed from the wasm host's own
    /// `effects_json()`: `[{name, wire, params: [{name, unit, min, max, default}]}]`, in
    /// `InsertKind::ALL` order. Never transcribed by hand, so the panel cannot drift from
    /// what `install_insert` and `OPCODE_INSERT_PARAM` actually accept.
    effects: [],
    /// The page's own belief of what is installed, per `"target:slot"` key — nothing reads
    /// the engine's chain back over the wire, so this is the only source of truth for the
    /// panel's own controls (the same one-way arrangement `InsertLayout` keeps natively).
    insertLayout: new Map(),
    /// Channels the target `<select>` was last built for, so it is only rebuilt when the
    /// module's own channel count changes.
    effectsChannelCount: -1,
    /// Whether the four slot rows have been built yet (`state.effects` arrives with the
    /// worklet's `ready` message, which can race the panel's own first render).
    effectsSlotsBuilt: false,
};

const loaderReady = initLoader().then(loadEffectNames);

function showMessage(message) {
    elements.message.textContent = message;
}

function showError(error) {
    elements.error.textContent = error && error.message ? error.message : String(error);
    elements.error.hidden = false;
}

function clearError() {
    elements.error.hidden = true;
    elements.error.textContent = '';
}

function sharedMemoryAvailable() {
    return typeof SharedArrayBuffer === 'function' && globalThis.crossOriginIsolated === true;
}

function mixerModeFromControls(channels = Number(elements.outputChannels.value) === 1 ? 1 : 2) {
    const path = elements.mixerPath.value === 'fixed' ? 1 : 0;
    const interpolator = { nearest: 0, linear: 1, cubic: 2, sinc: 3 }[elements.mixerInterpolator.value] ?? 1;
    const depth = { f32: 0, i32: 1, i24: 2, i16: 3, i8: 4 }[elements.mixerDepth.value] ?? 0;
    const dither = elements.mixerDither.value === 'tpdf' ? 1 : 0;
    return (path | (interpolator << 1) | (depth << 3) | (dither << 6) | (channels << 8)) >>> 0;
}

function describeMixerMode(wire) {
    if (!Number.isInteger(wire)) return '—';
    const path = (wire & 1) === 0 ? 'float' : 'fixed';
    const interpolators = ['nearest', 'linear', 'cubic', 'sinc'];
    const depths = ['32-bit float', '32-bit int', '24-bit', '16-bit', '8-bit'];
    const interpolator = interpolators[(wire >>> 1) & 3] ?? 'unknown';
    const depth = depths[(wire >>> 3) & 7] ?? 'unknown';
    const dither = (wire & (1 << 6)) === 0 ? '' : ' · TPDF';
    const channels = ((wire >>> 8) & 0xFF) === 1 ? 'mono' : 'stereo';
    return `${path} · ${interpolator} · ${depth}${dither} · ${channels}`;
}

function selectStoredValue(select, value) {
    if ([...select.options].some((option) => option.value === String(value))) select.value = String(value);
}

function restorePreferences() {
    try {
        const saved = JSON.parse(localStorage.getItem(MIXER_STORAGE_KEY) || 'null');
        if (!saved || typeof saved !== 'object') return;
        selectStoredValue(elements.sampleRate, saved.sampleRate ?? '');
        selectStoredValue(elements.outputChannels, saved.channels ?? '2');
        selectStoredValue(elements.mixerPath, saved.path ?? 'float');
        selectStoredValue(elements.mixerInterpolator, saved.interpolator ?? 'linear');
        selectStoredValue(elements.mixerDepth, saved.depth ?? 'f32');
        selectStoredValue(elements.mixerDither, saved.dither ?? 'off');
        if (typeof saved.backgroundPlayback === 'boolean') elements.backgroundPlayback.checked = saved.backgroundPlayback;
        if (typeof saved.repeat === 'boolean') elements.repeat.checked = saved.repeat;
        elements.modHeadphonePanning.checked = saved.headphoneFriendlyModPanning === true;
        // M10-W4: the enhancement checkboxes do not exist yet (they are built from the
        // worklet's first `ready` message), so the value is held until
        // `ensureEnhancementCheckboxes` can apply it.
        state.pendingEnhancementFlags = Number(saved.enhancementFlags) >>> 0 || 0;
        writeEnhancementFlagsToControls(state.pendingEnhancementFlags);
        elements.speedAdjust.value = String(clampSpeedAdjust(saved.speedAdjust));
        // Remembered, so a listener comparing enhancers finds the panel as they left it.
        // Whether it is open or closed, `renderLoadOptionsSummary` names every option that
        // is not at its default in the summary line — an option that is silently on and
        // out of sight is the one thing this disclosure must not do.
        elements.loadOptionsDisclosure.open = saved.loadOptionsOpen === true;
        if (typeof saved.sinkId === 'string') elements.outputDevice.dataset.savedSinkId = saved.sinkId;
    } catch (_) {
        // Storage may be disabled or contain data from a broken older development build.
    }
}

function persistPreferences() {
    try {
        localStorage.setItem(MIXER_STORAGE_KEY, JSON.stringify({
            repeat: elements.repeat.checked,
            sampleRate: elements.sampleRate.value,
            sinkId: state.context && 'sinkId' in state.context ? state.context.sinkId : (elements.outputDevice.dataset.savedSinkId || elements.outputDevice.value),
            backgroundPlayback: elements.backgroundPlayback.checked,
            channels: elements.outputChannels.value,
            path: elements.mixerPath.value,
            interpolator: elements.mixerInterpolator.value,
            depth: elements.mixerDepth.value,
            dither: elements.mixerDither.value,
            // The v1 record keeps this exact key so an older saved object still restores
            // (`headless.mjs`'s v1-record test asserts on it), even though the checkbox it
            // names is now one of several inputs a `loadOptionsFromControls()` record
            // carries.
            headphoneFriendlyModPanning: elements.modHeadphonePanning.checked,
            enhancementFlags: enhancementFlagsFromControls(),
            speedAdjust: speedAdjustFromControls(),
            loadOptionsOpen: elements.loadOptionsDisclosure.open,
        }));
    } catch (_) {
        // A private or storage-blocked page still gets fully working in-memory controls.
    }
}

// ── load options (M10-W4) ────────────────────────────────────────────────────────────
//
// A load option is anything applied when the module is prepared: the MOD-only panning
// checkbox, the sample-enhancement checkboxes built from `enhancements_json()`, and the
// experimental speed bias. `loadOptionsFromControls()` is the one record
// `activateModuleOnNode` and `applyLoadOptions` both send; nothing about a specific
// enhancer's id, label or description is written here — the checkboxes carry their bit in
// `dataset.bit`, read straight off `starplayer-enhance::CATALOGUE` through the wasm host.
//
// The speed bias is a load option rather than a live command for one reason: the song's
// timeline — its length, and every seek against it — is measured by the scan that runs
// during the load. Applying the bias without rescanning would leave the progress slider
// describing a song nobody is playing.

/// The range the "Adjust speed" select offers. A tracker speed is 1..255, so anything
/// wider than a few ticks either way is a different
/// song rather than a corrected one.
const SPEED_ADJUST_LIMIT = 6;

/// Normalize the selected or saved value to an integer within range. An absent or
/// unparseable value is zero: the control's own default, and canonical playback.
function clampSpeedAdjust(value) {
    const number = Math.trunc(Number(value));
    if (!Number.isFinite(number)) return 0;
    return Math.min(SPEED_ADJUST_LIMIT, Math.max(-SPEED_ADJUST_LIMIT, number));
}

function speedAdjustFromControls() {
    return clampSpeedAdjust(elements.speedAdjust.value);
}

function enhancementFlagsFromControls() {
    let flags = 0;
    for (const input of elements.enhancementOptions.querySelectorAll('input[type=checkbox]')) {
        if (input.checked) flags |= (1 << Number(input.dataset.bit));
    }
    return flags >>> 0;
}

function writeEnhancementFlagsToControls(flags) {
    for (const input of elements.enhancementOptions.querySelectorAll('input[type=checkbox]')) {
        input.checked = (flags & (1 << Number(input.dataset.bit))) !== 0;
    }
}

function loadOptionsFromControls() {
    return {
        headphoneFriendlyModPanning: elements.modHeadphonePanning.checked,
        enhancementFlags: enhancementFlagsFromControls(),
        speedAdjust: speedAdjustFromControls(),
    };
}

function writeLoadOptionsToControls(options) {
    elements.modHeadphonePanning.checked = options.headphoneFriendlyModPanning;
    writeEnhancementFlagsToControls(options.enhancementFlags);
    elements.speedAdjust.value = String(clampSpeedAdjust(options.speedAdjust));
}

/// What the closed disclosure says about itself. A summary is the whole point of hiding
/// these: a listener must be able to see that an option is on without opening the panel.
function renderLoadOptionsSummary() {
    const chosen = [];
    if (elements.modHeadphonePanning.checked) chosen.push('headphone panning');
    for (const enhancement of state.enhancements) {
        const input = byId(`enhancement-${enhancement.id}`);
        if (input && input.checked) chosen.push(enhancement.label.toLowerCase());
    }
    const adjust = speedAdjustFromControls();
    if (adjust !== 0) chosen.push(`speed ${adjust > 0 ? '+' : ''}${adjust}`);
    elements.loadOptionsSummary.textContent = chosen.length === 0 ? 'default' : chosen.join(' · ');
}

/** Build one checkbox per `enhancements_json()` entry, the first time `state.enhancements`
 *  is available. Mirrors `ensureEffectsSlots`'s race guard against the worklet's `ready`
 *  message. */
function ensureEnhancementCheckboxes() {
    if (state.enhancementCheckboxesBuilt || state.enhancements.length === 0) return;
    state.enhancementCheckboxesBuilt = true;
    elements.enhancementOptions.replaceChildren();
    for (const enhancement of state.enhancements) {
        const label = document.createElement('label');
        label.className = 'mod-panning-option';
        const input = document.createElement('input');
        input.type = 'checkbox';
        input.id = `enhancement-${enhancement.id}`;
        input.dataset.bit = String(enhancement.bit);
        const span = document.createElement('span');
        const strong = document.createElement('strong');
        strong.textContent = enhancement.label;
        const small = document.createElement('small');
        small.textContent = enhancement.description;
        span.append(strong, small);
        label.append(input, span);
        elements.enhancementOptions.append(label);
        input.addEventListener('change', () => applyLoadOptions('enhancementFlags').catch(showError));
    }
    writeEnhancementFlagsToControls(state.pendingEnhancementFlags);
    renderLoadOptionsSummary();
}

/// The enhancer id `CATALOGUE` gives the sinc upsampler's checkbox, so a reduced
/// `appliedEnhancementFactor` can be explained without transcribing anything about the
/// other enhancers.
function enhancementUpsampleBit() {
    const entry = state.enhancements.find((enhancement) => enhancement.id === 'sinc4x');
    return entry ? entry.bit : null;
}

/// `null` when nothing needs explaining; otherwise a sentence for the status line, because
/// `build_enhancement`'s frame-budget fallback silently narrowed the requested factor.
function describeEnhancementReduction(requestedFlags, appliedFactor) {
    const upsampleBit = enhancementUpsampleBit();
    if (upsampleBit === null || (requestedFlags & (1 << upsampleBit)) === 0) return null;
    if (!Number.isFinite(appliedFactor) || appliedFactor >= 4) return null;
    return appliedFactor <= 1
        ? 'the sinc upsampler did not fit the frame budget, so the module loaded without it'
        : `the sinc upsampler was reduced to ${appliedFactor}x to fit the frame budget`;
}

function formatRate(value) {
    return Number.isFinite(value) ? `${Math.round(value).toLocaleString()} Hz` : '—';
}

function formatLatency(value) {
    return Number.isFinite(value) ? `${(value * 1000).toFixed(2)} ms` : 'unavailable';
}

/// `m:ss`, or `h:mm:ss` once the song runs past an hour. `--:--` covers both a song whose
/// length is not yet known (`frames` is `null`) and a worklet that has not reported its
/// sample rate yet.
function formatTime(frames, rate) {
    if (frames === null || frames === undefined || !Number.isFinite(rate) || rate <= 0) return '--:--';
    const totalSeconds = Math.max(0, Math.floor(frames / rate));
    const hours = Math.floor(totalSeconds / 3600);
    const minutes = Math.floor((totalSeconds % 3600) / 60);
    const seconds = totalSeconds % 60;
    return hours > 0
        ? `${hours}:${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}`
        : `${minutes}:${String(seconds).padStart(2, '0')}`;
}

/// The elapsed readout, or — after a click on either time readout, as in VLC — the time
/// still to play as `-m:ss`. Remaining needs a known length; without one it falls back to
/// elapsed rather than showing a dash with nothing behind it.
function formatElapsed(frame, lengthFrames, rate) {
    if (state.showRemaining && lengthFrames !== null) {
        return `-${formatTime(Math.max(0, lengthFrames - frame), rate)}`;
    }
    return formatTime(frame, rate);
}

function readShowRemaining() {
    try {
        return localStorage.getItem(TIME_DISPLAY_STORAGE_KEY) === 'remaining';
    } catch {
        return false;
    }
}

function toggleTimeDisplay() {
    state.showRemaining = !state.showRemaining;
    try {
        localStorage.setItem(TIME_DISPLAY_STORAGE_KEY, state.showRemaining ? 'remaining' : 'elapsed');
    } catch {
        // Private browsing or storage disabled: the choice still holds for this page load.
    }
    if (state.scrubbing) {
        setText(elements.elapsed, formatElapsed(Number(elements.progress.value), progressLengthFrames(), state.workletSampleRate));
    } else if (state.latest) {
        updateProgress(state.latest);
    }
}

/// The slider's length as the page last set it, or `null` while the song's length is unknown.
function progressLengthFrames() {
    return elements.progress.disabled ? null : Number(elements.progress.max);
}

function updateOutputPanel() {
    const context = state.context;
    // Before the first `Start audio` there is no context to have asked anything of, so the
    // "requested" half reports the pending selection instead of a request never made.
    const pending = context === null ? elements.sampleRate.value : state.requestedSampleRate;
    const requested = pending === '' ? 'device default' : formatRate(Number(pending));
    elements.sampleRateStatus.textContent = state.rateReport || `${requested} / ${context ? formatRate(context.sampleRate) : '—'}`;
    elements.contextState.textContent = context?.state ?? '—';
    elements.baseLatency.textContent = context ? formatLatency(context.baseLatency) : '—';
    elements.outputLatency.textContent = context ? formatLatency(context.outputLatency) : '—';
    const channels = context ? state.outputChannelCount : (Number(elements.outputChannels.value) === 1 ? 1 : 2);
    elements.channelStatus.textContent = context ? `${channels === 1 ? 'mono' : 'stereo'} · destination ${context.destination.channelCount}` : (channels === 1 ? 'mono' : 'stereo');
    elements.maxChannelCount.textContent = context ? String(context.destination.maxChannelCount) : '—';
    elements.workletRate.textContent = state.workletSampleRate === null ? '—' : formatRate(state.workletSampleRate);
    const description = describeMixerMode(state.activeModeWire);
    elements.engineMode.textContent = description;
    elements.outputEngineMode.textContent = description;
    elements.outputStatus.textContent = context ? `${formatRate(context.sampleRate)} · ${context.state}` : 'audio asleep';
    const sinkId = context && 'sinkId' in context ? context.sinkId : '';
    const device = state.outputDevices.get(sinkId);
    elements.sinkStatus.textContent = device?.label || (sinkId ? `device ${sinkId.slice(0, 8)}…` : 'default');
}

function installGraph(graph) {
    // A rebuilt graph is a new Rust host with no live-input source. `activateLoadedModule`
    // asks for it back once the module has been reinstalled on it.
    releaseHeldKeys(false);
    state.liveInputActive = false;
    state.liveInputPending = false;
    state.node = graph.node;
    if (graph.node.starplayerAppliedPolicies) state.appliedPolicies = graph.node.starplayerAppliedPolicies;
    state.policyEdits = (state.policyEdits ?? []).filter(edit => Ring.sameIdentity(edit.identity, graph.node.starplayerIdentity));
    state.pendingInserts.clear();
    state.interactiveReady = !!graph.node.starplayerIdentity && graph.node.starplayerCapabilities === 3;
    if (graph.node.starplayerWarmup) { graph.node.disconnect(graph.node.starplayerWarmup); graph.node.starplayerWarmup.disconnect(); graph.node.starplayerWarmup = null; }
    state.commandRing = graph.commandRing;
    state.telemetry = graph.telemetry;
    state.scope = graph.scope;
    state.latestScope = null;
    state.drawnScopeGeneration = -1;
    state.workletSampleRate = graph.node.starplayerSampleRate ?? graph.context.sampleRate;
    state.eventLeadFrames = graph.node.starplayerLeadFrames ?? 0;
    state.eventLeadMillis = graph.node.starplayerLeadMillis ?? 0;
    state.outputChannelCount = graph.channels;
    state.liveInputActive = state.interactiveReady && (elements.keyboardEnable.checked || state.midiPort !== null);
    updateLiveInputPanel();
    if (graph.node.starplayerRenderQuantum) elements.quantum.textContent = `${graph.node.starplayerRenderQuantum} frames`;
}

async function prepareNode(context, channels, mixerMode, addModule) {
    if (!context.audioWorklet) {
        throw new Error('AudioWorklet is unavailable here: the player must be served over https or from localhost (a secure context).');
    }
    if (addModule) await context.audioWorklet.addModule(WORKLET_URL);
    const shared = sharedMemoryAvailable() && !elements.forceFallback.checked;
    const commandRing = shared ? Ring.createCommandRing(Ring.INTERACTIVE_WIRE_VERSION) : null;
    const telemetry = shared ? Ring.createTelemetry() : null;
    const scope = shared ? Ring.createScope() : null;
    const processorOptions = {
        channelCount: channels,
        interactiveVersion: Ring.INTERACTIVE_WIRE_VERSION,
        seedSession: state.sessionClock, seedBuild: ++state.buildClock,
        mixerMode,
        wasmModule: state.wasmModule,
        commandRing: commandRing ? commandRing.buffer : null,
        telemetry: telemetry ? telemetry.buffer : null,
        scope: scope ? scope.buffer : null,
    };
    const node = createNode(context, processorOptions, state.wasmBytes, channels);
    node.port.onmessage = (event) => onWorkletMessage(event.data, node);
    // A muted connected candidate renders prepared loads/replay before acknowledgement.
    // Disconnected worklets are not guaranteed to receive process() callbacks.
    const warmup = context.createGain();
    warmup.gain.value = 0;
    node.connect(warmup); warmup.connect(context.destination);
    node.starplayerWarmup = warmup;
    node.onprocessorerror = () => {
        if (node === state.node) showError('The AudioWorklet processor stopped unexpectedly. Reload the page to restart it.');
    };
    return { context, node, commandRing, telemetry, scope, channels };
}

async function startAudio(playRequested = false) {
    if (state.node !== null) {
        if (document.hidden && playback.stopped && !playRequested) return;
        if (!playback.stopped || playRequested) backgroundOutput.start();
        await state.context.resume();
        return;
    }
    if (state.startPromise !== null) {
        return state.startPromise;
    }

    // Construct and resume before the first await. That keeps this function inside the
    // originating tap on iOS Safari, whose audio unlock rules are stricter than Chromium.
    state.requestedSampleRate = elements.sampleRate.value;
    const contextOptions = { latencyHint: 'interactive' };
    if (state.requestedSampleRate !== '') contextOptions.sampleRate = Number(state.requestedSampleRate);
    state.context = new AudioContext(contextOptions);
    const unlock = state.context.resume();
    backgroundOutput.start();
    state.startPromise = (async () => {
        clearError();
        elements.startAudio.disabled = true;
        showMessage('Starting the AudioWorklet…');
        const response = await fetch(HOST_WASM_URL);
        if (!response.ok) {
            throw new Error(`Could not fetch the audio engine (${response.status} ${response.statusText}).`);
        }
        const wasmBytes = await response.arrayBuffer();
        const wasmModule = await WebAssembly.compile(wasmBytes);
        state.wasmBytes = wasmBytes;
        state.wasmModule = wasmModule;
        await unlock;

        const channels = Number(elements.outputChannels.value) === 1 ? 1 : 2;
        const graph = await prepareNode(state.context, channels, mixerModeFromControls(), true);
        installGraph(graph);
        state.node.connect(state.context.destination);
        state.context.addEventListener('statechange', updateOutputPanel);

        elements.commandTransport.textContent = state.commandRing ? 'SharedArrayBuffer SPSC ring' : 'postMessage, batched once per frame';
        elements.telemetryTransport.textContent = state.telemetry ? 'SharedArrayBuffer seqlock' : `postMessage every ${Ring.TELEMETRY_FALLBACK_QUANTA} quanta`;
        elements.scopeTransport.textContent = state.scope
            ? 'SharedArrayBuffer window, read in place'
            : `${Ring.SCOPE_WINDOW_BUCKETS} buckets per channel on the telemetry message`;
        elements.transportChip.textContent = 'audio ready';
        elements.transportChip.classList.add('live');
        elements.startAudio.textContent = 'Audio ready';
        elements.forceFallback.disabled = true;
        state.rateReport = `${state.requestedSampleRate === '' ? 'device default' : formatRate(Number(state.requestedSampleRate))} / ${formatRate(state.context.sampleRate)}`;
        await refreshOutputDevices();
        const savedSinkId = elements.outputDevice.dataset.savedSinkId;
        if (savedSinkId && typeof state.context.setSinkId === 'function') {
            await state.context.setSinkId(savedSinkId).catch((error) => {
                elements.outputDeviceNote.textContent = `Saved output was not available (${error.name || error.message}).`;
            });
        }
        updateOutputPanel();
        showMessage('Audio is ready. Load a module.');
    })().catch(async (error) => {
        showError(error);
        elements.startAudio.disabled = false;
        elements.startAudio.textContent = 'Retry audio';
        if (state.context !== null) {
            await state.context.close().catch(() => {});
        }
        state.context = null;
        state.node = null;
        state.startPromise = null;
        throw error;
    });
    return state.startPromise;
}

function createNode(context, processorOptions, wasmBytes, channels) {
    const options = {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [channels],
        processorOptions,
    };
    try {
        return new AudioWorkletNode(context, PROCESSOR_NAME, options);
    } catch (error) {
        options.processorOptions = { ...processorOptions, wasmModule: null, wasmBytes };
        return new AudioWorkletNode(context, PROCESSOR_NAME, options);
    }
}

export async function playLibraryRow(row) {
    if (!row.playable || !state.library) return;
    const library = state.library;
    const label = `${row.name}${row.title ? ` — ${row.title}` : ''}${row.folder ? ` (${row.folder})` : ''}`;
    const result = await playback.play(libraryLocator(row, library.identity), { origin: 'library', folder: row.folder, label });
    if (result.status === 'played' && library === state.library) libraryView.setPlaying(row.id);
    else if (result.status === 'failed') reportLoadFailure(row.name, result.error);
}

async function loadBuffer(buffer, label, origin = 'file') {
    const result = await playback.play({ kind: 'bytes', ...(typeof buffer === 'function' ? { read: buffer } : { buffer }), label }, { origin });
    if (result.status === 'failed') reportLoadFailure(label, result.error);
}

// Archive selection is part of the same intent as the file/URL read. A newer load
// closes the old picker and the guard prevents its continuation from activating.
async function preparePlayback(buffer, label, origin, context) {
    clearError();
    await loaderReady;
    if (!context.isCurrent()) return null;
    const inputBytes = new Uint8Array(buffer);
    if (Loader.is_archive(inputBytes)) {
        const entries = parseArchiveModules(Loader.archive_modules(inputBytes));
        if (!entries.length) throw new Error(`No supported modules were found inside ${label}.`);
        const archive = retainArchive(label, inputBytes, entries);
        const choice = entries.length === 1 ? entries[0] : await chooseArchiveEntry(entries, label);
        if (!context.isCurrent() || !choice) return null;
        buffer = Uint8Array.from(Loader.archive_extract(archive.bytes, choice.index)).buffer;
        label = `${choice.name} (from ${archive.label})`;
        context.archiveLabel = archive.label;
        selectRetainedArchiveTrack(archive, choice);
    }
    return activateLoadedModule(buffer, label, context);
}

/// Everything that happens once a module's own bytes are known: validate them on the
/// page, refresh the format's effect-name table, then activate on the worklet and play.
/// Every source arrives through the controller's guarded adapter. Returns null when a
/// later intent or graph change has superseded this activation.
async function activateLoadedModule(moduleBuffer, moduleLabel, context) {
    if (!context.isCurrent()) return null;
    const revision = state.moduleRevision;
    Loader.inspect_s3m(new Uint8Array(moduleBuffer));
    loadEffectNames();
    const metadata = readMetadata(moduleLabel);
    // The loader is mutable: snapshot its optional structured data before any await.
    // Older assets or accessor/JSON errors must not prevent ordinary playback.
    let libraryMetadata = null;
    try { libraryMetadata = JSON.parse(Loader.module_metadata()); } catch {}
    await context.audio;
    if (!context.isCurrent() || revision !== state.moduleRevision) return null;
    showMessage(`Activating ${moduleLabel}…`);
    const retainedBytes = moduleBuffer.slice(0);
    const loadOptions = loadOptionsFromControls();
    const node = state.node;
    const controlRevision = playback.controlRevision;
    const result = await activateModuleOnNode(node, moduleBuffer, loadOptions);
    if (!context.isCurrent() || revision !== state.moduleRevision || node !== state.node) {
        // Stop may cancel the request after audio installed it. Old metadata cannot arm
        // input for that newly installed session; a newer installation keeps its authority.
        if (node === state.node && result.identity && Ring.sameIdentity(node.starplayerIdentity, result.identity) && !Ring.sameIdentity(state.metadata?.identity, result.identity)) {
            state.interactiveReady = false;
            state.liveInputActive = false;
            state.liveInputPending = false;
            state.currentModuleBytes = null;
            renderMetadata(); setControlsEnabled(false); updateLiveInputPanel();
        }
        return null;
    }
    libraryView.setPlaying(null);
    metadata.identity = result.identity;
    state.metadata = metadata;
    state.letNotesRing = metadata.instruments.map(() => false);
    elements.overlapEnable.checked = state.overlapEnabled;
    elements.uniqueNotes.checked = state.uniqueNotesAcrossChannels;
    state.selectedInstrument = null;
    elements.inputDestination.value = 'jam';
    state.currentModuleBytes = retainedBytes;
    state.activeLoadOptions = {
        headphoneFriendlyModPanning: metadata.isMod && loadOptions.headphoneFriendlyModPanning,
        enhancementFlags: result.appliedEnhancementFlags ?? loadOptions.enhancementFlags,
        speedAdjust: loadOptions.speedAdjust,
    };
    state.activationMemoryBytes = result.memoryBytes;
    state.loadCount += 1;
    renderMetadata();
    const entry = playlist.entries.find(entry => entry.id === context.entryId);
    mediaSession.metadata({ ...metadata, folder: context.folder || (entry?.locator.kind === 'library' ? entry.display.folder : null), archiveLabel: context.archiveLabel || (entry?.locator.kind === 'upload' && entry.locator.memberIndex >= 0 ? entry.display.folder : null) });
    scheduleMediaPosition();
    state.mediaWasPlaying = false;
    backgroundOutput.start();
    mediaSession.position({ lengthFrames: null });
    setControlsEnabled(true);
    // Activating a module installs its own sequencer, which is what retires any live-input
    // source with it. The page's toggles are unchanged, so ask for it back.
    state.liveInputActive = false;
    state.liveInputPending = false;
    state.keyboardInstrument = 0;
    releaseHeldKeys(false);
    syncLiveInput();
    queueCommand(Ring.OPCODE_MASTER_VOLUME, Math.round(Number(elements.volume.value) * 65535 / 100), 0);
    queueRepeatCommand();
    if (controlRevision === playback.controlRevision || state.lastTransportOpcode === Ring.OPCODE_INTERACTIVE_PLAY || state.lastTransportOpcode === Ring.OPCODE_RESUME) {
        if (state.interactiveReady) queueInteractive(Ring.OPCODE_INTERACTIVE_PLAY); else queueCommand(Ring.OPCODE_PLAY);
    }
    const reduction = describeEnhancementReduction(loadOptions.enhancementFlags, result.appliedEnhancementFactor);
    showMessage(`Playing ${metadata.title || moduleLabel}.${reduction ? ` (${reduction}.)` : ''}`);
    const library = context.libraryContext;
    if (libraryMetadata && context.locator?.kind === 'library' && library === state.library
        && library?.identity.sha256 === context.locator.libraryIdentity?.sha256) {
        // D2 borrows bytes and owns any asynchronous hash copy. Enrichment is optional
        // and follows the successful current/revision/node activation commit above.
        try { library.discovery?.acceptPlaybackMetadata(context.locator, libraryMetadata, retainedBytes); } catch {}
    }

    // The engine applies LoadModule at the next quantum. Collection happens in a
    // later worklet message task, never inside process(). Two attempts cover a busy
    // tab without turning garbage collection into a render-path poll.
    setTimeout(requestGarbageCollection, 100);
    setTimeout(requestGarbageCollection, 500);
    return { graph: node, generation: result.generation };
}

async function playArchiveEntry(archive, choice) {
    const result = await playback.play({ kind: 'bytes', read: () => Uint8Array.from(Loader.archive_extract(archive.bytes, choice.index)).buffer }, { origin: 'archive', archiveLabel: archive.label, label: `${choice.name} (from ${archive.label})` });
    if (result.status === 'played') selectRetainedArchiveTrack(archive, choice);
    else if (result.status === 'failed') reportLoadFailure(archive.label, result.error);
}

function reportLoadFailure(label, error) {
    showError(`Could not load ${label}: ${error && error.message ? error.message : error}`);
    showMessage(state.metadata ? `Still playing ${state.metadata.title}.` : 'The player is ready for another file.');
}

function parseArchiveModules(records) {
    const entries = [];
    for (const record of records.split('\n')) {
        if (record === '') continue;
        const fields = record.split('\t');
        if (fields.length !== 3) throw new Error('The archive entry list was malformed.');
        const index = Number(fields[0]);
        const size = Number(fields[2]);
        if (!Number.isSafeInteger(index) || index < 0 || index > 0xFFFF_FFFF || !Number.isSafeInteger(size) || size < 0) {
            throw new Error('The archive entry list contained an invalid index or size.');
        }
        entries.push({ index, name: fields[1], size });
    }
    return entries;
}

function chooseArchiveEntry(entries, archiveLabel) {
    if (state.archivePickerResolve !== null) finishArchivePicker(null);
    state.archiveChoices = entries;
    state.archivePreviousFocus = document.activeElement;
    elements.archivePickerName.textContent = archiveLabel;
    fillArchiveOptions(elements.archiveEntries, entries);
    elements.archiveEntries.selectedIndex = 0;
    elements.archivePicker.hidden = false;
    showMessage(`Choose one of ${entries.length} modules in ${archiveLabel}. Playback continues until you load one.`);
    const promise = new Promise((resolve) => { state.archivePickerResolve = resolve; });
    elements.archiveEntries.focus();
    return promise;
}

function finishArchivePicker(choice) {
    const resolve = state.archivePickerResolve;
    if (resolve === null) return;
    state.archivePickerResolve = null;
    elements.archivePicker.hidden = true;
    state.archiveChoices = [];
    const previousFocus = state.archivePreviousFocus;
    state.archivePreviousFocus = null;
    if (previousFocus instanceof HTMLElement) previousFocus.focus();
    resolve(choice);
}

function loadArchiveChoice() {
    const choice = state.archiveChoices[elements.archiveEntries.selectedIndex];
    if (choice) finishArchivePicker(choice);
}

/// One archive is remembered at a time. Its entry list stays in the load panel so the
/// other tracks in a dropped ZIP are one click away instead of another drop away; the
/// ZIP's own bytes are the only extra memory this holds, because `archive_extract`
/// hands back a fresh copy on every call.
function retainArchive(label, bytes, entries) {
    const archive = { label, bytes, entries };
    state.retainedArchive = archive;
    renderRetainedArchive();
    return archive;
}

function fillArchiveOptions(select, entries) {
    select.replaceChildren();
    for (const [choiceIndex, entry] of entries.entries()) {
        const option = document.createElement('option');
        option.value = String(choiceIndex);
        option.textContent = `${entry.name} — ${formatByteSize(entry.size)}`;
        select.append(option);
    }
}

function renderRetainedArchive() {
    const archive = state.retainedArchive;
    elements.archiveTracks.hidden = archive === null;
    elements.loadArchiveTrack.hidden = archive === null;
    if (archive === null) return;
    fillArchiveOptions(elements.archiveTracks, archive.entries);
    const placeholder = document.createElement('option');
    placeholder.value = '';
    placeholder.textContent = `Track from ${archive.label}…`;
    elements.archiveTracks.prepend(placeholder);
    elements.archiveTracks.selectedIndex = 0;
}

function selectRetainedArchiveTrack(archive, choice) {
    if (state.retainedArchive !== archive) return;
    elements.archiveTracks.value = String(archive.entries.indexOf(choice));
}

function loadRetainedArchiveTrack() {
    const archive = state.retainedArchive;
    if (archive === null) return;
    const selected = elements.archiveTracks.value;
    if (selected === '') {
        showMessage(`Choose one of the ${archive.entries.length} modules in ${archive.label}.`);
        return;
    }

    // The modal owns `archivePickerResolve`. Loading from the dropdown while it is open
    // cancels it rather than resolving it a second time.
    if (state.archivePickerResolve !== null) finishArchivePicker(null);
    const choice = archive.entries[Number(selected)];
    clearError();
    playArchiveEntry(archive, choice).catch((error) => reportLoadFailure(archive.label, error));
}

function formatByteSize(bytes) {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function readMetadata(label) {
    const instruments = [];
    for (let index = 0; index < Loader.module_instrument_count(); index += 1) {
        instruments.push({
            name: Loader.instrument_name(index),
            length: Loader.instrument_sample_length(index),
            capability: Loader.instrument_capability?.(index) ?? 0,
            // Copy now: inspection can change before a candidate load succeeds.
            noteCapabilities: Array.from(Loader.instrument_note_capabilities?.(index) ?? []),
        });
    }
    return {
        label,
        title: Loader.module_title() || label,
        channels: Loader.module_channel_count(),
        orders: Loader.module_order_count(),
        patterns: Loader.module_pattern_count(),
        isMod: Loader.module_is_mod(),
        instruments,
    };
}

function activateModuleOnNode(node, buffer, loadOptions = loadOptionsFromControls(), replacement = null) {
    const previousReady = node === state.node && state.interactiveReady;
    const previousIdentity = node.starplayerIdentity;
    // A fresh module cancels unsent instrument edits for the old session. Already
    // admitted groups keep their acknowledged state if the load fails; replacement
    // captures instead drain every queued edit before preparing the new build.
    if (node === state.node && replacement === null) state.fallbackCommands = state.fallbackCommands.filter(record => record.opcode !== Ring.OPCODE_LET_NOTES_RING || !Ring.sameIdentity(record.identity, previousIdentity));
    cleanupLiveInput();
    if (node === state.node) state.interactiveReady = false;
    const identity = { session: replacement ? replacement[1] : ++state.sessionClock, build: ++state.buildClock };
    const requestId = state.nextRequestId++;
    node.starplayerLatestLoadRequest = requestId;
    return new Promise((resolve, reject) => {
        state.pendingLoads.set(requestId, { resolve, reject, node, identity, fresh: replacement === null, loadOptions: { ...loadOptions }, previousReady, previousIdentity, previousDestination: elements.inputDestination.value, previousKeyboardEnabled: elements.keyboardEnable.checked });
        if (node === state.node) restoreUnreadyInteractiveControls();
        updateModPanningAvailability();
        node.port.postMessage({
            type: 'loadModule',
            requestId, identity, replacement,
            hostSettings: {
                ...pagePolicies(),
                volume: Math.round(Number(elements.volume.value) * 65535 / 100),
                atEnd: playback.effectiveAtEnd() ? Ring.AT_END_CONTINUE : Ring.AT_END_FADE_OUT,
                fadeFrames: Math.round(SONG_FADE_SECONDS * (node.starplayerSampleRate || state.workletSampleRate || 44100)),
                mutes: replacement ? (state.latest?.channels ?? []).map((channel, index) => channel.muted ? index : -1).filter(index => index >= 0) : [],
                inserts: node !== state.node ? [...state.insertLayout].map(([key, insert]) => ({ target: Number(key.split(':')[0]), slot: Number(key.split(':')[1]), ...insert })) : [],
            },
            bytes: buffer,
            headphoneFriendlyModPanning: loadOptions.headphoneFriendlyModPanning,
            enhancementFlags: loadOptions.enhancementFlags,
            speedAdjust: loadOptions.speedAdjust,
        }, [buffer]);
    });
}

/// A load-option checkbox reloads the module the page is holding. Doing that while another
/// load is already in flight interleaves two `loadModule` messages on one FIFO port, so
/// every load option is unavailable until the port is quiet again — on the error path too,
/// which is why this reads the map rather than being set and cleared by hand.
function updateModPanningAvailability() {
    const disabled = state.pendingLoads.size > 0 || state.loadOptionsReloadInProgress;
    elements.modHeadphonePanning.disabled = disabled;
    elements.speedAdjust.disabled = disabled;
    for (const input of elements.enhancementOptions.querySelectorAll('input[type=checkbox]')) {
        input.disabled = disabled;
    }
}

function onWorkletMessage(message, node) {
    if (message.type === 'ready') {
        node.starplayerInteractiveVersion = message.interactiveVersion === Ring.INTERACTIVE_WIRE_VERSION && message.commandRingVersion === Ring.INTERACTIVE_WIRE_VERSION ? message.interactiveVersion : 0;
        node.starplayerSampleRate = message.sampleRate;
        node.starplayerRenderQuantum = message.renderQuantum;
        node.starplayerLeadFrames = message.leadFrames ?? 0;
        node.starplayerLeadMillis = message.leadMillis ?? 0;
        if (node === state.node) {
            state.workletSampleRate = message.sampleRate;
            state.eventLeadFrames = node.starplayerLeadFrames;
            state.eventLeadMillis = node.starplayerLeadMillis;
            elements.quantum.textContent = `${message.renderQuantum} frames`;
            updateOutputPanel();
        }
        // The effect list is the same for every node this build creates — a build-time
        // fact, not a per-instance one — so it is parsed once and never re-parsed on a
        // later rebuild's own 'ready'.
        if (state.effects.length === 0 && message.effects) {
            try {
                state.effects = JSON.parse(message.effects);
            } catch (error) {
                showError(`Could not parse the effect list: ${error && error.message ? error.message : error}`);
            }
            ensureEffectsSlots();
        }
        // M10-W4: same build-time-fact reasoning as `effects` above — parsed once, from
        // whichever node's `ready` message arrives first.
        if (state.enhancements.length === 0 && message.enhancements) {
            try {
                state.enhancements = JSON.parse(message.enhancements);
            } catch (error) {
                showError(`Could not parse the enhancement list: ${error && error.message ? error.message : error}`);
            }
            ensureEnhancementCheckboxes();
        }
    } else if (message.type === 'quantum') {
        if (node !== state.node) return;
        elements.quantum.textContent = `${message.frames} frames${message.matchesEngine ? '' : ' (unexpected)'}`;
    } else if (message.type === 'telemetry') {
        if (node === state.node) {
            state.latest = message;
            if (message.scope) state.latestScope = message.scope;
        }
    } else if (message.type === 'loadReplayReady') {
        const pending = state.pendingLoads.get(message.requestId);
        if (pending?.node !== node || !message.accepted || !Ring.sameIdentity(pending.identity, message.identity)) return;
        node.port.postMessage({ type: 'completeLoad', requestId: message.requestId, identity: pending.identity, policies: pagePolicies() });
    } else if (message.type === 'loadReplayPending') {
        if (state.pendingLoads.get(message.requestId)?.node === node) node.port.postMessage({ type: 'continueReplay', requestId: message.requestId });
    } else if (message.type === 'loadDeferred') {
        if (state.pendingLoads.get(message.requestId)?.node === node) setTimeout(() => node.port.postMessage({ type: 'continueLoad', requestId: message.requestId }), 10);
    } else if (message.type === 'moduleLoaded' || message.type === 'moduleError') {
        const pending = state.pendingLoads.get(message.requestId);
        if (pending && pending.node === node) {
            // A desire may have changed after the worklet requested confirmation.
            // Keep this installation unready and reconcile its newest page revision.
            if (message.type === 'moduleLoaded' && message.accepted && Ring.sameIdentity(pending.identity, message.identity) && message.policyRevision !== undefined && message.policyRevision !== (state.policyRevision ?? 0)) {
                node.port.postMessage({ type: 'reconcilePolicies', requestId: message.requestId, identity: pending.identity, generation: message.generation, transportOpcode: pending.transportOpcode, transportRevision: pending.transportRevision, policies: pagePolicies() });
                return;
            }
            state.pendingLoads.delete(message.requestId);
            updateModPanningAvailability();
            if (message.type === 'moduleLoaded' && ((!node.starplayerInteractiveVersion && message.generation > 0) || (message.accepted && Ring.sameIdentity(pending.identity, message.identity)))) {
                // A valid reply can already be superseded by a newer same-node load.
                // Resolve its waiter so the caller's revision check can finish, while
                // only the latest request may publish identity/readiness or retry input.
                if (node.starplayerLatestLoadRequest !== undefined && node.starplayerLatestLoadRequest !== message.requestId) { pending.resolve(message); return; }
                node.starplayerIdentity = message.identity ?? pending.identity;
                node.starplayerCapabilities = message.capabilities ?? 0;
                node.starplayerAppliedPolicies = { identity: node.starplayerIdentity, overlapEnabled: message.overlapEnabled === true, uniqueNotesAcrossChannels: message.uniqueNotesAcrossChannels === true };
                if (node === state.node) {
                    state.appliedPolicies = node.starplayerAppliedPolicies;
                    state.policyEdits = (state.policyEdits ?? []).filter(edit => Ring.sameIdentity(edit.identity, node.starplayerIdentity));
                    state.interactiveReady = message.capabilities === 3;
                    state.fallbackCommands = state.fallbackCommands.filter(record => Ring.sameIdentity(record.identity, node.starplayerIdentity));
                    // A transport edit can follow the audio acknowledgement being posted
                    // but precede its page delivery. Its loadTransport message then has
                    // no pending worklet request; replay only that newer intent here.
                    if (node.starplayerInteractiveVersion && pending.transportRevision > (message.transportRevision ?? 0)) {
                        queueCommand(pending.transportOpcode, 0, 0, node.starplayerIdentity);
                        flushFallbackCommands();
                    }
                }
                if (pending.recovering) {
                    renderMetadata(); setControlsEnabled(state.interactiveReady); updateLiveInputPanel();
                    pending.reject(new Error(pending.failureReason));
                } else pending.resolve(message);
            }
            else {
                if (node === state.node && node.starplayerLatestLoadRequest === message.requestId) {
                    // Recovery requires explicit unchanged control AND audio-applied proof;
                    // a failed partial installation must leave mismatched controls disabled.
                    const retained = Boolean(!pending.recovering && pending.previousReady && message.retainedCapabilities === 3 && node.starplayerCapabilities === 3 && Ring.sameIdentity(message.retainedIdentity, pending.previousIdentity) && Ring.sameIdentity(message.retainedIdentity, node.starplayerIdentity) && Ring.sameIdentity(message.retainedIdentity, state.metadata?.identity));
                    state.interactiveReady = false;
                    if (retained) {
                        pending.recovering = true;
                        pending.failureReason = message.reason || 'The module could not be loaded.';
                        pending.identity = message.retainedIdentity;
                        state.pendingLoads.set(message.requestId, pending);
                        node.port.postMessage({ type: 'reconcilePolicies', requestId: message.requestId, identity: pending.identity, transportOpcode: pending.transportOpcode, transportRevision: pending.transportRevision, policies: pagePolicies() });
                        return;
                    }
                    renderMetadata(); setControlsEnabled(false); updateLiveInputPanel();
                }
                pending.reject(new Error(message.reason || 'Interactive audio protocol is unavailable or the load acknowledgement is stale.'));
            }
        }
    } else if (message.type === 'stateBusy') {
        const pending = state.pendingStates.get(message.requestId);
        if (pending?.node === node) setTimeout(() => node.port.postMessage({ type: 'captureState', requestId: message.requestId, identity: pending.identity }), 10);
    } else if (message.type === 'stateReady') {
        const pending = state.pendingStates.get(message.requestId);
        if (pending?.node === node && message.accepted && Ring.sameIdentity(pending.identity, message.identity)) node.port.postMessage({ type: 'readState', requestId: message.requestId, identity: message.identity, serial: BigInt(message.serial) });
        else if (pending?.node === node) { state.pendingStates.delete(message.requestId); pending.reject(new Error(message.reason || 'Renderer state was not applied.')); }
    } else if (message.type === 'stateCaptured' || message.type === 'stateError') {
        const pending = state.pendingStates.get(message.requestId);
        if (pending?.node === node) {
            state.pendingStates.delete(message.requestId);
            if (message.type === 'stateCaptured' && Ring.sameIdentity(pending.identity, message.identity)) pending.resolve(message.replacement);
            else pending.reject(new Error(message.reason || 'Stale renderer state.'));
        }
    } else if (message.type === 'interactiveOverflow') {
        if (node === state.node && Ring.sameIdentity(message.identity, node.starplayerIdentity)) { state.keysDown.clear(); state.liveRecoveryPending = true; queueInteractive(Ring.OPCODE_STOP_PREVIEW); flushFallbackCommands(); }
    } else if (message.type === 'interactiveApplied') {
        if (node !== state.node || !message.words) return;
        const words = message.words;
        const identity = { session: BigInt(words[2]) | (BigInt(words[3]) << 32n), build: BigInt(words[4]) | (BigInt(words[5]) << 32n) };
        if (!Ring.sameIdentity(identity, node.starplayerIdentity)) return;
        state.liveInputPending = false;
        if (!message.accepted) {
            if (message.count && state.interactiveReady) { state.liveRecoveryPending = true; queueInteractive(Ring.OPCODE_STOP_PREVIEW); flushFallbackCommands(); }
            showError('Audio rejected a stale, invalid or full-queue control change.');
        }
        for (let index = 0; index < message.count * 3; index += 3) {
            const opcode = message.commands[index], argument = message.commands[index + 1], extra = message.commands[index + 2];
            if (opcode === Ring.OPCODE_NOTE_OVERLAP || opcode === Ring.OPCODE_UNIQUE_NOTES) {
                const key = opcode === Ring.OPCODE_NOTE_OVERLAP ? 'overlapEnabled' : 'uniqueNotesAcrossChannels';
                state.appliedPolicies ??= { identity, overlapEnabled: state.overlapEnabled, uniqueNotesAcrossChannels: state.uniqueNotesAcrossChannels };
                const editIndex = (state.policyEdits ?? []).findIndex(edit => edit.opcode === opcode && edit.argument === argument && Ring.sameIdentity(edit.identity, identity));
                const edit = editIndex >= 0 ? state.policyEdits.splice(editIndex, 1)[0] : null;
                if (message.accepted) state.appliedPolicies[key] = argument !== 0;
                else if (edit && edit.revision === state.policyIntentRevisions[key]) {
                    state[key] = state.appliedPolicies[key];
                    state.policyRevision += 1;
                    state.policyIntentRevisions[key] = state.policyRevision;
                    state.sessionSettingsRevision += 1;
                    publishPendingPolicies();
                }
            }
            if (!message.accepted) continue;
            if (opcode === Ring.OPCODE_LET_NOTES_RING) state.letNotesRing[argument] = extra !== 0;
            if (opcode === Ring.OPCODE_LIVE_DESTINATION) { state.liveInputActive = argument !== 0; state.liveRecoveryPending = false; }
            if (opcode === Ring.OPCODE_STOP_PREVIEW) state.liveRecoveryPending = false;
        }
        elements.overlapEnable.checked = state.overlapEnabled;
        if (elements.uniqueNotes) elements.uniqueNotes.checked = state.uniqueNotesAcrossChannels;
        updateInstrumentControls();
        updateLiveInputPanel();
    } else if (message.type === 'garbageCollected') {
        if (node !== state.node) return;
        state.retiredSeen = message.total;
        elements.retired.textContent = `${state.retiredSeen} returned, ${message.pending} pending`;
        if (state.loadCount > 1 && message.collected > 0) {
            showMessage(`Loaded ${state.metadata.title}; the retired module Arc returned off the audio callback.`);
        }
    } else if (message.type === 'fault') {
        if (node === state.node) showError(`Audio engine fault: ${message.reason}`);
    } else if (message.type === 'mixerModeApplied') {
        if (node !== state.node || node.starplayerInteractiveVersion) return;
        state.activeModeWire = message.active >>> 0;
        state.activationMemoryBytes = message.memoryBytes;
        updateOutputPanel();
        showMessage(`Mixer active: ${describeMixerMode(state.activeModeWire)}.`);
    } else if (message.type === 'mixerModeError') {
        if (node === state.node) showError(`Could not switch mixer mode: ${message.reason}`);
    } else if (message.type === 'midiInputApplied') {
        if (node !== state.node) return;
        state.liveInputPending = false;
        state.liveInputActive = message.active === true;
        state.eventLeadFrames = message.leadFrames;
        state.eventLeadMillis = message.leadMillis;
        state.activationMemoryBytes = message.memoryBytes;
        showMessage(state.liveInputActive
            ? 'Jam mode: the keyboard and any MIDI port play this module’s instruments over its pattern data.'
            : 'Jam mode off; the module keeps playing.');
        updateLiveInputPanel();
    } else if (message.type === 'midiInputError') {
        if (node !== state.node) return;
        state.liveInputPending = false;
        elements.keyboardEnable.checked = false;
        showError(`Could not switch live input: ${message.reason}`);
        updateLiveInputPanel();
    } else if (message.type === 'insertsApplied') {
        state.pendingInserts.delete(message.requestId);
        if (node !== state.node || node.starplayerInteractiveVersion && !Ring.sameIdentity(message.identity, node.starplayerIdentity)) return;
        state.activationMemoryBytes = message.memoryBytes;
        renderEffectSlotRows();
    } else if (message.type === 'insertsError') {
        const pending = state.pendingInserts.get(message.requestId);
        state.pendingInserts.delete(message.requestId);
        if (node !== state.node || node.starplayerInteractiveVersion && !Ring.sameIdentity(message.identity, node.starplayerIdentity)) return;
        // The install or remove did not happen, so the page's own belief has to be put
        // back the way it was before the optimistic UI update — never left claiming an
        // effect is installed that the engine refused.
        const key = `${message.target}:${message.slot}`;
        if (pending?.previous) state.insertLayout.set(key, pending.previous);
        else state.insertLayout.delete(key);
        showError(`Could not ${message.action} the insert effect: ${message.reason}`);
        renderEffectSlotRows();
    }
}

// ── live input: Web MIDI and the tracker-style keyboard (task E6) ───────────────────
//
// The engine half is `starplayer_engine::instrument`: an `InstrumentRack` bound to the
// module's instruments, fed by an SPSC queue. The page's half is only ever three things —
// install the source, pack three bytes, and say how far ahead of the audio clock they
// land.
//
// # Why the keyboard map is this one
//
// The original StarPlayer has no note-entry keyboard at all: it is a player, and every key
// in `plans/reference/original-star-ui.md` is transport, menu or CD control. So the map
// echoed here is the one every tracker since Ultimate Soundtracker has used and the one a
// tracker musician's fingers already know — FastTracker 2's and Impulse Tracker's: the
// `Z`-`M` row is an octave of white keys with its sharps on `S D  G H J`, and the `Q`-`P`
// row is the octave above with its sharps on `2 3  5 6 7`. Both rows run five notes past
// their octave, exactly as a tracker's do, so a two-handed player has thirty-one keys
// rather than twenty-four. The TUI (plan A1) reuses this table.
//
// Named by `KeyboardEvent.code` rather than `.key`, so an AZERTY or Dvorak keyboard plays
// the same *physical* keys a tracker player reaches for.
const KEYBOARD_SEMITONES = new Map([
    // Lower octave: the `Z` row, sharps on the `S` row above it.
    ['KeyZ', 0], ['KeyS', 1], ['KeyX', 2], ['KeyD', 3], ['KeyC', 4], ['KeyV', 5], ['KeyG', 6],
    ['KeyB', 7], ['KeyH', 8], ['KeyN', 9], ['KeyJ', 10], ['KeyM', 11],
    ['Comma', 12], ['KeyL', 13], ['Period', 14], ['Semicolon', 15], ['Slash', 16],
    // Upper octave: the `Q` row, sharps on the number row above it.
    ['KeyQ', 12], ['Digit2', 13], ['KeyW', 14], ['Digit3', 15], ['KeyE', 16], ['KeyR', 17], ['Digit5', 18],
    ['KeyT', 19], ['Digit6', 20], ['KeyY', 21], ['Digit7', 22], ['KeyU', 23],
    ['KeyI', 24], ['Digit9', 25], ['KeyO', 26], ['Digit0', 27], ['KeyP', 28],
]);

/// Hardware MIDI channel used by the computer keyboard. The host maps 0–15 through its
/// configured lane layout: legacy base 48, or interactive base 64. Keep MIDI identity
/// separate from engine lane IDs; NO-W1 negotiates interactive capability on the wire.
const KEYBOARD_MIDI_CHANNEL = 0;
/// Fixed, as the deliverable specifies: a computer keyboard has no velocity to read.
const KEYBOARD_VELOCITY = 100;
const KEYBOARD_MIN_OCTAVE = 0;
const KEYBOARD_MAX_OCTAVE = 8;

/// One channel-voice message onto the command ring. On shared memory it reaches the
/// worklet at its next render quantum; on the `postMessage` fallback it waits for the
/// animation frame the rest of the commands are batched into, which is the added latency
/// `updateLiveInputPanel` reports.
function queueMidiMessage(status, data1, data2) {
    if (!state.interactiveReady || !state.liveInputActive || state.liveRecoveryPending) return;
    queueInteractive(Ring.OPCODE_LIVE_MIDI, Ring.packMidiMessage(status, data1, data2), 0);
    state.eventsSent += 1;
}

/// Turn the live-input source on or off in the worklet. It allocates an instrument rack,
/// so it is a port message rather than a ring command.
function requestLiveInput(enabled) {
    if (!state.interactiveReady) { restoreUnreadyInteractiveControls(); return; }
    if (state.node === null || state.liveInputPending) return;
    const selected = elements.inputDestination.value === 'selected';
    const destination = !enabled ? 0 : selected && state.selectedInstrument !== null ? state.selectedInstrument + 2 : selected ? 0 : 1;
    state.liveInputPending = true;
    queueInteractive(Ring.OPCODE_LIVE_DESTINATION, destination);
    flushFallbackCommands();
}


/// Live input is wanted while the keyboard is armed or a MIDI port is open.
function syncLiveInput() {
    requestLiveInput(elements.keyboardEnable.checked || state.midiPort !== null);
}

function updateLiveInputPanel() {
    const active = state.liveInputActive;
    elements.liveInputStatus.textContent = active ? (elements.inputDestination.value === 'selected' ? `selected instrument ${(state.selectedInstrument ?? 0) + 1}` : 'jamming over the module') : 'off';
    const lead = state.eventLeadFrames > 0
        ? `lead ${state.eventLeadFrames} frames (${state.eventLeadMillis.toFixed(1)} ms)`
        : 'lead —';
    // The fallback transport batches commands into one animation frame, so a note waits
    // for the next one on top of the engine's own lead. Architecture §9.1's Q1 resolution
    // asks for that extra latency to be *reported* rather than hidden.
    setText(elements.eventLead, state.commandRing === null && state.node !== null ? `${lead} + one frame batched` : lead);
    setText(elements.keyboardOctave, String(state.keyboardOctave));
    setText(elements.keyboardInstrument, String(state.keyboardInstrument + 1));
    // Only what the page sent: an event the engine's queue refused is already counted in
    // the Engine panel's "Dropped", because a refused live event is a command that could
    // not be delivered like any other.
    setText(elements.midiEvents, `${state.eventsSent} sent`);
    elements.keyboardEnable.disabled = !state.interactiveReady || state.metadata === null;
    elements.inputDestination.disabled = !state.interactiveReady || state.metadata === null;
    elements.midiInput.disabled = !state.interactiveReady || state.midiAccess === null || state.metadata === null;
}

/// Ask the browser for MIDI access, once, on a click. Sysex is not requested: nothing here
/// decodes it, and asking would turn a silent permission into a prompt.
async function enableWebMidi() {
    if (typeof navigator.requestMIDIAccess !== 'function') {
        elements.midiNote.textContent = 'This browser has no Web MIDI. The computer keyboard still works.';
        return;
    }
    try {
        state.midiAccess = await navigator.requestMIDIAccess({ sysex: false });
    } catch (error) {
        elements.midiNote.textContent = `Web MIDI was refused: ${error.message || error}`;
        return;
    }
    state.midiAccess.onstatechange = fillMidiInputs;
    fillMidiInputs();
    elements.enableMidi.disabled = true;
    elements.midiNote.textContent = 'Web MIDI is on. Choose an input.';
}

function fillMidiInputs() {
    if (state.midiAccess === null) return;
    const chosen = elements.midiInput.value;
    // Built as elements rather than as markup: a port name is whatever the device's
    // firmware says it is, and it has no business being parsed as HTML.
    elements.midiInput.replaceChildren(new Option('None', ''));
    for (const input of state.midiAccess.inputs.values()) {
        if (input.state === 'disconnected') continue;
        const label = `${input.name || 'MIDI input'}${input.manufacturer ? ` — ${input.manufacturer}` : ''}`;
        elements.midiInput.append(new Option(label, input.id));
    }
    // A port that vanished takes the selection with it; one that is still there keeps it.
    elements.midiInput.value = [...state.midiAccess.inputs.values()].some(input => input.id === chosen && input.state !== 'disconnected') ? chosen : '';
    if (elements.midiInput.value !== chosen) selectMidiInput();
    updateLiveInputPanel();
}

/// Open the selected port and forward its messages as they arrive. Web MIDI delivers
/// *framed* messages — the browser has already resolved running status — so each one is
/// packed straight into a wire record with no decoder on this side.
function selectMidiInput() {
    cleanupLiveInput();
    if (state.midiPort !== null) {
        state.midiPort.onmidimessage = null;
        state.midiPort = null;
    }
    const id = elements.midiInput.value;
    if (id && state.midiAccess !== null) {
        const port = state.midiAccess.inputs.get(id);
        if (port) {
            port.onmidimessage = (event) => {
                const data = event.data;
                if (!data || data.length === 0 || data[0] < 0x80 || data[0] >= 0xF0) return;
                queueMidiMessage(data[0], data.length > 1 ? data[1] : 0, data.length > 2 ? data[2] : 0);
            };
            state.midiPort = port;
        }
    }
    syncLiveInput();
    updateLiveInputPanel();
}

/// Whether a key event is the page's to play, rather than something the person is typing.
function keyboardIsListening(event) {
    if (!elements.keyboardEnable.checked || !state.liveInputActive || event.target.closest?.('#library-panel, #playlist-panel')) return false;
    if (event.ctrlKey || event.altKey || event.metaKey) return false;
    const target = event.target;
    if (target instanceof HTMLElement && target.isContentEditable) return false;
    return !(target instanceof HTMLInputElement || target instanceof HTMLSelectElement || target instanceof HTMLTextAreaElement);
}

/// MIDI note for a physical key at the current octave. Octave 4 puts `Z` on note 60,
/// which is the engine's reference note — a sample plays at its own rate there
/// (M4 master-plan decision 4).
function noteForKey(code) {
    const semitone = KEYBOARD_SEMITONES.get(code);
    if (semitone === undefined) return null;
    const note = (state.keyboardOctave + 1) * 12 + semitone;
    return note >= 0 && note <= 127 ? note : null;
}

function onKeyDown(event) {
    if (!keyboardIsListening(event)) return;
    // Held keys repeat at the platform's typematic rate. A retrigger every 30 ms is not
    // what holding a note means, so a repeat is dropped rather than sounded.
    if (event.repeat) return;

    if (event.code === 'BracketLeft' || event.code === 'BracketRight') {
        const step = event.code === 'BracketRight' ? 1 : -1;
        state.keyboardOctave = Math.max(KEYBOARD_MIN_OCTAVE, Math.min(KEYBOARD_MAX_OCTAVE, state.keyboardOctave + step));
        event.preventDefault();
        updateLiveInputPanel();
        return;
    }
    if (event.code === 'Minus' || event.code === 'Equal') {
        const count = Math.max(1, state.metadata?.instruments?.length ?? 1);
        const step = event.code === 'Equal' ? 1 : -1;
        state.keyboardInstrument = Math.max(0, Math.min(count - 1, state.keyboardInstrument + step));
        if (elements.inputDestination.value === 'selected') selectInstrument(state.keyboardInstrument);
        else queueMidiMessage(0xC0 | KEYBOARD_MIDI_CHANNEL, state.keyboardInstrument, 0);
        event.preventDefault();
        updateLiveInputPanel();
        return;
    }

    const note = noteForKey(event.code);
    if (note === null || state.keysDown.has(event.code)) return;
    // Remembered per key, so the note-off releases the note the key *started* even if the
    // octave moved while it was held.
    state.keysDown.set(event.code, note);
    queueMidiMessage(0x90 | KEYBOARD_MIDI_CHANNEL, note, KEYBOARD_VELOCITY);
    event.preventDefault();
}

function onKeyUp(event) {
    const note = state.keysDown.get(event.code);
    if (note === undefined) return;
    state.keysDown.delete(event.code);
    queueMidiMessage(0x80 | KEYBOARD_MIDI_CHANNEL, note, 0);
    event.preventDefault();
}

/// A page that loses focus with keys held would leave those notes sounding for ever.
///
/// `send` is false when live input is about to be torn down: reinstalling the module's own
/// sequencer releases every voice anyway, so a note-off queued here would arrive at a queue
/// that has gone and be counted as a command that could not be delivered. Forgetting the
/// keys is the whole job in that case.
function releaseHeldKeys(send = true) {
    if (send) {
        for (const note of state.keysDown.values()) {
            queueMidiMessage(0x80 | KEYBOARD_MIDI_CHANNEL, note, 0);
        }
    }
    state.keysDown.clear();
}

function requestGarbageCollection() {
    if (state.node !== null) {
        state.node.port.postMessage({ type: 'collectGarbage' });
    }
}

function pagePolicies() {
    return { overlapEnabled: state.overlapEnabled === true, uniqueNotesAcrossChannels: state.uniqueNotesAcrossChannels === true, policyRevision: state.policyRevision ?? 0 };
}

function publishPendingPolicies() {
    for (const [requestId, pending] of state.pendingLoads) {
        pending.node.port.postMessage({ type: 'loadPolicies', requestId, identity: pending.identity, policies: pagePolicies() });
    }
}

function setPagePolicy(opcode, enabled) {
    const key = opcode === Ring.OPCODE_NOTE_OVERLAP ? 'overlapEnabled' : 'uniqueNotesAcrossChannels';
    state[key] = Boolean(enabled);
    state.policyRevision = (state.policyRevision ?? 0) + 1;
    state.policyIntentRevisions ??= {};
    state.policyIntentRevisions[key] = state.policyRevision;
    state.sessionSettingsRevision = (state.sessionSettingsRevision ?? 0) + 1;
    elements.overlapEnable.checked = state.overlapEnabled;
    elements.uniqueNotes.checked = state.uniqueNotesAcrossChannels;
    publishPendingPolicies();
    if (!state.interactiveReady || !state.node?.starplayerIdentity) return;
    const identity = state.node.starplayerIdentity;
    state.policyEdits ??= [];
    state.policyEdits.push({ identity, opcode, argument: Number(enabled), revision: state.policyRevision });
    queueInteractive(opcode, Number(enabled));
    flushFallbackCommands();
}

function queueTransport(opcode) {
    state.lastTransportOpcode = opcode;
    state.transportRevision = (state.transportRevision ?? 0) + 1;
    let currentPending = false;
    for (const [requestId, load] of state.pendingLoads) {
        load.transportOpcode = opcode;
        load.transportRevision = state.transportRevision;
        load.node.port.postMessage({ type: 'loadTransport', requestId, identity: load.identity, transportRevision: load.transportRevision, opcode, argument: 0, extra: 0 });
        currentPending ||= load.node === state.node;
    }
    if (!currentPending && state.interactiveReady) queueInteractive(opcode);
    else if (!currentPending && !state.node?.starplayerInteractiveVersion) queueCommand(opcode === Ring.OPCODE_HOLD_STOP ? Ring.OPCODE_STOP : Ring.OPCODE_PLAY);
}

function restoreUnreadyInteractiveControls() {
    elements.overlapEnable.checked = state.overlapEnabled;
    elements.overlapEnable.disabled = true;
    elements.uniqueNotes.checked = state.uniqueNotesAcrossChannels;
    elements.uniqueNotes.disabled = true;
    if (state.metadata) renderMetadata();
    for (const pending of state.pendingLoads?.values() ?? []) {
        if (pending.node !== state.node) continue;
        elements.inputDestination.value = pending.previousDestination;
        elements.keyboardEnable.checked = pending.previousKeyboardEnabled;
    }
    updateLiveInputPanel();
}

function queueInteractive(opcode, argument = 0, extra = 0) {
    if (!state.interactiveReady) { restoreUnreadyInteractiveControls(); return; }
    if (!state.node?.starplayerIdentity) return;
    queueCommand(opcode, argument, extra, state.node.starplayerIdentity);
}

function queueCommand(opcode, argument = 0, extra = 0, identity = null) {
    if (state.node === null) return;
    if (opcode === Ring.OPCODE_LIVE_MIDI && state.fallbackCommands.length >= 128) {
        state.fallbackCommands = state.fallbackCommands.filter(record => record.opcode !== Ring.OPCODE_LIVE_MIDI);
        state.liveRecoveryPending = true;
        state.keysDown.clear();
        state.fallbackCommands.push({ opcode: Ring.OPCODE_STOP_PREVIEW, argument: 0, extra: 0, identity });
        return;
    }
    identity ??= state.node.starplayerIdentity;
    if (!Ring.validIdentity(identity)) return;
    if (!Ring.validWireWord(argument) || !Ring.validWireWord(extra)) return;
    switch (opcode) {
        case Ring.OPCODE_MASTER_VOLUME: case Ring.OPCODE_MUTE_CHANNEL: case Ring.OPCODE_AT_END:
        case Ring.OPCODE_INSERT_PARAM: case Ring.OPCODE_INSERT_BYPASS: case Ring.OPCODE_SET_MIXER_MODE:
        case Ring.OPCODE_LIVE_DESTINATION: case Ring.OPCODE_NOTE_OVERLAP: case Ring.OPCODE_UNIQUE_NOTES: case Ring.OPCODE_LET_NOTES_RING:
            state.sessionSettingsRevision = (state.sessionSettingsRevision ?? 0) + 1;
    }
    const record = { opcode, argument: argument >>> 0, extra: extra >>> 0, identity };
    // Native host-control admission also reserves engine settlement room within its
    // 32-command quantum budget. Keep interactive groups at 16; the worklet's existing
    // applied barrier paces subsequent groups, including a bulk edit's remainder.
    if (state.fallbackCommands.length || !state.commandRing || (state.node.starplayerInteractiveVersion && Ring.commandOccupancy(state.commandRing.words) >= 16) || !Ring.pushCommand(state.commandRing, opcode, argument, extra, identity)) state.fallbackCommands.push(record);
}

function repeatFadeFrames() {
    return Math.round(SONG_FADE_SECONDS * (state.workletSampleRate || 44100));
}

/// Send the controller's effective end policy on activation and Repeat changes.
function queueRepeatCommand() {
    queueCommand(Ring.OPCODE_AT_END, playback.effectiveAtEnd() ? Ring.AT_END_CONTINUE : Ring.AT_END_FADE_OUT, repeatFadeFrames());
}

function flushFallbackCommands() {
    if (state.node === null || state.fallbackCommands.length === 0) return;
    if (state.commandRing !== null) {
        while (state.fallbackCommands.length) {
            if (state.node.starplayerInteractiveVersion && Ring.commandOccupancy(state.commandRing.words) >= 16) break;
            const record = state.fallbackCommands[0];
            if (!Ring.pushCommand(state.commandRing, record.opcode, record.argument, record.extra, record.identity)) break;
            state.fallbackCommands.shift();
            if (record.opcode === Ring.OPCODE_SET_MIXER_MODE) state.node.port.postMessage({ type: 'flushCommands' });
        }
    } else {
        const records = state.fallbackCommands.splice(0, state.node.starplayerInteractiveVersion ? 16 : Ring.COMMAND_CAPACITY - 1);
        if (state.node.starplayerInteractiveVersion === 0) state.node.port.postMessage({ type: 'commandBatch', version: 1, commands: records.flatMap(record => [record.opcode, record.argument, record.extra]) });
        else state.node.port.postMessage({ type: 'commandBatch', version: Ring.INTERACTIVE_WIRE_VERSION, commands: records });
    }
}

async function captureReplacement() {
    cleanupLiveInput();
    while (state.fallbackCommands.length) { flushFallbackCommands(); if (state.fallbackCommands.length) await new Promise(resolve => setTimeout(resolve, 10)); }
    const node = state.node, identity = node.starplayerIdentity;
    const requestId = state.nextRequestId++;
    return new Promise((resolve, reject) => {
        state.pendingStates.set(requestId, { resolve, reject, node, identity });
        node.port.postMessage({ type: 'captureState', requestId, identity });
    });
}

/// How a change to one load option is worded, in the three places `applyLoadOptions` has
/// to say something: deferred to the next load, being applied, and applied. Keeping the
/// three together is what stops a new option growing a third ternary in each of them.
function describeLoadOptionChange(changedKey, options) {
    if (changedKey === 'headphoneFriendlyModPanning') {
        const choice = options.headphoneFriendlyModPanning ? 'Headphone-friendly S3M-style 60% spacing' : 'Authentic hard MOD panning';
        return {
            deferred: options.headphoneFriendlyModPanning
                ? 'Headphone-friendly MOD panning saved for the next MOD load.'
                : 'Authentic MOD panning saved for the next MOD load.',
            reloading: `Reloading this MOD with ${options.headphoneFriendlyModPanning ? 'S3M-style 60% spacing' : 'authentic hard panning'}…`,
            applied: `${choice} applied`,
        };
    }
    if (changedKey === 'speedAdjust') {
        const adjust = options.speedAdjust;
        const choice = adjust === 0 ? 'Canonical speed' : `Speed adjustment of ${adjust > 0 ? '+' : ''}${adjust}`;
        return {
            deferred: `${choice} saved for the next load.`,
            reloading: `Rescanning and reloading at ${adjust === 0 ? 'the module\u2019s own speeds' : `every speed ${adjust > 0 ? '+' : ''}${adjust}`}…`,
            applied: `${choice} applied`,
        };
    }
    return {
        deferred: 'Sample enhancement choice saved for the next load.',
        reloading: 'Reloading with the new sample enhancement choice…',
        applied: 'Sample enhancement choice applied',
    };
}

/// A load-option control reloads the module the page is holding. `changedKey` is
/// `'headphoneFriendlyModPanning'` for the panning checkbox, `'speedAdjust'` for the
/// speed bias, or `'enhancementFlags'` for any enhancement checkbox; only the panning
/// checkbox's early-out is format-restricted — the other two apply to every format, so
/// they always reload if a module is retained.
async function applyLoadOptions(changedKey) {
    const requested = loadOptionsFromControls();
    if (state.loadOptionsReloadInProgress) {
        // A disabled checkbox cannot normally emit another user change, but keeping this
        // guard deterministic also covers scripted changes and a racing load interaction.
        writeLoadOptionsToControls(state.loadOptionsReloadRequested);
        renderLoadOptionsSummary();
        persistPreferences();
        return;
    }
    // A disabled control can still emit a scripted event while a fresh session installs.
    // Its cursor/identity cannot rebuild the previously retained module. Preserve the
    // options actually posted with the latest fresh load, before claiming any revision.
    let pendingFreshLoad = null;
    for (const pending of state.pendingLoads.values()) {
        if (pending.node === state.node && pending.fresh) pendingFreshLoad = pending;
    }
    if (pendingFreshLoad) {
        writeLoadOptionsToControls(pendingFreshLoad.loadOptions);
        renderLoadOptionsSummary();
        persistPreferences();
        return;
    }
    // After the guard, so a refused change is summarised as what is still loading rather
    // than as what was refused.
    renderLoadOptionsSummary();
    persistPreferences();
    const modOnlyChange = changedKey === 'headphoneFriendlyModPanning';
    const wording = describeLoadOptionChange(changedKey, requested);
    if (state.currentModuleBytes === null || state.node === null || (modOnlyChange && !state.metadata?.isMod)) {
        showMessage(wording.deferred);
        return;
    }

    const previous = state.activeLoadOptions;
    // Claim a revision exactly as the load path does. Reusing the current value let a
    // concurrent load and this reload both believe they were last: the port is a FIFO, so
    // the module posted second is the one that sounds, and whichever handler ran second
    // would then paint the other module's metadata over it.
    const revision = ++state.moduleRevision;
    const node = state.node;
    const retainedBytes = state.currentModuleBytes;
    state.loadOptionsReloadInProgress = true;
    state.loadOptionsReloadRequested = requested;
    updateModPanningAvailability();
    clearError();
    showMessage(wording.reloading);
    try {
        let controlRevision, settingsRevision, replacement;
        do { controlRevision = playback.controlRevision; settingsRevision = state.sessionSettingsRevision; replacement = await captureReplacement(); }
        while ((controlRevision !== playback.controlRevision || settingsRevision !== state.sessionSettingsRevision) && revision === state.moduleRevision && node === state.node);
        if (revision !== state.moduleRevision || node !== state.node || retainedBytes !== state.currentModuleBytes) return;
        const activation = await activateModuleOnNode(node, retainedBytes.slice(0), requested, replacement);
        if (revision !== state.moduleRevision || node !== state.node) return;
        state.activeLoadOptions = {
            headphoneFriendlyModPanning: requested.headphoneFriendlyModPanning,
            enhancementFlags: activation.appliedEnhancementFlags ?? requested.enhancementFlags,
            speedAdjust: requested.speedAdjust,
        };
        playback.rebind(node, activation.generation);
        scheduleMediaPosition();
        state.activationMemoryBytes = activation.memoryBytes;
        state.loadCount += 1;
        if (state.metadata) state.metadata.identity = activation.identity;
        state.liveInputPending = false;
        state.liveInputActive = (elements.keyboardEnable.checked || state.midiPort !== null);
        renderMetadata(); setControlsEnabled(true);
        persistPreferences();
        const reduction = describeEnhancementReduction(requested.enhancementFlags, activation.appliedEnhancementFactor);
        showMessage(`${wording.applied}; playback restored at the sounding order.${reduction ? ` (${reduction}.)` : ''}`);
        setTimeout(requestGarbageCollection, 100);
        setTimeout(requestGarbageCollection, 500);
    } catch (error) {
        // The preference was persisted before the reload was attempted, so it has to be
        // put back whatever else has happened since — a revision check here would leave a
        // failed toggle stored as the user's choice for every future session.
        writeLoadOptionsToControls(previous);
        renderLoadOptionsSummary();
        persistPreferences();
        if (revision === state.moduleRevision && node === state.node) {
            showError(`Could not change load options: ${error && error.message ? error.message : error}`);
            showMessage(`Still playing ${state.metadata.title}.`);
        }
    } finally {
        state.loadOptionsReloadInProgress = false;
        state.loadOptionsReloadRequested = null;
        updateModPanningAvailability();
    }
}

async function rebuildAudioContext() {
    if (state.node === null) {
        await startAudio();
        return;
    }
    const requested = elements.sampleRate.value;
    const label = requested === '' ? 'device default' : formatRate(Number(requested));
    const oldContext = state.context;
    const oldNode = state.node;
    const moduleBytes = state.currentModuleBytes;
    const revision = state.moduleRevision;
    const controlRevision = playback.controlRevision;
    const settingsRevision = state.sessionSettingsRevision;
    const policyRevision = state.policyRevision;
    const oldSinkId = 'sinkId' in oldContext ? oldContext.sinkId : '';
    const replacement = state.currentModuleBytes === null ? null : await captureReplacement();
    const contextOptions = { latencyHint: 'interactive' };
    if (requested !== '') contextOptions.sampleRate = Number(requested);
    let candidateContext = null;
    let candidateNode = null;
    try {
        // Construction is synchronous and is the point at which Firefox may throw
        // NotSupportedError. Nothing in the live graph has changed yet.
        candidateContext = new AudioContext(contextOptions);
        const unlock = candidateContext.resume();
        const channels = Number(elements.outputChannels.value) === 1 ? 1 : 2;
        const graph = await prepareNode(candidateContext, channels, mixerModeFromControls(), true);
        candidateNode = graph.node;
        const loadOptions = loadOptionsFromControls();
        const activation = state.currentModuleBytes === null
            ? null
            : await activateModuleOnNode(graph.node, state.currentModuleBytes.slice(0), loadOptions, replacement);
        if (oldSinkId && typeof candidateContext.setSinkId === 'function') {
            await candidateContext.setSinkId(oldSinkId).catch((error) => {
                elements.outputDeviceNote.textContent = `The previous output device could not be restored (${error.name || error.message}).`;
            });
        }
        await unlock;
        if (revision !== state.moduleRevision || controlRevision !== playback.controlRevision || state.node !== oldNode || moduleBytes !== state.currentModuleBytes) { graph.node.disconnect(); await candidateContext.close(); return; }
        if (settingsRevision !== state.sessionSettingsRevision) { graph.node.disconnect(); await candidateContext.close(); if (policyRevision !== state.policyRevision) return rebuildAudioContext(); return; }
        graph.node.connect(candidateContext.destination);

        state.context = candidateContext;
        installGraph(graph);
        state.latest = null;
        state.activeModeWire = mixerModeFromControls();
        if (activation) {
            state.metadata.identity = activation.identity;
            playback.rebind(graph.node, activation.generation);
            scheduleMediaPosition();
            state.activationMemoryBytes = activation.memoryBytes;
            state.activeLoadOptions = {
                headphoneFriendlyModPanning: state.metadata?.isMod ? loadOptions.headphoneFriendlyModPanning : state.activeLoadOptions.headphoneFriendlyModPanning,
                enhancementFlags: activation.appliedEnhancementFlags ?? loadOptions.enhancementFlags,
            };
        }
        oldNode.disconnect();
        await oldContext.close().catch(() => {});
        state.requestedSampleRate = requested;
        state.rateReport = `${label} / ${formatRate(candidateContext.sampleRate)}`;
        candidateContext.addEventListener('statechange', updateOutputPanel);
        persistPreferences();
        await refreshOutputDevices();
        updateOutputPanel();
        showMessage(`Output rate rebuilt: requested ${label}, actual ${formatRate(candidateContext.sampleRate)}; playback restored at the sounding order.`);
    } catch (error) {
        if (candidateNode !== null) candidateNode.disconnect();
        if (candidateContext !== null) await candidateContext.close().catch(() => {});
        const name = error?.name || 'Error';
        state.rateReport = `${label} refused (${name}); kept ${formatRate(oldContext.sampleRate)}`;
        updateOutputPanel();
        showMessage(`Sample-rate request was refused (${name}); the existing ${formatRate(oldContext.sampleRate)} context is still playing.`);
    }
}

async function rebuildOutputChannels() {
    const alreadyStarted = state.node !== null;
    await startAudio();
    if (!alreadyStarted) {
        persistPreferences();
        updateOutputPanel();
        return;
    }
    const channels = Number(elements.outputChannels.value) === 1 ? 1 : 2;
    const oldNode = state.node;
    const moduleBytes = state.currentModuleBytes;
    const revision = state.moduleRevision;
    const controlRevision = playback.controlRevision;
    const settingsRevision = state.sessionSettingsRevision;
    const policyRevision = state.policyRevision;
    const replacement = state.currentModuleBytes === null ? null : await captureReplacement();
    let candidateNode = null;
    try {
        const graph = await prepareNode(state.context, channels, mixerModeFromControls(), false);
        candidateNode = graph.node;
        const loadOptions = loadOptionsFromControls();
        const activation = state.currentModuleBytes === null
            ? null
            : await activateModuleOnNode(graph.node, state.currentModuleBytes.slice(0), loadOptions, replacement);
        if (revision !== state.moduleRevision || controlRevision !== playback.controlRevision || state.node !== oldNode || moduleBytes !== state.currentModuleBytes) { graph.node.disconnect(); return; }
        if (settingsRevision !== state.sessionSettingsRevision) { graph.node.disconnect(); if (policyRevision !== state.policyRevision) return rebuildOutputChannels(); return; }
        graph.node.connect(state.context.destination);

        installGraph(graph);
        state.latest = null;
        state.activeModeWire = mixerModeFromControls();
        if (activation) {
            state.metadata.identity = activation.identity;
            playback.rebind(graph.node, activation.generation);
            scheduleMediaPosition();
            state.activationMemoryBytes = activation.memoryBytes;
            state.activeLoadOptions = {
                headphoneFriendlyModPanning: state.metadata?.isMod ? loadOptions.headphoneFriendlyModPanning : state.activeLoadOptions.headphoneFriendlyModPanning,
                enhancementFlags: activation.appliedEnhancementFlags ?? loadOptions.enhancementFlags,
            };
        }
        oldNode.disconnect();
        persistPreferences();
        updateOutputPanel();
        showMessage(`Output rebuilt for ${channels === 1 ? 'mono' : 'stereo'}; playback restored at the sounding order.`);
    } catch (error) {
        if (candidateNode !== null) candidateNode.disconnect();
        showError(`Could not rebuild the output channels: ${error.message || error}`);
    }
}

function applyMixerMode() {
    persistPreferences();
    if (state.node === null) { state.activeModeWire = mixerModeFromControls(); updateOutputPanel(); return; }
    if (!state.node.starplayerInteractiveVersion) { queueCommand(Ring.OPCODE_SET_MIXER_MODE, mixerModeFromControls(state.outputChannelCount)); flushFallbackCommands(); state.node.port.postMessage({ type: 'flushCommands' }); return; }
    rebuildOutputChannels().catch(showError);
}


// ── insert effects (M7-H7) ───────────────────────────────────────────────────────────
//
// Four ordered slots per bus (`starplayer_engine::insert::MAX_INSERTS_PER_CHAIN`).
// Installing or removing an effect allocates its delay lines, so it crosses as a worklet
// message exactly as `midiInput` does; a parameter change or a bypass flip is
// `OPCODE_INSERT_PARAM` / `OPCODE_INSERT_BYPASS` on the ordinary command ring, applied in
// `process()` like every other opcode. Nothing about a specific effect's name, unit,
// range or default is known here — all of it comes from `state.effects`, itself the wasm
// host's own `effects_json()`, so this panel cannot drift from what the wire actually
// accepts.
//
// Nothing reads the engine's insert chain back over the wire, so `state.insertLayout` —
// keyed by `"target:slot"` — is the page's *own* belief of what is installed, the same
// one-way arrangement `starplayer_host::InsertLayout` keeps natively. A rejected install
// or remove (`insertsError`) puts that belief back the way it was.

const INSERT_SLOT_COUNT = 4;

function insertLayoutKey(target, slot) {
    return `${target}:${slot}`;
}

/** Build the four slot rows once `state.effects` has arrived. Each row: an effect select
 *  (`None` plus every entry `effects_json()` reported), a Bypass checkbox, and a container
 *  the selected effect's own parameter sliders are generated into. */
function ensureEffectsSlots() {
    if (state.effectsSlotsBuilt || state.effects.length === 0) return;
    state.effectsSlotsBuilt = true;
    elements.effectsSlots.replaceChildren();
    for (let slot = 0; slot < INSERT_SLOT_COUNT; slot += 1) {
        const row = document.createElement('div');
        row.className = 'effects-slot';

        const heading = document.createElement('div');
        heading.className = 'effects-slot-heading';
        const label = document.createElement('span');
        label.textContent = `Slot ${slot + 1}`;
        const select = document.createElement('select');
        select.append(new Option('None', ''));
        for (const effect of state.effects) select.append(new Option(effect.name, String(effect.wire)));
        const bypassLabel = document.createElement('label');
        bypassLabel.className = 'effects-bypass';
        const bypassInput = document.createElement('input');
        bypassInput.type = 'checkbox';
        bypassInput.disabled = true;
        bypassLabel.append(bypassInput, document.createTextNode(' Bypass'));
        heading.append(label, select, bypassLabel);

        const params = document.createElement('div');
        params.className = 'effects-params';
        row.append(heading, params);
        elements.effectsSlots.append(row);

        select.addEventListener('change', () => onEffectSelectChanged(slot, select));
        bypassInput.addEventListener('change', () => onBypassChanged(slot, bypassInput));
    }
    elements.effectsReset.addEventListener('click', resetEffects);
    renderEffectSlotRows();
}

/** Put the whole panel back to its page-load state: every installed effect on every target
 *  (master and each channel) is removed, so its parameters and bypass go with it, and the
 *  target select returns to Master. Removal is the allocating half, so each one is a
 *  worklet message like a manual `None` selection would be. */
function resetEffects() {
    if (!state.effectsSlotsBuilt) return;
    for (const key of [...state.insertLayout.keys()]) {
        const [target, slot] = key.split(':').map(Number);
        const previous = state.insertLayout.get(key);
        state.insertLayout.delete(key);
        sendInsertMessage('remove', target, slot, 0, previous);
    }
    elements.effectsTarget.value = String(Ring.INSERT_TARGET_MASTER);
    renderEffectSlotRows();
}

/** Rebuild the target `<select>`'s channel options when the loaded module's own channel
 *  count changes — cheap, and called every telemetry update, unlike `renderEffectSlotRows`
 *  which rebuilds slider DOM and must not run at that rate (it would fight a drag in
 *  progress). */
function refreshEffectsTargetOptions() {
    if (!state.effectsSlotsBuilt) return;
    const channelCount = state.latest?.channelCount ?? 0;
    if (channelCount === state.effectsChannelCount) return;
    state.effectsChannelCount = channelCount;
    const chosen = elements.effectsTarget.value;
    elements.effectsTarget.replaceChildren(new Option('Master', String(Ring.INSERT_TARGET_MASTER)));
    for (let channel = 0; channel < channelCount; channel += 1) {
        elements.effectsTarget.append(new Option(`Channel ${channel + 1}`, String(channel)));
    }
    if ([...elements.effectsTarget.options].some((option) => option.value === chosen)) elements.effectsTarget.value = chosen;
    renderEffectSlotRows();
}

/** Reflect `state.insertLayout` into the four slot rows for whichever target is currently
 *  selected. Rebuilds every parameter slider, so this is called only on a real change —
 *  the panel first appearing, a target switch, an install/remove round trip settling — and
 *  never once per telemetry tick. */
function renderEffectSlotRows() {
    if (!state.effectsSlotsBuilt) return;
    const target = Number(elements.effectsTarget.value);
    const rows = elements.effectsSlots.children;
    for (let slot = 0; slot < rows.length; slot += 1) {
        const row = rows[slot];
        const select = row.querySelector('select');
        const bypassInput = row.querySelector('input[type="checkbox"]');
        const params = row.querySelector('.effects-params');
        const installed = state.insertLayout.get(insertLayoutKey(target, slot));
        select.value = installed ? String(installed.wire) : '';
        bypassInput.checked = installed ? installed.bypassed : false;
        const pending = [...state.pendingInserts.values()].some(change => change.target === target && change.slot === slot && Ring.sameIdentity(change.identity, state.node.starplayerIdentity));
        select.disabled = pending;
        bypassInput.disabled = pending || !installed;
        renderEffectParams(params, target, slot, installed);
        for (const input of params.querySelectorAll('input')) input.disabled = pending;
    }
}

function renderEffectParams(container, target, slot, installed) {
    container.replaceChildren();
    if (!installed) return;
    const effect = state.effects.find((candidate) => candidate.wire === installed.wire);
    if (!effect) return;
    effect.params.forEach((spec, paramId) => {
        const value = installed.params[paramId] ?? spec.default;
        const row = document.createElement('label');
        row.className = 'effects-param';
        const name = document.createElement('span');
        name.textContent = spec.name.replace(/_/g, ' ');
        const input = document.createElement('input');
        input.type = 'range';
        input.min = String(spec.min);
        input.max = String(spec.max);
        input.step = '1';
        input.value = String(value);
        const readout = document.createElement('span');
        readout.className = 'effects-param-value';
        readout.textContent = formatParamValue(spec, value);
        input.addEventListener('input', () => {
            const newValue = Number(input.value);
            installed.params[paramId] = newValue;
            readout.textContent = formatParamValue(spec, newValue);
            queueCommand(Ring.OPCODE_INSERT_PARAM, Ring.packInsertParamArgument(target, slot, paramId), newValue);
        });
        row.append(name, input, readout);
        container.append(row);
    });
}

/** A short display suffix for a parameter's own `ParamUnit` — the Rust enum's variant
 *  name, read from `effects_json()` — used only to format the readout, never to decide
 *  what crosses the wire (that is always the raw integer the slider holds). */
function formatParamValue(spec, value) {
    switch (spec.unit) {
        case 'CentiDecibels': return `${(value / 100).toFixed(2)} dB`;
        case 'CentiMilliseconds': return `${(value / 100).toFixed(2)} ms`;
        case 'CentiHertz': return `${(value / 100).toFixed(2)} Hz`;
        case 'Ratio': return `${(value / 100).toFixed(2)}:1`;
        case 'Milliseconds': return `${value} ms`;
        case 'Hertz': return `${value} Hz`;
        case 'Cents': return `${value} ct`;
        case 'Frames': return `${value} fr`;
        case 'Switch': return value !== 0 ? 'on' : 'off';
        default: return String(value);
    }
}

function onEffectSelectChanged(slot, select) {
    const target = Number(elements.effectsTarget.value);
    const key = insertLayoutKey(target, slot);
    const previous = state.insertLayout.get(key);
    if (select.value === '') {
        if (state.insertLayout.has(key)) {
            state.insertLayout.delete(key);
            sendInsertMessage('remove', target, slot, 0, previous);
        }
        renderEffectSlotRows();
        return;
    }
    const wire = Number(select.value);
    const effect = state.effects.find((candidate) => candidate.wire === wire);
    if (!effect) return;
    const defaults = {};
    effect.params.forEach((spec, index) => { defaults[index] = spec.default; });
    state.insertLayout.set(key, { wire, params: defaults, bypassed: false });
    sendInsertMessage('install', target, slot, wire, previous);
    renderEffectSlotRows();
}

function onBypassChanged(slot, bypassInput) {
    const target = Number(elements.effectsTarget.value);
    const installed = state.insertLayout.get(insertLayoutKey(target, slot));
    if (!installed) return;
    installed.bypassed = bypassInput.checked;
    queueCommand(Ring.OPCODE_INSERT_BYPASS, Ring.packInsertSlotArgument(target, slot), bypassInput.checked ? 1 : 0);
}

/** The allocating half: install or remove, from a worklet message task. */
function sendInsertMessage(action, target, slot, kind, previous) {
    if (state.node === null) return;
    const requestId = state.nextRequestId++;
    flushFallbackCommands();
    const identity = state.node.starplayerIdentity;
    state.sessionSettingsRevision = (state.sessionSettingsRevision ?? 0) + 1;
    state.pendingInserts.set(requestId, { target, slot, previous, identity });
    state.node.port.postMessage({ type: 'inserts', version: state.node.starplayerInteractiveVersion, identity, action, target, slot, kind, requestId });
}

async function refreshOutputDevices() {
    const sinkSupported = typeof AudioContext !== 'undefined' && 'setSinkId' in AudioContext.prototype;
    const selectorSupported = navigator.mediaDevices && typeof navigator.mediaDevices.selectAudioOutput === 'function';
    elements.outputDeviceLabel.hidden = !sinkSupported;
    elements.applyOutputDevice.hidden = !sinkSupported;
    elements.chooseOutputDevice.hidden = !selectorSupported;
    if (!sinkSupported) {
        elements.outputDeviceNote.textContent = 'This browser does not expose AudioContext.setSinkId(); the system default output is used.';
        updateOutputPanel();
        return;
    }
    if (!navigator.mediaDevices || typeof navigator.mediaDevices.enumerateDevices !== 'function') {
        elements.outputDeviceNote.textContent = 'Output-device enumeration is unavailable; the default output remains usable.';
        return;
    }
    try {
        const devices = (await navigator.mediaDevices.enumerateDevices()).filter((device) => device.kind === 'audiooutput');
        const selected = state.context && 'sinkId' in state.context
            ? state.context.sinkId
            : (elements.outputDevice.dataset.savedSinkId || '');
        state.outputDevices.clear();
        elements.outputDevice.replaceChildren();
        const defaultOption = document.createElement('option');
        defaultOption.value = '';
        defaultOption.textContent = 'Default';
        elements.outputDevice.append(defaultOption);
        state.outputDevices.set('', { label: 'default' });
        devices.forEach((device, index) => {
            const option = document.createElement('option');
            option.value = device.deviceId;
            option.textContent = device.label || (device.deviceId === 'default' ? 'Default' : `Output ${index + 1}`);
            elements.outputDevice.append(option);
            state.outputDevices.set(device.deviceId, device);
        });
        if ([...elements.outputDevice.options].some((option) => option.value === selected)) elements.outputDevice.value = selected;
        elements.outputDeviceNote.textContent = devices.some((device) => device.label)
            ? 'Choose a permitted browser output.'
            : 'Device labels remain hidden until the browser grants output permission; default is available.';
        updateOutputPanel();
    } catch (error) {
        elements.outputDeviceNote.textContent = `Could not enumerate outputs (${error.name || error.message}); default remains available.`;
    }
}

async function applyOutputDevice() {
    await startAudio();
    if (typeof state.context.setSinkId !== 'function') return;
    try {
        await state.context.setSinkId(elements.outputDevice.value);
        persistPreferences();
        await refreshOutputDevices();
        updateOutputPanel();
        showMessage(`Output device: ${elements.sinkStatus.textContent}.`);
    } catch (error) {
        showError(`Could not select that output device: ${error.name || error.message}`);
    }
}

async function chooseOutputDevice() {
    try {
        const device = await navigator.mediaDevices.selectAudioOutput();
        await refreshOutputDevices();
        selectStoredValue(elements.outputDevice, device.deviceId);
        await applyOutputDevice();
    } catch (error) {
        elements.outputDeviceNote.textContent = `No output was selected (${error.name || error.message}).`;
    }
}

/// The instrument column is sized once per module, to the longest name in it, and clipped
/// with an ellipsis after that. Left auto-width, the column re-measured on every snapshot
/// as different names came and went, and the whole table shuffled with it.
function sizeInstrumentColumn(metadata) {
    const longest = metadata.instruments.reduce((width, instrument) => Math.max(width, instrument.name.length), 0);
    const widthInCharacters = Math.min(28, Math.max(6, longest));
    document.documentElement.style.setProperty('--instrument-width', `${widthInCharacters}ch`);
}

function renderMetadata() {
    const metadata = state.metadata;
    sizeInstrumentColumn(metadata);
    elements.title.textContent = metadata.title;
    elements.moduleDetail.textContent = `${metadata.label} · ${metadata.channels} channels · ${metadata.orders} orders · ${metadata.patterns} patterns · ${metadata.instruments.length} instruments`;
    elements.seekOrder.max = String(Math.max(0, metadata.orders - 1));
    elements.instrumentCount.textContent = String(metadata.instruments.length);
    // Retain native controls through same-module acknowledgements and selection updates.
    // Replacing a held Play node would implicitly release pointer capture or focus.
    if (state.instrumentTableInstruments !== metadata.instruments) {
        state.instrumentTableInstruments = metadata.instruments;
        state.instrumentRows = [];
        elements.instrumentList.replaceChildren();
        for (const [index, instrument] of metadata.instruments.entries()) {
            const item = document.createElement('tr');
            item.dataset.instrument = String(index);
            item.className = 'instrument-select';
            item.addEventListener('click', event => { if (!event.target.closest('button, input, label')) selectInstrument(index); });
            item.addEventListener('keydown', event => {
                if (event.target === item && (event.key === 'Enter' || event.key === ' ')) { event.preventDefault(); if (!event.repeat) selectInstrument(index); }
            });
            const cells = Array.from({ length: 6 }, () => document.createElement('td'));
            cells[0].textContent = String(index + 1).padStart(2, '0');
            cells[1].className = 'instrument-name';
            cells[1].textContent = instrument.name || `(instrument ${index + 1})`;
            cells[1].title = cells[1].textContent;
            cells[5].textContent = instrument.length ? String(instrument.length) : '';
            let preview = null, checkbox = null;
            if (instrument.length > 0) {
                preview = document.createElement('button');
                preview.type = 'button'; preview.className = 'instrument-play';
                preview.innerHTML = '<svg viewBox="0 0 12 12" aria-hidden="true"><path d="M2 1v10l9-5z"/></svg>';

                const begin = () => {
                    const note = state.referenceNote;
                    const flags = instrument.noteCapabilities?.[note] ?? (note === 60 ? ((instrument.capability >> 3) & 3) : 0);
                    if (!state.interactiveReady || !(flags & 1)) return;
                    state.previewInstrument = index;
                    state.previewLooping = !!(flags & 2);
                    backgroundOutput.start(); state.context?.resume().catch(showError);
                    queueInteractive(Ring.OPCODE_PREVIEW_NOTE, index, note); flushFallbackCommands();
                };
                const finish = () => {
                    if (state.previewInstrument !== index) return;
                    if (state.previewLooping) queueInteractive(Ring.OPCODE_RELEASE_PREVIEW);
                    state.previewInstrument = null;
                    updateInstrumentControls();
                    flushFallbackCommands();
                };
                preview.addEventListener('pointerdown', event => { if (event.button !== 0) return; preview.setPointerCapture(event.pointerId); begin(); });
                preview.addEventListener('pointerup', finish);
                preview.addEventListener('pointercancel', cleanupLiveInput);
                preview.addEventListener('lostpointercapture', event => { if (state.previewInstrument === index) cleanupLiveInput(); });
                let keyboardPreview = false;
                preview.addEventListener('keydown', event => { if ((event.key === ' ' || event.key === 'Enter') && !event.repeat) { keyboardPreview = true; event.preventDefault(); begin(); } });
                preview.addEventListener('keyup', event => { if (event.key === ' ' || event.key === 'Enter') { event.preventDefault(); finish(); } });
                preview.addEventListener('blur', () => { if (state.previewInstrument === index) cleanupLiveInput(); });
                preview.addEventListener('click', event => { if (event.detail === 0 && !keyboardPreview) { begin(); finish(); } keyboardPreview = false; });
                cells[3].append(preview);
            }
            const mixed = (instrument.capability & 6) === 6;
            cells[2].textContent = instrument.capability & 4 ? '∞' : '';
            if (instrument.capability & 4) {
                const explanation = mixed ? 'Mixed map: looping and non-looping samples; overlap applies only to non-looping notes' : 'Native looping samples, including sustain loops; note overlap unavailable';
                cells[2].setAttribute('aria-label', explanation); cells[2].title = explanation;
            }
            if (instrument.capability & 2) {
                const label = document.createElement('label'); label.className = 'instrument-checkbox';
                label.title = 'Let previous non-looping notes finish. Identical instrument, note and starting volume on this channel replace earlier sound. Unique notes across channels also replaces matching detached song tails on other channels; live notes are excluded. Unchecked preserves native behavior, including IT NNA.';
                checkbox = document.createElement('input');
                checkbox.type = 'checkbox'; checkbox.dataset.instrument = String(index);
                checkbox.setAttribute('aria-label', `Let notes ring for instrument ${index + 1}`);
                checkbox.addEventListener('change', () => { queueInteractive(Ring.OPCODE_LET_NOTES_RING, index, Number(checkbox.checked)); flushFallbackCommands(); });
                label.append(checkbox); cells[4].append(label);
            }
            item.append(...cells); elements.instrumentList.append(item);
            state.instrumentRows.push({ item, preview, checkbox });
        }
        if (metadata.instruments.length === 0) elements.instrumentList.innerHTML = '<tr><td class="empty" colspan="6">No instruments.</td></tr>';
    }
    updateInstrumentControls();
    ensureChannelRows(metadata.channels);
    state.lastPatternKey = '';
}

function updateInstrumentControls() {
    let eligible = 0, accepted = 0;
    for (const [index, instrument] of (state.metadata?.instruments ?? []).entries()) {
        const controls = state.instrumentRows?.[index];
        if (instrument.capability & 2) { eligible++; if (state.letNotesRing[index]) accepted++; }
        if (!controls) continue;
        const selectable = state.interactiveReady && !!(instrument.capability & 1);
        controls.item.tabIndex = selectable ? 0 : -1;
        controls.item.dataset.selectable = String(selectable);
        controls.item.setAttribute('aria-label', `Instrument ${index + 1}: ${instrument.name || 'empty slot'}. ${selectable ? 'Select for keyboard and MIDI' : 'Keyboard and MIDI selection unavailable'}`);
        controls.item.setAttribute('aria-selected', String(state.selectedInstrument === index));
        if (controls.preview) {
            const note = state.referenceNote;
            const playable = !!((instrument.noteCapabilities?.[note] ?? (note === 60 ? instrument.capability >> 3 : 0)) & 1);
            // Preserve capture until button-up even if the next selected key is unavailable.
            controls.preview.disabled = !state.interactiveReady || (!playable && state.previewInstrument !== index);
            controls.preview.setAttribute('aria-label', `Play instrument ${index + 1} at ${formatNote(note)} (${note})`);
            controls.preview.title = !playable ? `No playable PCM at ${formatNote(note)} (${note})` : !state.interactiveReady ? 'Interactive audio is not ready' : `${formatNote(note)} (${note}), using prepared samples and master processing`;
        }
        if (controls.checkbox) { controls.checkbox.disabled = !state.interactiveReady; controls.checkbox.checked = !!state.letNotesRing[index]; }
    }
    elements.instrumentSelectAll.checked = eligible > 0 && accepted === eligible;
    elements.instrumentSelectAll.indeterminate = accepted > 0 && accepted < eligible;
    elements.instrumentSelectAll.disabled = !state.interactiveReady || eligible === 0;
}

function setAllInstrumentOverlap() {
    if (!state.interactiveReady) { restoreUnreadyInteractiveControls(); return; }
    const enabled = Number(elements.instrumentSelectAll.checked);
    // Snapshot the requested value for every eligible row. Accepted state may lag
    // queued writes; filtering against it would lose a quick select-then-clear.
    for (const [index, instrument] of (state.metadata?.instruments ?? []).entries()) {
        if (instrument.capability & 2) queueInteractive(Ring.OPCODE_LET_NOTES_RING, index, enabled);
    }
    flushFallbackCommands();
}

function selectInstrument(index) {
    if (!state.interactiveReady || !(state.metadata?.instruments[index]?.capability & 1)) return;
    cleanupLiveInput();
    state.selectedInstrument = index;
    state.keyboardInstrument = index;
    elements.inputDestination.value = 'selected';
    elements.keyboardEnable.checked = true;
    state.liveInputPending = false;
    syncLiveInput();
    renderMetadata(); updateLiveInputPanel();
}

function cleanupLiveInput() {
    releaseHeldKeys(true);
    state.liveRecoveryPending = true;
    if (state.interactiveReady) { queueInteractive(Ring.OPCODE_STOP_PREVIEW); flushFallbackCommands(); }
    state.previewInstrument = null;
}

function setControlsEnabled(enabled) {
    elements.overlapEnable.disabled = !enabled || !state.interactiveReady;
    elements.uniqueNotes.disabled = !enabled || !state.interactiveReady;
    elements.stopPreview.disabled = !enabled || !state.interactiveReady;
    elements.allSoundOff.disabled = !enabled || !state.interactiveReady;
    for (const control of [elements.previous, elements.play, elements.pause, elements.stop, elements.next, elements.seekOrder, elements.volume, elements.progress]) {
        control.disabled = !enabled;
    }
    elements.pause.disabled = !enabled || !state.interactiveReady;
    elements.repeat.disabled = !enabled || playback.playlistMode;
}

function ensureChannelRows(count) {
    if (state.channelRows.length === count) return;
    state.channelRows = [];
    elements.channelBody.replaceChildren();
    for (let index = 0; index < count; index += 1) {
        const row = document.createElement('tr');
        const channel = document.createElement('td');
        const instrument = document.createElement('td');
        const note = document.createElement('td');
        const volume = document.createElement('td');
        const pan = document.createElement('td');
        const vuCell = document.createElement('td');
        const scopeCell = document.createElement('td');
        const effect = document.createElement('td');
        const action = document.createElement('td');
        const vuTrack = document.createElement('span');
        const vuFill = document.createElement('span');
        const scopeCanvas = document.createElement('canvas');
        const mute = document.createElement('button');
        channel.textContent = String(index + 1).padStart(2, '0');
        instrument.className = 'instrument';
        effect.className = 'effect';
        vuTrack.className = 'vu-track';
        vuFill.className = 'vu-fill';
        vuTrack.append(vuFill);
        vuCell.append(vuTrack);
        scopeCell.className = 'scope-column';
        scopeCanvas.className = 'scope-canvas';
        scopeCanvas.width = SCOPE_CANVAS_WIDTH;
        scopeCanvas.height = SCOPE_CANVAS_HEIGHT;
        scopeCanvas.setAttribute('aria-label', `Channel ${index + 1} oscilloscope`);
        scopeCell.append(scopeCanvas);
        mute.textContent = 'Mute';
        mute.addEventListener('click', () => {
            const muted = mute.dataset.muted !== 'true';
            queueCommand(Ring.OPCODE_MUTE_CHANNEL, index, muted ? 1 : 0);
        });
        action.append(mute);
        row.append(channel, instrument, note, volume, pan, vuCell, scopeCell, effect, action);
        elements.channelBody.append(row);
        state.channelRows.push({ row, instrument, note, volume, pan, vuFill, effect, mute, scope: scopeCanvas.getContext('2d') });
    }
    state.drawnScopeGeneration = -1;
}

/// The newest scope window, from whichever transport is live.
///
/// On the shared-memory path this is a *view* into the worklet's block, not a copy: the
/// taps are architecture 9(b)'s lossy half, so a window torn by a republish mid-draw is
/// a seam of a few milliseconds that nobody can see, and paying to remove it would cost
/// the audio thread real work.
function currentScope() {
    if (state.scope !== null) return Ring.readScope(state.scope);
    return state.latestScope;
}

/// Draw one trace per channel from the tap window.
///
/// The values are voice state sampled per render segment, scaled by the voice's volume —
/// not the mix, which has no per-channel bus to read (see `crates/starplayer-engine/src/scope.rs`).
/// So a muted channel still draws its own signal, which is the point of having one scope
/// per channel rather than one per bus.
function drawScopes() {
    const scope = currentScope();
    if (!scope || state.channelRows.length === 0) return;
    if (scope.generation === state.drawnScopeGeneration) return;
    state.drawnScopeGeneration = scope.generation;

    const middle = SCOPE_CANVAS_HEIGHT / 2;
    const amplitude = middle - 1;
    const columns = Math.min(SCOPE_CANVAS_WIDTH, scope.windowBuckets);
    // The newest `columns` buckets: the window is wider than the canvas so that a page
    // drawing slower than the worklet publishes still has an unbroken trace to show.
    const oldest = scope.windowBuckets - columns;

    for (let index = 0; index < state.channelRows.length; index += 1) {
        const context = state.channelRows[index].scope;
        if (!context) continue;
        context.clearRect(0, 0, SCOPE_CANVAS_WIDTH, SCOPE_CANVAS_HEIGHT);
        context.strokeStyle = '#243040';
        context.lineWidth = 1;
        context.beginPath();
        context.moveTo(0, middle + 0.5);
        context.lineTo(SCOPE_CANVAS_WIDTH, middle + 0.5);
        context.stroke();
        if (index >= scope.channelCount) continue;

        const base = index * scope.windowBuckets;
        context.strokeStyle = state.latest?.channels?.[index]?.muted ? '#5b6a7d' : '#6fd3c5';
        context.beginPath();
        for (let column = 0; column < columns; column += 1) {
            const value = scope.values[base + oldest + column] / 32768;
            const y = middle - value * amplitude;
            if (column === 0) context.moveTo(0.5, y);
            else context.lineTo(column + 0.5, y);
        }
        context.stroke();
    }
}

function updateChannels(snapshot) {
    ensureChannelRows(snapshot.channelCount);
    refreshEffectsTargetOptions();
    for (let index = 0; index < state.channelRows.length; index += 1) {
        const view = state.channelRows[index];
        const channel = snapshot.channels[index];
        const instrument = state.metadata?.instruments[channel.instrument - 1];
        setText(view.instrument, instrument?.name || (channel.instrument ? `#${channel.instrument}` : '—'));
        setText(view.note, formatNote(channel.note));
        setText(view.volume, String(Math.round(channel.volume * 64 / 65535)).padStart(2, '0'));
        setText(view.pan, formatPan(channel.pan));
        setText(view.effect, effectName(channel.effectCode, channel.effectParam) || '—');
        view.vuFill.style.width = `${(channel.vu * 100 / 65535).toFixed(1)}%`;
        view.row.classList.toggle('inactive', !channel.active);
        view.mute.dataset.muted = String(channel.muted);
        setText(view.mute, channel.muted ? 'Unmute' : 'Mute');
    }
}

function ensurePatternGrid(channelCount) {
    if (state.patternRows.length === PATTERN_WINDOW_ROWS && state.patternChannelCount === channelCount) return;
    state.patternRows = [];
    state.patternChannelCount = channelCount;
    const head = document.createElement('thead');
    const heading = document.createElement('tr');
    const rowHeading = document.createElement('th');
    rowHeading.textContent = 'Row';
    heading.append(rowHeading);
    for (let channel = 0; channel < channelCount; channel += 1) {
        const cell = document.createElement('th');
        cell.textContent = `Ch ${String(channel + 1).padStart(2, '0')}`;
        heading.append(cell);
    }
    head.append(heading);
    const body = document.createElement('tbody');
    for (let index = 0; index < PATTERN_WINDOW_ROWS; index += 1) {
        const row = document.createElement('tr');
        const number = document.createElement('td');
        row.append(number);
        const cells = [];
        for (let channel = 0; channel < channelCount; channel += 1) {
            const cell = document.createElement('td');
            cell.className = 'pattern-cell';
            row.append(cell);
            cells.push(cell);
        }
        body.append(row);
        state.patternRows.push({ row, number, cells });
    }
    elements.patternTable.replaceChildren(head, body);
}

function updatePattern(snapshot) {
    const rowCount = Loader.pattern_row_count(snapshot.pattern);
    const channelCount = Loader.pattern_channel_count(snapshot.pattern);
    if (rowCount === 0 || channelCount === 0) return;
    const first = Math.min(Math.max(0, snapshot.row - Math.floor(PATTERN_WINDOW_ROWS / 2)), Math.max(0, rowCount - PATTERN_WINDOW_ROWS));
    const key = `${snapshot.pattern}:${snapshot.row}:${first}:${channelCount}`;
    if (key === state.lastPatternKey) return;
    state.lastPatternKey = key;
    ensurePatternGrid(channelCount);
    const cells = Loader.pattern_window(snapshot.pattern, first, PATTERN_WINDOW_ROWS);
    for (let windowRow = 0; windowRow < PATTERN_WINDOW_ROWS; windowRow += 1) {
        const rowNumber = first + windowRow;
        const view = state.patternRows[windowRow];
        const exists = rowNumber < rowCount;
        view.row.hidden = !exists;
        if (!exists) continue;
        setText(view.number, String(rowNumber).padStart(2, '0'));
        view.row.classList.toggle('sounding', rowNumber === snapshot.row);
        for (let channel = 0; channel < channelCount; channel += 1) {
            const offset = (windowRow * channelCount + channel) * PATTERN_CELL_BYTES;
            const note = cells[offset];
            const instrument = cells[offset + 1];
            const volume = cells[offset + 2];
            const effectCode = cells[offset + 3];
            const effectParam = cells[offset + 4];
            const rawEffect = effectCode === 0 ? '···' : `${String.fromCharCode(64 + effectCode)}${hex(effectParam)}`;
            const text = `${formatPatternNote(note)} ${instrument === 0 ? '··' : hex(instrument)} ${volume === 255 ? '··' : hex(volume)} ${rawEffect}`;
            setText(view.cells[channel], text);
            view.cells[channel].title = effectName(effectCode, effectParam);
        }
    }
}

/// Mirrors the engine's song timeline onto the slider and its two time readouts. A seek in
/// flight keeps showing the frame it targeted until a snapshot published after the seek was
/// queued proves the engine caught up — otherwise a slow SAB or fallback tick would snap the
/// slider back to the pre-seek position for a frame or two. `state.scrubbing` short-circuits
/// all of it: while the user is dragging, only the `input` handler touches the display.
function updateProgress(snapshot) {
    const rate = state.workletSampleRate;
    const lengthKnown = (snapshot.songFlags & Ring.SONG_FLAG_LENGTH_KNOWN) !== 0;
    // With Repeat off a song that *loops* — something in it jumps back into music already
    // played — plays on past its loop point under the fade, so the slider covers the fade
    // too and always represents one complete playback. A song that merely runs out of
    // order list does not loop and does not fade: the host stops it on its end frame, so
    // its displayed length is exactly one pass (task D2).
    const fadesAtEnd = !playback.effectiveAtEnd() && (snapshot.songFlags & Ring.SONG_FLAG_LOOPS) !== 0;
    const lengthFrames = lengthKnown ? Math.max(0, snapshot.songLengthFrames) + (fadesAtEnd ? repeatFadeFrames() : 0) : null;
    elements.progress.max = String(lengthFrames ?? 0);
    elements.progress.disabled = !lengthKnown;
    setText(elements.duration, formatTime(lengthFrames, rate));

    const endReachedStopped = snapshot.naturalEnd === true;
    if (endReachedStopped) {
        state.pendingSeekFrame = null;
        if (state.scrubbing) return;
        elements.progress.value = '0';
        setText(elements.elapsed, formatElapsed(0, lengthFrames, rate));
        return;
    }
    if (state.pendingSeekFrame !== null && snapshot.sequence <= state.pendingSeekSequence) {
        if (state.scrubbing) return;
        elements.progress.value = String(state.pendingSeekFrame);
        setText(elements.elapsed, formatElapsed(state.pendingSeekFrame, lengthFrames, rate));
        return;
    }
    state.pendingSeekFrame = null;
    if (state.scrubbing) return;
    let frame = Math.max(0, snapshot.songFrame);
    if (lengthFrames !== null) frame = Math.min(frame, lengthFrames);
    elements.progress.value = String(frame);
    setText(elements.elapsed, formatElapsed(frame, lengthFrames, rate));
}

function updateSnapshot(snapshot) {
    if (!snapshot || snapshot.sequence === 0 || !state.metadata) return;
    state.latest = snapshot;
    state.activeModeWire = snapshot.mixerModeWire >>> 0;
    setText(elements.order, `${snapshot.order + 1}/${state.metadata.orders}`);
    setText(elements.pattern, String(snapshot.pattern));
    setText(elements.row, `${String(snapshot.row).padStart(2, '0')}:${snapshot.tick}`);
    setText(elements.speed, String(snapshot.speed));
    setText(elements.bpm, String(snapshot.bpm));
    elements.seekOrder.value = String(snapshot.order);
    updateProgress(snapshot);
    elements.transportChip.textContent = snapshot.transport || (snapshot.playing ? 'playing' : 'stopped');
    elements.pause.setAttribute('aria-pressed', String(snapshot.transport === 'paused'));
    elements.play.setAttribute('aria-pressed', String(snapshot.playing));
    elements.play.textContent = snapshot.transport === 'paused' ? '▶ Resume' : '▶ Play';
    elements.transportChip.classList.toggle('live', snapshot.playing);
    elements.voices.textContent = `${snapshot.voicesActive} ${snapshot.voicesActive === 1 ? 'voice' : 'voices'}`;
    elements.masterPeak.style.width = `${(snapshot.masterPeak * 100 / 65535).toFixed(1)}%`;
    state.retiredSeen = Math.max(state.retiredSeen, snapshot.retiredCollected);
    updateChannels(snapshot);
    updatePattern(snapshot);
    const pages = snapshot.memoryBytes > 0 ? Math.round(snapshot.memoryBytes / 65536) : 0;
    const stable = state.activationMemoryBytes === 0 || snapshot.memoryBytes === state.activationMemoryBytes;
    elements.memory.textContent = `${(snapshot.memoryBytes / 1048576).toFixed(1)} MiB · ${pages} pages${stable ? ' · stable' : ' · GREW'}`;
    elements.dropped.textContent = String(snapshot.droppedCommands + snapshot.publishesDropped);
    elements.retired.textContent = `${state.retiredSeen} returned, ${snapshot.pendingGarbage} pending`;
    elements.health.textContent = snapshot.warnings === 0 && stable ? 'healthy' : 'warning';
    elements.health.style.color = snapshot.warnings === 0 && stable ? 'var(--accent)' : 'var(--danger)';
    updateOutputPanel();
    if (snapshot.warnings !== 0) showError(`Engine warning flags: 0x${snapshot.warnings.toString(16)}`);
}

function refresh() {
    requestAnimationFrame(refresh);
    flushFallbackCommands();
    const snapshot = state.telemetry ? Ring.readTelemetry(state.telemetry) : state.latest;
    updateSnapshot(snapshot);
    drawScopes();
    updateLiveInputPanel();
}

function setText(element, text) {
    if (element.textContent !== text) element.textContent = text;
}

function hex(value) { return Number(value).toString(16).toUpperCase().padStart(2, '0'); }

function formatNote(note) {
    return note === null || note === undefined ? '—' : `${NOTE_NAMES[note % 12]}${Math.floor(note / 12)}`;
}

function formatPatternNote(note) {
    if (note === 255) return '···';
    if (note === 254) return '^^^';
    if (note === 253) return '===';
    return formatNote(note);
}

function formatPan(bits) {
    const position = Math.max(0, Math.min(15, Math.round((bits + 32767) * 15 / 65534)));
    if (position === 0) return 'LFT';
    if (position === 15) return 'RGT';
    return position.toString(16).toUpperCase().padStart(3, ' ');
}

function loadEffectNames() {
    EFFECT_NAMES.clear();
    for (const record of Loader.effect_names().split('\n')) {
        if (record === '') continue;
        const first = record.indexOf(':');
        const second = record.indexOf(':', first + 1);
        const code = Number(record.slice(0, first));
        const nybble = Number(record.slice(first + 1, second));
        EFFECT_NAMES.set(nybble < 0 ? `${code}` : `${code}.${nybble}`, record.slice(second + 1));
    }
}

function effectName(code, param) {
    if (code === 0 && param === 0) return '';
    return EFFECT_NAMES.get(`${code}.${param >> 4}`) || EFFECT_NAMES.get(`${code}`) || '';
}

// The packaging step decides what, if anything, is bundled: `cargo xtask wasm` writes
// `modules/index.json` beside the fixtures, `cargo xtask wasm --pages` ships neither. A
// 404 is therefore the normal state of the public build, not an error worth reporting —
// the menu and its button simply are not part of that page.
async function populateFixtureMenu() {
    let names = null;
    try {
        const response = await fetch('modules/index.json');
        if (response.ok) {
            const parsed = await response.json();
            if (Array.isArray(parsed)) names = parsed;
        }
    } catch {
        names = null;
    }

    if (names === null || names.length === 0) {
        elements.fixturePicker.hidden = true;
        elements.loadFixture.hidden = true;
        return;
    }
    for (const name of names) {
        const option = document.createElement('option');
        option.textContent = String(name);
        elements.fixturePicker.append(option);
    }
}

async function loadFixture() {
    const name = elements.fixturePicker.value;
    if (!name) {
        showError('Choose a bundled fixture first.');
        return;
    }
    await loadBuffer(async signal => {
        const response = await fetch(`modules/${name}`, { signal });
        if (!response.ok) throw new Error(`Fixture request failed (${response.status}).`);
        return response.arrayBuffer();
    }, name, 'fixture');
}

elements.startAudio.addEventListener('click', () => startAudio().catch(() => {}));
elements.filePicker.addEventListener('change', () => {
    const file = elements.filePicker.files[0];
    if (file) loadBuffer(() => file.arrayBuffer(), file.name).catch(showError);
    elements.filePicker.value = '';
});
elements.loadFixture.addEventListener('click', () => loadFixture().catch(showError));
elements.archiveLoad.addEventListener('click', loadArchiveChoice);
elements.archiveCancel.addEventListener('click', () => finishArchivePicker(null));
elements.archiveEntries.addEventListener('dblclick', loadArchiveChoice);
elements.loadArchiveTrack.addEventListener('click', loadRetainedArchiveTrack);
elements.archivePicker.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
        event.preventDefault();
        finishArchivePicker(null);
    } else if (event.key === 'Enter' && event.target === elements.archiveEntries) {
        event.preventDefault();
        loadArchiveChoice();
    }
});
elements.urlForm.addEventListener('submit', (event) => {
    event.preventDefault();
    const url = elements.url.value.trim();
    if (!url) return;
    loadBuffer(async signal => {
        const response = await fetch(url, { signal });
        if (!response.ok) throw new Error(`URL request failed (${response.status} ${response.statusText}).`);
        return response.arrayBuffer();
    }, url, 'url').catch((error) => showError(`Could not fetch that URL. It may need CORS permission. ${error.message}`));
});
for (const type of ['dragenter', 'dragover']) {
    elements.dropZone.addEventListener(type, (event) => { event.preventDefault(); elements.dropZone.classList.add('over'); });
}
for (const type of ['dragleave', 'drop']) {
    elements.dropZone.addEventListener(type, (event) => { event.preventDefault(); elements.dropZone.classList.remove('over'); });
}
elements.dropZone.addEventListener('drop', (event) => {
    const file = event.dataTransfer.files[0];
    if (file) loadBuffer(() => file.arrayBuffer(), file.name).catch(showError);
});
elements.dropZone.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' || event.key === ' ') elements.filePicker.click();
});
elements.play.addEventListener('click', () => { backgroundOutput.start(); state.context?.resume().catch(showError); playback.userControl('play'); queueTransport(state.latest?.transport === 'paused' ? Ring.OPCODE_RESUME : Ring.OPCODE_INTERACTIVE_PLAY); refreshMediaPosition(); });
elements.pause.addEventListener('click', () => { playback.userControl('pause'); queueTransport(Ring.OPCODE_PAUSE); refreshMediaPosition(); });
elements.stop.addEventListener('click', () => { cleanupLiveInput(); playlistPlayback.cancel(); playback.stop(); refreshMediaPosition(); });
elements.previous.addEventListener('click', () => userSeek(Ring.OPCODE_SEEK_ORDER, Math.max(0, (state.latest?.order || 0) - 1)));
elements.next.addEventListener('click', () => userSeek(Ring.OPCODE_SEEK_ORDER, Math.min((state.metadata?.orders || 1) - 1, (state.latest?.order || 0) + 1)));
elements.seekOrder.addEventListener('change', () => userSeek(Ring.OPCODE_SEEK_ORDER, Number(elements.seekOrder.value)));
// Continuous `input` events while dragging never reach the command ring — it holds only
// 64 entries and is drained once per render quantum — only the released `change` does.
elements.progress.addEventListener('input', () => {
    state.scrubbing = true;
    setText(elements.elapsed, formatElapsed(Number(elements.progress.value), progressLengthFrames(), state.workletSampleRate));
});
elements.progress.addEventListener('change', () => {
    const frame = Number(elements.progress.value);
    state.pendingSeekFrame = userSeek(Ring.OPCODE_SEEK_FRAME, frame);
    state.pendingSeekSequence = state.latest?.sequence ?? 0;
    state.scrubbing = false;
});
elements.repeat.addEventListener('change', () => { queueRepeatCommand(); persistPreferences(); });
elements.elapsed.addEventListener('click', toggleTimeDisplay);
elements.duration.addEventListener('click', toggleTimeDisplay);
elements.volume.addEventListener('input', () => {
    const percent = Number(elements.volume.value);
    elements.volumeValue.value = `${percent}%`;
    queueCommand(Ring.OPCODE_MASTER_VOLUME, Math.round(percent * 65535 / 100));
});

// ── live input (task E6) ────────────────────────────────────────────────────────────
elements.keyboardEnable.addEventListener('change', () => {
    // Unchecking with a MIDI port still open leaves live input running, so the held notes
    // need their note-offs; unchecking with nothing else open tears the source down, which
    // silences them by itself.
    if (!elements.keyboardEnable.checked) cleanupLiveInput();
    syncLiveInput();
    updateLiveInputPanel();
});
elements.instrumentSelectAll.addEventListener('change', setAllInstrumentOverlap);
for (let note = 0; note < 128; note++) {
    const option = document.createElement('option');
    option.value = String(note); option.textContent = `${formatNote(note)} (${note})${note === 60 ? ' — middle C' : ''}`;
    elements.referenceNote.append(option);
}
elements.referenceNote.value = String(state.referenceNote);
elements.referenceNote.addEventListener('change', () => { state.referenceNote = Number(elements.referenceNote.value); updateInstrumentControls(); });
elements.overlapEnable.addEventListener('change', () => setPagePolicy(Ring.OPCODE_NOTE_OVERLAP, elements.overlapEnable.checked));
elements.uniqueNotes.addEventListener('change', () => setPagePolicy(Ring.OPCODE_UNIQUE_NOTES, elements.uniqueNotes.checked));
elements.stopPreview.addEventListener('click', cleanupLiveInput);
elements.allSoundOff.addEventListener('click', () => { releaseHeldKeys(false); state.previewInstrument = null; queueInteractive(Ring.OPCODE_ALL_SOUND_OFF); flushFallbackCommands(); });
elements.inputDestination.addEventListener('change', () => { cleanupLiveInput(); state.liveInputPending = false; syncLiveInput(); updateLiveInputPanel(); });
elements.enableMidi.addEventListener('click', () => enableWebMidi().catch(showError));
elements.midiInput.addEventListener('change', selectMidiInput);
// On `window`, not on an element: the keyboard should play wherever the focus is, as long
// as the focus is not somewhere text is being typed — which `keyboardIsListening` decides.
window.addEventListener('keydown', onKeyDown);
window.addEventListener('keyup', onKeyUp);
window.addEventListener('blur', cleanupLiveInput);
document.addEventListener('focusin', event => { if (state.keysDown.size && !keyboardIsListening({ target: event.target })) cleanupLiveInput(); });

elements.applySampleRate.addEventListener('click', () => rebuildAudioContext().catch(showError));
elements.applyChannels.addEventListener('click', () => rebuildOutputChannels().catch(showError));
elements.applyOutputDevice.addEventListener('click', () => applyOutputDevice().catch(showError));
elements.chooseOutputDevice.addEventListener('click', () => chooseOutputDevice().catch(showError));
elements.applyMixer.addEventListener('click', applyMixerMode);
elements.modHeadphonePanning.addEventListener('change', () => applyLoadOptions('headphoneFriendlyModPanning').catch(showError));
// A committed selection reloads and rescans the module. Normalize first so scripted
// changes use the same integer range as restored preferences.
elements.speedAdjust.addEventListener('change', () => {
    elements.speedAdjust.value = String(speedAdjustFromControls());
    applyLoadOptions('speedAdjust').catch(showError);
});
elements.loadOptionsDisclosure.addEventListener('toggle', persistPreferences);
elements.effectsTarget.addEventListener('change', renderEffectSlotRows);
// A choice is remembered as it is made, not only when it is applied: the owner's test
// setup should survive a reload even if the page is reloaded mid-comparison.
for (const select of [elements.sampleRate, elements.outputChannels, elements.mixerPath, elements.mixerInterpolator, elements.mixerDepth, elements.mixerDither]) {
    select.addEventListener('change', persistPreferences);
}
if (navigator.mediaDevices && typeof navigator.mediaDevices.addEventListener === 'function') {
    navigator.mediaDevices.addEventListener('devicechange', () => { refreshOutputDevices().catch(() => {}); });
}

elements.forceFallback.disabled = !sharedMemoryAvailable();
if (!sharedMemoryAvailable()) {
    elements.forceFallback.checked = true;
    elements.commandTransport.textContent = 'postMessage fallback (COOP/COEP unavailable)';
    elements.telemetryTransport.textContent = 'postMessage fallback (COOP/COEP unavailable)';
    elements.scopeTransport.textContent = 'postMessage fallback (COOP/COEP unavailable)';
}
elements.backgroundPlayback.checked = backgroundPlaybackDefault(navigator, window.matchMedia.bind(window));
restorePreferences();
const backgroundOutput = createBackgroundOutput({
    enabled: elements.backgroundPlayback.checked,
    createAudio: () => { const audio = document.createElement('audio'); document.body.append(audio); return audio; },
    report: message => { elements.backgroundNote.textContent = message; },
});
elements.backgroundPlayback.addEventListener('change', () => {
    const snapshot = state.telemetry ? Ring.readTelemetry(state.telemetry) : state.latest;
    elements.backgroundNote.textContent = '';
    backgroundOutput.setEnabled(elements.backgroundPlayback.checked, !!(snapshot?.playing || snapshot?.transport === 'paused' || snapshot?.live?.active));
    persistPreferences();
    refreshOutputDevices().catch(() => {});
    updateOutputPanel();
});
renderLoadOptionsSummary();
state.activeModeWire = mixerModeFromControls();
updateOutputPanel();
updateLiveInputPanel();
refreshOutputDevices().catch(() => {});
requestAnimationFrame(refresh);

// The fixture menu is part of start-up, not a background nicety: the harness picks an
// entry the moment `playerReady` appears, so the fetch has to have settled before it does.
await populateFixtureMenu();
const playlist = createPlaylist();
const playlistControls = await createPlaylistControls({ playlist, state, loader: Loader, ready: () => loaderReady });
const libraryResolver = {
    async resolve(locator, options) {
        if (locator.kind === 'upload') return playlistControls.resolver.resolve(locator, options);
        const library = state.library;
        if (!library || library.identity.sha256 !== locator.libraryIdentity?.sha256) throw new Error('Library unavailable: reconnect its library.');
        const buffer = await library.resolver.resolve(locator, options);
        if (library !== state.library) throw new DOMException('Library changed', 'AbortError');
        return buffer;
    },
    prefetch(locator) {
        if (locator.kind === 'upload') return playlistControls.resolver.prefetch(locator);
        if (state.library && state.library.identity.sha256 === locator.libraryIdentity?.sha256) return state.library.resolver.prefetch(locator);
    },
};
const playback = createPlaybackController({
    resolver: libraryResolver, activate: preparePlayback, ready: () => startAudio(true),
    repeat: () => elements.repeat.checked,
    captureContext: locator => ({ libraryContext: locator.kind === 'library' ? state.library : null }),
    readSnapshot: () => {
        const snapshot = state.telemetry ? Ring.readTelemetry(state.telemetry) : state.latest;
        if (snapshot && state.metadata && playback.current?.graph === state.node && snapshot.moduleGeneration === playback.current.generation) {
            mediaSession.playing(snapshot.playing);
            // A playlist advance keeps the element playing silence: pausing it between tracks
            // would let a phone drop the media session, and freeze the tab, before the next load.
            if (state.mediaWasPlaying && snapshot.naturalEnd && !playback.playlistMode && !snapshot.live?.active) backgroundOutput.pause();
            const edge = state.mediaWasPlaying !== snapshot.playing;
            state.mediaWasPlaying = snapshot.playing;
            if (edge || (state.mediaPositionDue !== null && performance.now() >= state.mediaPositionDue)) { refreshMediaPosition(snapshot.songFrame); state.mediaPositionDue = null; }
        }
        reconcileKeepAlive(snapshot);
        refreshMediaHandlers(snapshot);
        return { graph: state.node, snapshot };
    },
    flushCommands: flushFallbackCommands, stop: () => queueTransport(Ring.OPCODE_HOLD_STOP),
    onIntent({ origin }) {
        state.moduleRevision += 1;
        finishArchivePicker(null);
        if (origin !== 'playlist') { playlistPlayback.cancel(); playlist.setCurrent(null); }
        elements.repeat.disabled = origin === 'playlist';
        elements.repeat.title = origin === 'playlist' ? 'Playlist playback ends each song to advance; your Repeat preference is preserved.' : '';
        queueRepeatCommand();
    },
    onTrackEnded: id => playlistPlayback.ended(id),
});
const mediaSession = createMediaSession({
    mediaSession: navigator.mediaSession,
    play: () => elements.play.click(), pause: () => elements.pause.click(), stop: () => elements.stop.click(),
    next: () => byId('playlist-next').click(), previous: () => byId('playlist-previous').click(),
    seek: seconds => {
        const length = mediaLengthFrames();
        if (length !== null && Number.isFinite(seconds)) userSeek(Ring.OPCODE_SEEK_FRAME, Math.round(Math.max(0, Math.min(length, seconds * state.workletSampleRate))));
    },
});
function mediaLengthFrames(snapshot = state.telemetry ? Ring.readTelemetry(state.telemetry) : state.latest) {
    if (!snapshot || !(snapshot.songFlags & Ring.SONG_FLAG_LENGTH_KNOWN)) return null;
    return Math.max(0, snapshot.songLengthFrames) + (!playback.effectiveAtEnd() && (snapshot.songFlags & Ring.SONG_FLAG_LOOPS) ? repeatFadeFrames() : 0);
}
function refreshMediaPosition(frame) {
    const snapshot = state.telemetry ? Ring.readTelemetry(state.telemetry) : state.latest;
    mediaSession.position({ lengthFrames: mediaLengthFrames(snapshot), frame: frame ?? snapshot?.songFrame ?? 0, sampleRate: state.workletSampleRate });
}
/// Republish the position once the engine has had time to apply a seek, reload or rebuild:
/// on the fallback transport the next tick still reads telemetry from before the command.
function scheduleMediaPosition() { state.mediaPositionDue = performance.now() + MEDIA_POSITION_SETTLE_MS; }
/// One rule pauses the keep-alive, whatever ended playback (a finished playlist, failed or
/// cancelled loads, a failed audio start): the engine has been idle for a while and no load
/// or playlist advance still owns playback. The grace covers a load that has just activated.
function reconcileKeepAlive(snapshot) {
    const busy = snapshot?.playing || snapshot?.transport === 'paused' || snapshot?.live?.active || state.liveInputActive || state.previewInstrument !== null || playback.loading || playlistPlayback.pending !== null;
    if (busy || backgroundOutput.audio.paused) { state.keepAliveIdleSince = null; return; }
    const now = performance.now();
    state.keepAliveIdleSince ??= now;
    if (now - state.keepAliveIdleSince < KEEP_ALIVE_IDLE_MS) return;
    state.keepAliveIdleSince = null;
    backgroundOutput.pause();
    mediaSession.playing(false);
}
function refreshMediaHandlers(snapshot) {
    mediaSession.refresh({ hasEntries: playlist.entries.length > 0, nextEnabled: !byId('playlist-next').disabled, previousEnabled: !byId('playlist-previous').disabled, lengthKnown: mediaLengthFrames(snapshot) !== null });
}
document.addEventListener('visibilitychange', () => {
    if (!document.hidden) { playback.tick(); refreshMediaPosition(); }
});
const playlistPlayback = createPlaylistPlayback({
    playlist, controller: playback, resolver: libraryResolver,
    loop: () => byId('playlist-loop').checked, report: showError,
    changed: () => playlistView.render(), available: playlistControls.canPlay,
});
const playlistView = wirePlaylistList(playlist, { playback: playlistPlayback, availability: playlistControls.availability });
playlistControls.setRender(() => playlistView.render());
function userSeek(opcode, frame) {
    playlistPlayback.cancel();
    playback.userControl('seek');
    const snapshot = state.telemetry ? Ring.readTelemetry(state.telemetry) : state.latest;
    // The displayed fade margin is output time, not additional sequenced music.
    if (opcode === Ring.OPCODE_SEEK_FRAME && (snapshot?.songFlags & Ring.SONG_FLAG_LENGTH_KNOWN) !== 0 && snapshot.songLengthFrames > 0) frame = Math.min(Math.max(0, frame), snapshot.songLengthFrames - 1);
    queueCommand(opcode, frame);
    refreshMediaPosition(opcode === Ring.OPCODE_SEEK_FRAME ? frame : snapshot?.songFrame);
    scheduleMediaPosition();
    return frame;
}
const libraryView = createLibraryView({ playlist, play: playLibraryRow });
await wireLibraryControls({ state, loader: Loader,
    onLibrary: library => { libraryView.setLibrary(library); playlistControls.libraryChanged(library); },
    onLibraryUpdate: (library, change) => { libraryView.updateLibrary(library, change); playlistControls.libraryUpdated(library, change); },
    onComments: () => libraryView.refreshComments(),
});

// `app.js` is a module and therefore deferred. This is the one honest signal that its
// listeners are attached, which is what the headless harness waits on before clicking.
document.documentElement.dataset.playerReady = 'true';
