import initLoader, * as Loader from './starplayer_web.js';

const Ring = globalThis.StarPlayerRing;
const HOST_WASM_URL = 'starplayer_host_wasm_bg.wasm';
const WORKLET_URL = 'starplayer-worklet.js';
const PROCESSOR_NAME = 'starplayer-player';
const OUTPUT_CHANNELS = 2;
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
    voices: byId('voices'), masterPeak: byId('master-peak'), channelBody: byId('channel-body'), patternTable: byId('pattern-table'),
    instrumentCount: byId('instrument-count'), instrumentList: byId('instrument-list'), commandTransport: byId('command-transport'),
    telemetryTransport: byId('telemetry-transport'), quantum: byId('quantum'), memory: byId('memory'),
    dropped: byId('dropped'), retired: byId('retired'), health: byId('health'), forceFallback: byId('force-fallback'),
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
    state.context = new AudioContext({ latencyHint: 'interactive' });
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
        await state.context.audioWorklet.addModule(WORKLET_URL);
        await unlock;

        const shared = sharedMemoryAvailable() && !elements.forceFallback.checked;
        state.commandRing = shared ? Ring.createCommandRing() : null;
        state.telemetry = shared ? Ring.createTelemetry() : null;
        const processorOptions = {
            channelCount: OUTPUT_CHANNELS,
            wasmModule,
            commandRing: state.commandRing ? state.commandRing.buffer : null,
            telemetry: state.telemetry ? state.telemetry.buffer : null,
        };
        state.node = createNode(processorOptions, wasmBytes);
        state.node.port.onmessage = (event) => onWorkletMessage(event.data);
        state.node.onprocessorerror = () => showError('The AudioWorklet processor stopped unexpectedly. Reload the page to restart it.');
        state.node.connect(state.context.destination);

        elements.commandTransport.textContent = state.commandRing ? 'SharedArrayBuffer SPSC ring' : 'postMessage, batched once per frame';
        elements.telemetryTransport.textContent = state.telemetry ? 'SharedArrayBuffer seqlock' : `postMessage every ${Ring.TELEMETRY_FALLBACK_QUANTA} quanta`;
        elements.transportChip.textContent = 'audio ready';
        elements.transportChip.classList.add('live');
        elements.startAudio.textContent = 'Audio ready';
        elements.forceFallback.disabled = true;
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

function createNode(processorOptions, wasmBytes) {
    const options = {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [OUTPUT_CHANNELS],
        processorOptions,
    };
    try {
        return new AudioWorkletNode(state.context, PROCESSOR_NAME, options);
    } catch (error) {
        options.processorOptions = { ...processorOptions, wasmModule: null, wasmBytes };
        return new AudioWorkletNode(state.context, PROCESSOR_NAME, options);
    }
}

async function loadBuffer(buffer, label) {
    const audio = startAudio();
    clearError();
    showMessage(`Checking ${label}…`);
    try {
        await loaderReady;
        Loader.inspect_s3m(new Uint8Array(buffer));
        const metadata = readMetadata(label);
        await audio;
        showMessage(`Activating ${label}…`);
        const result = await activateModule(buffer);
        state.metadata = metadata;
        state.activationMemoryBytes = result.memoryBytes;
        state.loadCount += 1;
        renderMetadata();
        setControlsEnabled(true);
        queueCommand(Ring.OPCODE_MASTER_VOLUME, Math.round(Number(elements.volume.value) * 65535 / 100), 0);
        queueCommand(Ring.OPCODE_PLAY, 0, 0);
        showMessage(`Playing ${metadata.title || label}.`);

        // The engine applies LoadModule at the next quantum. Collection happens in a
        // later worklet message task, never inside process(). Two attempts cover a busy
        // tab without turning garbage collection into a render-path poll.
        setTimeout(requestGarbageCollection, 100);
        setTimeout(requestGarbageCollection, 500);
    } catch (error) {
        showError(`Could not load ${label}: ${error && error.message ? error.message : error}`);
        showMessage(state.metadata ? `Still playing ${state.metadata.title}.` : 'The player is ready for another file.');
    }
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
        instruments,
    };
}

