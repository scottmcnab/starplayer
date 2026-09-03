import initLoader, * as Loader from './starplayer_web.js';

const Ring = globalThis.StarPlayerRing;
const HOST_WASM_URL = 'starplayer_host_wasm_bg.wasm';
const WORKLET_URL = 'starplayer-worklet.js';
const PROCESSOR_NAME = 'starplayer-player';
const MIXER_STORAGE_KEY = 'starplayer.output-and-mixer.v1';
const DEFAULT_MIXER_MODE = 0x0000_0202;
const SONG_FADE_SECONDS = 5;
const PATTERN_WINDOW_ROWS = 13;
const PATTERN_CELL_BYTES = 5;

// The English effect names are the engine's own table, read once from the page-side wasm
// instance rather than transcribed here. `EffectDisplay.name` is a `&'static str` that
// cannot cross the wasm boundary in a packed snapshot, so the table crosses instead — one
// string at start-up, and each channel's effect column stays two scalar words in the
// snapshot instead of a string that would have to be copied per tick.
const EFFECT_NAMES = new Map();
const NOTE_NAMES = ['C-', 'C#', 'D-', 'D#', 'E-', 'F-', 'F#', 'G-', 'G#', 'A-', 'A#', 'B-'];

const byId = (id) => document.getElementById(id);
const elements = {
    startAudio: byId('start-audio'), filePicker: byId('file-picker'), fixturePicker: byId('fixture-picker'),
    loadFixture: byId('load-fixture'), urlForm: byId('url-form'), url: byId('url'), dropZone: byId('drop-zone'),
    error: byId('error'), message: byId('message'), transportChip: byId('transport-chip'), title: byId('title'),
    moduleDetail: byId('module-detail'), order: byId('order'), pattern: byId('pattern'), row: byId('row'),
    speed: byId('speed'), bpm: byId('bpm'), previous: byId('previous'), play: byId('play'), stop: byId('stop'),
    next: byId('next'), seekOrder: byId('seek-order'), volume: byId('volume'), volumeValue: byId('volume-value'),
    progress: byId('progress'), elapsed: byId('elapsed'), duration: byId('duration'), repeat: byId('repeat'),
    voices: byId('voices'), masterPeak: byId('master-peak'), channelBody: byId('channel-body'), patternTable: byId('pattern-table'),
    instrumentCount: byId('instrument-count'), instrumentList: byId('instrument-list'), commandTransport: byId('command-transport'),
    telemetryTransport: byId('telemetry-transport'), quantum: byId('quantum'), memory: byId('memory'),
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
};

const state = {
    context: null,
    node: null,
    startPromise: null,
    commandRing: null,
    telemetry: null,
    latest: null,
    fallbackCommands: [],
    pendingLoads: new Map(),
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
    activeModHeadphonePanning: false,
    panningReloadInProgress: false,
    panningReloadRequested: false,
    scrubbing: false,
    pendingSeekFrame: null,
    pendingSeekSequence: 0,
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
    const interpolator = elements.mixerInterpolator.value === 'linear' ? 1 : 0;
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
        elements.modHeadphonePanning.checked = saved.headphoneFriendlyModPanning === true;
        if (typeof saved.sinkId === 'string') elements.outputDevice.dataset.savedSinkId = saved.sinkId;
    } catch (_) {
        // Storage may be disabled or contain data from a broken older development build.
    }
}

function persistPreferences() {
    try {
        localStorage.setItem(MIXER_STORAGE_KEY, JSON.stringify({
            sampleRate: elements.sampleRate.value,
            sinkId: state.context?.sinkId ?? elements.outputDevice.value,
            channels: elements.outputChannels.value,
            path: elements.mixerPath.value,
            interpolator: elements.mixerInterpolator.value,
            depth: elements.mixerDepth.value,
            dither: elements.mixerDither.value,
            headphoneFriendlyModPanning: elements.modHeadphonePanning.checked,
        }));
    } catch (_) {
        // A private or storage-blocked page still gets fully working in-memory controls.
    }
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
    state.node = graph.node;
    state.commandRing = graph.commandRing;
    state.telemetry = graph.telemetry;
    state.workletSampleRate = graph.node.starplayerSampleRate ?? graph.context.sampleRate;
    state.outputChannelCount = graph.channels;
    if (graph.node.starplayerRenderQuantum) elements.quantum.textContent = `${graph.node.starplayerRenderQuantum} frames`;
}

async function prepareNode(context, channels, mixerMode, addModule) {
    if (!context.audioWorklet) {
        throw new Error('AudioWorklet is unavailable here: the player must be served over https or from localhost (a secure context).');
    }
    if (addModule) await context.audioWorklet.addModule(WORKLET_URL);
    const shared = sharedMemoryAvailable() && !elements.forceFallback.checked;
    const commandRing = shared ? Ring.createCommandRing() : null;
    const telemetry = shared ? Ring.createTelemetry() : null;
    const processorOptions = {
        channelCount: channels,
        mixerMode,
        wasmModule: state.wasmModule,
        commandRing: commandRing ? commandRing.buffer : null,
        telemetry: telemetry ? telemetry.buffer : null,
    };
    const node = createNode(context, processorOptions, state.wasmBytes, channels);
    node.port.onmessage = (event) => onWorkletMessage(event.data, node);
    node.onprocessorerror = () => {
        if (node === state.node) showError('The AudioWorklet processor stopped unexpectedly. Reload the page to restart it.');
    };
    return { context, node, commandRing, telemetry, channels };
}

async function startAudio() {
    if (state.node !== null) {
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

async function loadBuffer(buffer, label) {
    const audio = startAudio();
    const previousMessage = elements.message.textContent;
    clearError();
    showMessage(`Checking ${label}…`);
    try {
        await loaderReady;
        const inputBytes = new Uint8Array(buffer);
        if (Loader.is_archive(inputBytes)) {
            const entries = parseArchiveModules(Loader.archive_modules(inputBytes));
            if (entries.length === 0) {
                throw new Error(`No supported modules were found inside ${label}.`);
            }

            // Retained before the pick, so cancelling the modal still leaves this ZIP's
            // other tracks one click away in the load panel.
            const archive = retainArchive(label, inputBytes, entries);
            const choice = entries.length === 1 ? entries[0] : await chooseArchiveEntry(entries, label);
            if (choice === null) {
                audio.catch(() => {});
                showMessage(previousMessage);
                return;
            }
            await playArchiveEntry(archive, choice, audio);
            return;
        }
        await activateLoadedModule(buffer, label, audio);
    } catch (error) {
        reportLoadFailure(label, error);
    }
}

/// Everything that happens once a module's own bytes are known: validate them on the
/// page, refresh the format's effect-name table, then activate on the worklet and play.
/// `loadBuffer` and the retained-archive dropdown are the two routes in, so a track
/// loads identically whichever one the user took. Returns false when a later load has
/// already claimed the module revision and this activation must be discarded.
async function activateLoadedModule(moduleBuffer, moduleLabel, audio) {
    Loader.inspect_s3m(new Uint8Array(moduleBuffer));
    loadEffectNames();
    const metadata = readMetadata(moduleLabel);
    await audio;
    showMessage(`Activating ${moduleLabel}…`);
    const retainedBytes = moduleBuffer.slice(0);
    const revision = ++state.moduleRevision;
    const headphoneFriendlyModPanning = elements.modHeadphonePanning.checked;
    const result = await activateModuleOnNode(state.node, moduleBuffer, headphoneFriendlyModPanning);
    if (revision !== state.moduleRevision) return false;
    state.metadata = metadata;
    state.currentModuleBytes = retainedBytes;
    state.activeModHeadphonePanning = metadata.isMod && headphoneFriendlyModPanning;
    state.activationMemoryBytes = result.memoryBytes;
    state.loadCount += 1;
    renderMetadata();
    setControlsEnabled(true);
    queueCommand(Ring.OPCODE_MASTER_VOLUME, Math.round(Number(elements.volume.value) * 65535 / 100), 0);
    queueRepeatCommand();
    queueCommand(Ring.OPCODE_PLAY, 0, 0);
    showMessage(`Playing ${metadata.title || moduleLabel}.`);

    // The engine applies LoadModule at the next quantum. Collection happens in a
    // later worklet message task, never inside process(). Two attempts cover a busy
    // tab without turning garbage collection into a render-path poll.
    setTimeout(requestGarbageCollection, 100);
    setTimeout(requestGarbageCollection, 500);
    return true;
}

async function playArchiveEntry(archive, choice, audio) {
    showMessage(`Extracting ${choice.name} from ${archive.label}…`);
    const extracted = Loader.archive_extract(archive.bytes, choice.index);
    const moduleBuffer = Uint8Array.from(extracted).buffer;
    const activated = await activateLoadedModule(moduleBuffer, `${choice.name} (from ${archive.label})`, audio);
    if (activated) selectRetainedArchiveTrack(archive, choice);
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
    const audio = startAudio();
    clearError();
    playArchiveEntry(archive, choice, audio).catch((error) => reportLoadFailure(archive.label, error));
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

function activateModuleOnNode(node, buffer, headphoneFriendlyModPanning = elements.modHeadphonePanning.checked) {
    const requestId = state.nextRequestId++;
    return new Promise((resolve, reject) => {
        state.pendingLoads.set(requestId, { resolve, reject });
        updateModPanningAvailability();
        node.port.postMessage({ type: 'loadModule', requestId, bytes: buffer, headphoneFriendlyModPanning }, [buffer]);
    });
}

/// The panning toggle reloads the module the page is holding. Doing that while another
/// load is already in flight interleaves two `loadModule` messages on one FIFO port, so
/// the option is unavailable until the port is quiet again — on the error path too, which
/// is why this reads the map rather than being set and cleared by hand.
function updateModPanningAvailability() {
    elements.modHeadphonePanning.disabled = state.pendingLoads.size > 0 || state.panningReloadInProgress;
}

function onWorkletMessage(message, node) {
    if (message.type === 'ready') {
        node.starplayerSampleRate = message.sampleRate;
        node.starplayerRenderQuantum = message.renderQuantum;
        if (node === state.node) {
            state.workletSampleRate = message.sampleRate;
            elements.quantum.textContent = `${message.renderQuantum} frames`;
            updateOutputPanel();
        }
    } else if (message.type === 'quantum') {
        if (node !== state.node) return;
        elements.quantum.textContent = `${message.frames} frames${message.matchesEngine ? '' : ' (unexpected)'}`;
    } else if (message.type === 'telemetry') {
        if (node === state.node) state.latest = message;
    } else if (message.type === 'moduleLoaded' || message.type === 'moduleError') {
        const pending = state.pendingLoads.get(message.requestId);
        if (pending) {
            state.pendingLoads.delete(message.requestId);
            updateModPanningAvailability();
            if (message.type === 'moduleLoaded') pending.resolve(message);
            else pending.reject(new Error(message.reason));
        }
    } else if (message.type === 'garbageCollected') {
        if (node !== state.node) return;
        state.retiredSeen = message.total;
        elements.retired.textContent = `${state.retiredSeen} returned, ${message.pending} pending`;
        if (state.loadCount > 1 && message.collected > 0) {
            showMessage(`Playing ${state.metadata.title}; the retired module Arc returned off the audio callback.`);
        }
    } else if (message.type === 'fault') {
        if (node === state.node) showError(`Audio engine fault: ${message.reason}`);
    } else if (message.type === 'mixerModeApplied') {
        if (node !== state.node) return;
        state.activeModeWire = message.active >>> 0;
        state.activationMemoryBytes = message.memoryBytes;
        updateOutputPanel();
        showMessage(`Mixer active: ${describeMixerMode(state.activeModeWire)}.`);
    } else if (message.type === 'mixerModeError') {
        if (node === state.node) showError(`Could not switch mixer mode: ${message.reason}`);
    }
}

function requestGarbageCollection() {
    if (state.node !== null) {
        state.node.port.postMessage({ type: 'collectGarbage' });
    }
}

function queueCommand(opcode, argument = 0, extra = 0) {
    if (state.node === null) return;
    if (state.commandRing !== null) {
        if (!Ring.pushCommand(state.commandRing, opcode, argument, extra)) {
            showError('The audio command ring is full. The newest control change will be retried.');
            state.fallbackCommands.push(opcode, argument, extra);
        }
    } else {
        state.fallbackCommands.push(opcode, argument, extra);
    }
}

function repeatFadeFrames() {
    return Math.round(SONG_FADE_SECONDS * (state.workletSampleRate || 44100));
}

/// Sent on `#repeat` `change`, on activating a module, and from `playbackRestoreCommands()`
/// — the three moments the engine's at-end policy needs to (re)match the checkbox.
function queueRepeatCommand() {
    queueCommand(Ring.OPCODE_AT_END, elements.repeat.checked ? Ring.AT_END_CONTINUE : Ring.AT_END_FADE_OUT, repeatFadeFrames());
}

function flushFallbackCommands() {
    if (state.node === null || state.fallbackCommands.length === 0) return;
    if (state.commandRing !== null) {
        const remaining = [];
        let modeQueued = false;
        for (let index = 0; index < state.fallbackCommands.length; index += 3) {
            if (!Ring.pushCommand(state.commandRing, state.fallbackCommands[index], state.fallbackCommands[index + 1], state.fallbackCommands[index + 2])) {
                remaining.push(...state.fallbackCommands.slice(index));
                break;
            }
            modeQueued ||= state.fallbackCommands[index] === Ring.OPCODE_SET_MIXER_MODE;
        }
        state.fallbackCommands = remaining;
        if (modeQueued) state.node.port.postMessage({ type: 'flushCommands' });
        return;
    }
    const commands = state.fallbackCommands;
    state.fallbackCommands = [];
    state.node.port.postMessage({ type: 'commandBatch', commands });
}

function sendCommandsToGraph(graph, commands) {
    if (graph.commandRing !== null) {
        for (let index = 0; index < commands.length; index += 3) {
            if (!Ring.pushCommand(graph.commandRing, commands[index], commands[index + 1], commands[index + 2])) {
                throw new Error('The new audio graph command ring filled while restoring playback.');
            }
        }
    } else {
        graph.node.port.postMessage({ type: 'commandBatch', commands });
    }
}

function playbackRestoreCommands() {
    const playing = state.latest?.playing ?? state.metadata !== null;
    const volume = Math.round(Number(elements.volume.value) * 65535 / 100);
    // A rebuilt graph starts its module from the top. Put it back where the ear left it:
    // by song frame when the timeline is known, which lands mid-pattern, and by order
    // otherwise.
    const lengthKnown = ((state.latest?.songFlags ?? 0) & Ring.SONG_FLAG_LENGTH_KNOWN) !== 0;
    const seek = lengthKnown
        ? [Ring.OPCODE_SEEK_FRAME, Math.max(0, state.latest?.songFrame ?? 0), 0]
        : [Ring.OPCODE_SEEK_ORDER, state.latest?.order ?? 0, 0];
    const commands = [
        Ring.OPCODE_MASTER_VOLUME, volume, 0,
        ...seek,
        Ring.OPCODE_AT_END, elements.repeat.checked ? Ring.AT_END_CONTINUE : Ring.AT_END_FADE_OUT, repeatFadeFrames(),
        playing ? Ring.OPCODE_PLAY : Ring.OPCODE_STOP, 0, 0,
    ];
    for (const [channel, snapshot] of (state.latest?.channels ?? []).entries()) {
        if (snapshot.muted) commands.push(Ring.OPCODE_MUTE_CHANNEL, channel, 1);
    }
    return commands;
}

function sendCommandsToCurrentGraph(commands) {
    for (let index = 0; index < commands.length; index += 3) {
        queueCommand(commands[index], commands[index + 1], commands[index + 2]);
    }
}

async function applyModPanningPreference() {
    const requested = elements.modHeadphonePanning.checked;
    if (state.panningReloadInProgress) {
        // A disabled checkbox cannot normally emit another user change, but keeping this
        // guard deterministic also covers scripted changes and a racing load interaction.
        elements.modHeadphonePanning.checked = state.panningReloadRequested;
        persistPreferences();
        return;
    }
    persistPreferences();
    if (!state.metadata?.isMod || state.currentModuleBytes === null || state.node === null) {
        showMessage(requested
            ? 'Headphone-friendly MOD panning saved for the next MOD load.'
            : 'Authentic MOD panning saved for the next MOD load.');
        return;
    }

    const previous = state.activeModHeadphonePanning;
    // Claim a revision exactly as the load path does. Reusing the current value let a
    // concurrent load and this reload both believe they were last: the port is a FIFO, so
    // the module posted second is the one that sounds, and whichever handler ran second
    // would then paint the other module's metadata over it.
    const revision = ++state.moduleRevision;
    const node = state.node;
    const restoreCommands = playbackRestoreCommands();
    state.panningReloadInProgress = true;
    state.panningReloadRequested = requested;
    updateModPanningAvailability();
    clearError();
    showMessage(`Reloading this MOD with ${requested ? 'S3M-style 60% spacing' : 'authentic hard panning'}…`);
    try {
        const activation = await activateModuleOnNode(node, state.currentModuleBytes.slice(0), requested);
        if (revision !== state.moduleRevision || node !== state.node) return;
        state.activeModHeadphonePanning = requested;
        state.activationMemoryBytes = activation.memoryBytes;
        state.loadCount += 1;
        sendCommandsToCurrentGraph(restoreCommands);
        persistPreferences();
        showMessage(`${requested ? 'Headphone-friendly S3M-style 60% spacing' : 'Authentic hard MOD panning'} applied; playback restored at the sounding order.`);
        setTimeout(requestGarbageCollection, 100);
        setTimeout(requestGarbageCollection, 500);
    } catch (error) {
        // The preference was persisted before the reload was attempted, so it has to be
        // put back whatever else has happened since — a revision check here would leave a
        // failed toggle stored as the user's choice for every future session.
        elements.modHeadphonePanning.checked = previous;
        persistPreferences();
        if (revision === state.moduleRevision && node === state.node) {
            showError(`Could not change MOD panning: ${error && error.message ? error.message : error}`);
            showMessage(`Still playing ${state.metadata.title} with ${previous ? 'headphone-friendly 60% spacing' : 'authentic hard panning'}.`);
        }
    } finally {
        state.panningReloadInProgress = false;
        state.panningReloadRequested = false;
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
    const oldSinkId = 'sinkId' in oldContext ? oldContext.sinkId : '';
    const restoreCommands = playbackRestoreCommands();
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
        const activation = state.currentModuleBytes === null
            ? null
            : await activateModuleOnNode(graph.node, state.currentModuleBytes.slice(0), elements.modHeadphonePanning.checked);
        if (oldSinkId && typeof candidateContext.setSinkId === 'function') {
            await candidateContext.setSinkId(oldSinkId).catch((error) => {
                elements.outputDeviceNote.textContent = `The previous output device could not be restored (${error.name || error.message}).`;
            });
        }
        await unlock;
        graph.node.connect(candidateContext.destination);
        if (state.currentModuleBytes !== null) sendCommandsToGraph(graph, restoreCommands);
        state.context = candidateContext;
        installGraph(graph);
        state.latest = null;
        state.activeModeWire = mixerModeFromControls();
        if (activation) state.activationMemoryBytes = activation.memoryBytes;
        if (activation && state.metadata?.isMod) state.activeModHeadphonePanning = elements.modHeadphonePanning.checked;
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
    const restoreCommands = playbackRestoreCommands();
    let candidateNode = null;
    try {
        const graph = await prepareNode(state.context, channels, mixerModeFromControls(), false);
        candidateNode = graph.node;
        const activation = state.currentModuleBytes === null
            ? null
            : await activateModuleOnNode(graph.node, state.currentModuleBytes.slice(0), elements.modHeadphonePanning.checked);
        graph.node.connect(state.context.destination);
        if (state.currentModuleBytes !== null) sendCommandsToGraph(graph, restoreCommands);
        installGraph(graph);
        state.latest = null;
        state.activeModeWire = mixerModeFromControls();
        if (activation) state.activationMemoryBytes = activation.memoryBytes;
        if (activation && state.metadata?.isMod) state.activeModHeadphonePanning = elements.modHeadphonePanning.checked;
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
    const wire = mixerModeFromControls(state.node === null ? undefined : state.outputChannelCount);
    if (state.node === null) {
        state.activeModeWire = wire;
        updateOutputPanel();
        showMessage('Mixer choice saved; it will be used when audio starts.');
        return;
    }
    queueCommand(Ring.OPCODE_SET_MIXER_MODE, wire, 0);
    if (state.commandRing !== null) state.node.port.postMessage({ type: 'flushCommands' });
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
    elements.instrumentList.replaceChildren();
    for (const instrument of metadata.instruments) {
        const item = document.createElement('li');
        const name = document.createElement('span');
        const length = document.createElement('small');
        name.textContent = instrument.name || '(empty slot)';
        length.textContent = instrument.length > 0 ? `${instrument.length.toLocaleString()} frames` : 'no PCM';
        item.append(name, length);
        elements.instrumentList.append(item);
    }
    if (metadata.instruments.length === 0) {
        elements.instrumentList.innerHTML = '<li class="empty">No instruments.</li>';
    }
    ensureChannelRows(metadata.channels);
    state.lastPatternKey = '';
}

function setControlsEnabled(enabled) {
    for (const control of [elements.previous, elements.play, elements.stop, elements.next, elements.seekOrder, elements.volume, elements.progress, elements.repeat]) {
        control.disabled = !enabled;
    }
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
        const effect = document.createElement('td');
        const action = document.createElement('td');
        const vuTrack = document.createElement('span');
        const vuFill = document.createElement('span');
        const mute = document.createElement('button');
        channel.textContent = String(index + 1).padStart(2, '0');
        instrument.className = 'instrument';
        effect.className = 'effect';
        vuTrack.className = 'vu-track';
        vuFill.className = 'vu-fill';
        vuTrack.append(vuFill);
        vuCell.append(vuTrack);
        mute.textContent = 'Mute';
        mute.addEventListener('click', () => {
            const muted = mute.dataset.muted !== 'true';
            queueCommand(Ring.OPCODE_MUTE_CHANNEL, index, muted ? 1 : 0);
        });
        action.append(mute);
        row.append(channel, instrument, note, volume, pan, vuCell, effect, action);
        elements.channelBody.append(row);
        state.channelRows.push({ row, instrument, note, volume, pan, vuFill, effect, mute });
    }
}

function updateChannels(snapshot) {
    ensureChannelRows(snapshot.channelCount);
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
    // With Repeat off a looping song plays on past its loop point under the fade, so the
    // slider covers the fade too: it always represents one complete playback.
    const fadesAtEnd = !elements.repeat.checked && (snapshot.songFlags & Ring.SONG_FLAG_LOOPS) !== 0;
    const lengthFrames = lengthKnown ? Math.max(0, snapshot.songLengthFrames) + (fadesAtEnd ? repeatFadeFrames() : 0) : null;
    elements.progress.max = String(lengthFrames ?? 0);
    elements.progress.disabled = !lengthKnown;
    setText(elements.duration, formatTime(lengthFrames, rate));

    const endReachedStopped = (snapshot.songFlags & Ring.SONG_FLAG_END_REACHED) !== 0 && !snapshot.playing;
    if (endReachedStopped) {
        state.pendingSeekFrame = null;
        if (state.scrubbing) return;
        elements.progress.value = '0';
        setText(elements.elapsed, '0:00');
        return;
    }
    if (state.pendingSeekFrame !== null && snapshot.sequence <= state.pendingSeekSequence) {
        if (state.scrubbing) return;
        elements.progress.value = String(state.pendingSeekFrame);
        setText(elements.elapsed, formatTime(state.pendingSeekFrame, rate));
        return;
    }
    state.pendingSeekFrame = null;
    if (state.scrubbing) return;
    let frame = Math.max(0, snapshot.songFrame);
    if (lengthFrames !== null) frame = Math.min(frame, lengthFrames);
    elements.progress.value = String(frame);
    setText(elements.elapsed, formatTime(frame, rate));
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
    elements.transportChip.textContent = snapshot.playing ? 'playing' : 'stopped';
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

async function loadFixture() {
    const name = elements.fixturePicker.value;
    if (!name) {
        showError('Choose a bundled fixture first.');
        return;
    }
    const response = await fetch(`modules/${name}`);
    if (!response.ok) throw new Error(`Fixture request failed (${response.status}).`);
    await loadBuffer(await response.arrayBuffer(), name);
}

elements.startAudio.addEventListener('click', () => startAudio().catch(() => {}));
elements.filePicker.addEventListener('change', () => {
    const file = elements.filePicker.files[0];
    if (file) file.arrayBuffer().then((buffer) => loadBuffer(buffer, file.name)).catch(showError);
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
    fetch(url).then((response) => {
        if (!response.ok) throw new Error(`URL request failed (${response.status} ${response.statusText}).`);
        return response.arrayBuffer();
    }).then((buffer) => loadBuffer(buffer, url)).catch((error) => showError(`Could not fetch that URL. It may need CORS permission. ${error.message}`));
});
for (const type of ['dragenter', 'dragover']) {
    elements.dropZone.addEventListener(type, (event) => { event.preventDefault(); elements.dropZone.classList.add('over'); });
}
for (const type of ['dragleave', 'drop']) {
    elements.dropZone.addEventListener(type, (event) => { event.preventDefault(); elements.dropZone.classList.remove('over'); });
}
elements.dropZone.addEventListener('drop', (event) => {
    const file = event.dataTransfer.files[0];
    if (file) file.arrayBuffer().then((buffer) => loadBuffer(buffer, file.name)).catch(showError);
});
elements.dropZone.addEventListener('keydown', (event) => {
    if (event.key === 'Enter' || event.key === ' ') elements.filePicker.click();
});
elements.play.addEventListener('click', () => queueCommand(Ring.OPCODE_PLAY));
elements.stop.addEventListener('click', () => queueCommand(Ring.OPCODE_STOP));
elements.previous.addEventListener('click', () => queueCommand(Ring.OPCODE_SEEK_ORDER, Math.max(0, (state.latest?.order || 0) - 1)));
elements.next.addEventListener('click', () => queueCommand(Ring.OPCODE_SEEK_ORDER, Math.min((state.metadata?.orders || 1) - 1, (state.latest?.order || 0) + 1)));
elements.seekOrder.addEventListener('change', () => queueCommand(Ring.OPCODE_SEEK_ORDER, Number(elements.seekOrder.value)));
// Continuous `input` events while dragging never reach the command ring — it holds only
// 64 entries and is drained once per render quantum — only the released `change` does.
elements.progress.addEventListener('input', () => {
    state.scrubbing = true;
    setText(elements.elapsed, formatTime(Number(elements.progress.value), state.workletSampleRate));
});
elements.progress.addEventListener('change', () => {
    const frame = Number(elements.progress.value);
    queueCommand(Ring.OPCODE_SEEK_FRAME, frame);
    state.pendingSeekFrame = frame;
    state.pendingSeekSequence = state.latest?.sequence ?? 0;
    state.scrubbing = false;
});
elements.repeat.addEventListener('change', queueRepeatCommand);
elements.volume.addEventListener('input', () => {
    const percent = Number(elements.volume.value);
    elements.volumeValue.value = `${percent}%`;
    queueCommand(Ring.OPCODE_MASTER_VOLUME, Math.round(percent * 65535 / 100));
});

elements.applySampleRate.addEventListener('click', () => rebuildAudioContext().catch(showError));
elements.applyChannels.addEventListener('click', () => rebuildOutputChannels().catch(showError));
elements.applyOutputDevice.addEventListener('click', () => applyOutputDevice().catch(showError));
elements.chooseOutputDevice.addEventListener('click', () => chooseOutputDevice().catch(showError));
elements.applyMixer.addEventListener('click', applyMixerMode);
elements.modHeadphonePanning.addEventListener('change', () => applyModPanningPreference().catch(showError));
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
}
restorePreferences();
state.activeModeWire = mixerModeFromControls();
updateOutputPanel();
refreshOutputDevices().catch(() => {});
requestAnimationFrame(refresh);

// `app.js` is a module and therefore deferred. This is the one honest signal that its
// listeners are attached, which is what the headless harness waits on before clicking.
document.documentElement.dataset.playerReady = 'true';