function activateModule(buffer) {
    const requestId = state.nextRequestId++;
    return new Promise((resolve, reject) => {
        state.pendingLoads.set(requestId, { resolve, reject });
        state.node.port.postMessage({ type: 'loadModule', requestId, bytes: buffer }, [buffer]);
    });
}

function onWorkletMessage(message) {
    if (message.type === 'ready') {
        elements.quantum.textContent = `${message.renderQuantum} frames`;
    } else if (message.type === 'quantum') {
        elements.quantum.textContent = `${message.frames} frames${message.matchesEngine ? '' : ' (unexpected)'}`;
    } else if (message.type === 'telemetry') {
        state.latest = message;
    } else if (message.type === 'moduleLoaded' || message.type === 'moduleError') {
        const pending = state.pendingLoads.get(message.requestId);
        if (pending) {
            state.pendingLoads.delete(message.requestId);
            if (message.type === 'moduleLoaded') pending.resolve(message);
            else pending.reject(new Error(message.reason));
        }
    } else if (message.type === 'garbageCollected') {
        state.retiredSeen = message.total;
        elements.retired.textContent = `${state.retiredSeen} returned, ${message.pending} pending`;
        if (state.loadCount > 1 && message.collected > 0) {
            showMessage(`Playing ${state.metadata.title}; the retired module Arc returned off the audio callback.`);
        }
    } else if (message.type === 'fault') {
        showError(`Audio engine fault: ${message.reason}`);
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

function flushFallbackCommands() {
    if (state.node === null || state.fallbackCommands.length === 0) return;
    if (state.commandRing !== null) {
        const remaining = [];
        for (let index = 0; index < state.fallbackCommands.length; index += 3) {
            if (!Ring.pushCommand(state.commandRing, state.fallbackCommands[index], state.fallbackCommands[index + 1], state.fallbackCommands[index + 2])) {
                remaining.push(...state.fallbackCommands.slice(index));
                break;
            }
        }
        state.fallbackCommands = remaining;
        return;
    }
    const commands = state.fallbackCommands;
    state.fallbackCommands = [];
    state.node.port.postMessage({ type: 'commandBatch', commands });
}

function renderMetadata() {
    const metadata = state.metadata;
    elements.title.textContent = metadata.title;
    elements.moduleDetail.textContent = `${metadata.channels} channels · ${metadata.orders} orders · ${metadata.patterns} patterns · ${metadata.instruments.length} instruments`;
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
    for (const control of [elements.previous, elements.play, elements.stop, elements.next, elements.seekOrder, elements.volume]) {
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

function updateSnapshot(snapshot) {
    if (!snapshot || snapshot.sequence === 0 || !state.metadata) return;
    state.latest = snapshot;
    setText(elements.order, `${snapshot.order + 1}/${state.metadata.orders}`);
    setText(elements.pattern, String(snapshot.pattern));
    setText(elements.row, `${String(snapshot.row).padStart(2, '0')}:${snapshot.tick}`);
    setText(elements.speed, String(snapshot.speed));
    setText(elements.bpm, String(snapshot.bpm));
    elements.seekOrder.value = String(snapshot.order);
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
    if (code === 0) return '';
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
elements.volume.addEventListener('input', () => {
    const percent = Number(elements.volume.value);
    elements.volumeValue.value = `${percent}%`;
    queueCommand(Ring.OPCODE_MASTER_VOLUME, Math.round(percent * 65535 / 100));
});

elements.forceFallback.disabled = !sharedMemoryAvailable();
if (!sharedMemoryAvailable()) {
    elements.forceFallback.checked = true;
    elements.commandTransport.textContent = 'postMessage fallback (COOP/COEP unavailable)';
    elements.telemetryTransport.textContent = 'postMessage fallback (COOP/COEP unavailable)';
}
requestAnimationFrame(refresh);

// `app.js` is a module and therefore deferred. This is the one honest signal that its
// listeners are attached, which is what the headless harness waits on before clicking.
document.documentElement.dataset.playerReady = 'true';
