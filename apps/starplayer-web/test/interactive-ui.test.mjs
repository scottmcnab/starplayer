import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFile } from 'node:fs/promises';
const app = await readFile(new URL('../www/app.js', import.meta.url), 'utf8');
const ring = await readFile(new URL('../www/ring.js', import.meta.url), 'utf8');
function source(name) {
    const start = app.indexOf(`function ${name}(`);
    const end = app.indexOf('\n}\n', start) + 3;
    assert.ok(start >= 0 && end > start);
    return app.slice(start, end);
}
class Element {
    constructor(tag = '') { this.tag = tag; this.children = []; this.listeners = {}; this.dataset = {}; this.attributes = {}; this.style = {}; }
    append(...children) { this.children.push(...children); }
    replaceChildren(...children) { this.children = children; }
    addEventListener(name, listener) { this.listeners[name] = listener; }
    setAttribute(name, value) { this.attributes[name] = value; }
    setPointerCapture() {}
    fire(name, event = {}) { this.listeners[name]?.({ button: 0, target: this, preventDefault() {}, ...event }); }
    closest(selector) { return selector.split(', ').includes(this.tag) ? this : null; }
    querySelectorAll(selector) {
        return this.children.flatMap(child => typeof child === 'object' ? [...(selector === child.tag || selector === `.${child.className}` ? [child] : []), ...child.querySelectorAll(selector)] : []);
    }
    querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
}
function instrumentHarness(capabilities = [11, 31, 29, 3, 0, 2, 6]) {
    const records = [];
    const elements = Object.fromEntries(['title', 'moduleDetail', 'seekOrder', 'instrumentCount', 'instrumentList', 'instrumentSelectAll', 'keyboardEnable', 'inputDestination', 'overlapEnable', 'uniqueNotes'].map(key => [key, new Element()]));
    const state = { metadata: { title: 'native IT', label: 'fixture.it', channels: 64, orders: 1, patterns: 1, instruments: capabilities.map((capability, index) => ({ capability, length: capability ? 123 + index : 0, name: `instrument${index}` })) }, selectedInstrument: null, referenceNote: 60, interactiveReady: true, letNotesRing: [true], overlapEnabled: false, fallbackCommands: [], keysDown: new Map(), context: null };
    const sandbox = { state, elements, formatNote: note => `C-${Math.floor(note / 12)}`, document: { createElement: tag => new Element(tag) }, backgroundOutput: { start() {} }, sizeInstrumentColumn() {}, ensureChannelRows() {}, queueInteractive: (...record) => records.push(record), flushFallbackCommands() {}, releaseHeldKeys() {}, syncLiveInput() {}, updateLiveInputPanel() {} };
    vm.createContext(sandbox); vm.runInContext(ring, sandbox); sandbox.Ring = sandbox.StarPlayerRing;
    vm.runInContext(['renderMetadata', 'updateInstrumentControls', 'setAllInstrumentOverlap', 'selectInstrument', 'cleanupLiveInput'].map(source).join('\n'), sandbox);
    sandbox.renderMetadata();
    return Object.assign(sandbox, { records, rows: elements.instrumentList.children });
}

test('compact semantic table keeps independent PCM, live, reference-preview, loop and overlap facts', async () => {
    const { rows, state, elements } = instrumentHarness();
    assert.equal(rows[0].tag, 'tr');
    assert.equal(rows[0].children.length, 6);
    assert.equal(rows[0].children[0].textContent, '01');
    assert.equal(rows[0].querySelector('input').checked, true);
    assert.equal(rows[0].querySelector('input').disabled, false, 'bypassed overlap remains editable');
    assert.equal(rows[1].querySelector('input').disabled, false);
    assert.match(rows[1].children[2].attributes['aria-label'], /Mixed map/);
    assert.equal(rows[1].children[2].textContent, '∞');
    assert.equal(rows[2].querySelector('input'), null, 'ineligible cells stay empty');
    assert.equal(rows[3].dataset.selectable, 'true', 'another key is playable');
    assert.equal(rows[3].querySelector('button').disabled, true, 'reference60 has no viable sample');
    assert.equal(rows[4].dataset.selectable, 'false');
    assert.equal(rows[4].querySelector('button'), null, 'no play icon without PCM');
    assert.equal(rows[4].children[5].textContent, '');
    assert.equal(rows[5].dataset.selectable, 'false', 'native low-map instrument has no live-tunable key');
    assert.equal(rows[5].querySelector('button').disabled, true, 'PCM still has a disabled play control');
    assert.equal(rows[5].querySelector('input').disabled, false, 'song one-shot PCM remains eligible');
    assert.equal(rows[5].attributes['aria-disabled'], undefined, 'unselectable rows must not semantically disable eligible native descendants');
    assert.match(rows[5].attributes['aria-label'], /selection unavailable/);
    assert.equal(rows[5].children[2].textContent, '');
    assert.equal(rows[5].children[5].textContent, '128');
    assert.match(rows[6].children[2].attributes['aria-label'], /Mixed map/);
    assert.match(rows[0].querySelector('label').title, /Unchecked preserves native behavior, including IT NNA/);
    assert.equal(elements.instrumentSelectAll.indeterminate, true);
    state.metadata.instruments[0].name = ''; // A new module rebuild gives empty-name fallbacks.
    state.metadata.instruments = [...state.metadata.instruments];
    const html = await readFile(new URL('../www/index.html', import.meta.url), 'utf8');
    assert.match(html, /<th scope="col">#<\/th><th scope="col">Name<\/th><th scope="col" aria-label="Loop"><\/th><th scope="col"><label[^>]+><input id="instrument-select-all"[^>]+><\/label><\/th><th scope="col" aria-label="Play"><\/th><th scope="col"[^>]+>Frames<\/th>/);
});

test('row activation retains native controls and same-module refresh preserves accepted selection and readiness', () => {
    const harness = instrumentHarness();
    const { rows, state, elements } = harness;
    const row = rows[0], preview = row.querySelector('button'), checkbox = row.querySelector('input');
    row.fire('keydown', { key: 'Enter', target: preview });
    assert.equal(state.selectedInstrument, null, 'native controls never activate the row');
    row.fire('keydown', { key: 'Enter' });
    assert.equal(state.selectedInstrument, 0);
    assert.equal(row.attributes['aria-selected'], 'true');
    assert.equal(state.keyboardInstrument, 0);
    assert.equal(elements.inputDestination.value, 'selected');
    harness.renderMetadata();
    assert.equal(state.instrumentRows[0].preview, preview);
    assert.equal(state.instrumentRows[0].checkbox, checkbox);
    rows[5].fire('keydown', { key: ' ' });
    assert.equal(state.selectedInstrument, 0, 'unplayable rows remain gated');
    state.interactiveReady = false; harness.renderMetadata();
    assert.equal(preview.disabled, true);
    assert.match(preview.title, /not ready/);
    assert.equal(checkbox.disabled, true);
    assert.equal(elements.instrumentSelectAll.disabled, true);
    state.metadata = { ...state.metadata, instruments: [] }; harness.renderMetadata();
    assert.match(elements.instrumentList.innerHTML, /colspan="6">No instruments/);
    assert.equal(elements.instrumentSelectAll.checked, false);
    assert.equal(elements.instrumentSelectAll.indeterminate, false);
    assert.equal(elements.instrumentSelectAll.disabled, true);
    state.metadata.instruments = [{ capability: 0, length: 0, name: '' }]; harness.renderMetadata();
    assert.equal(state.instrumentRows[0].item.querySelector('.instrument-name').textContent, '(instrument 1)');
});

test('one-shot Preview-up finishes; looping Preview-up releases; cancel and focus loss clear live scope', () => {
    const { rows, records, Ring } = instrumentHarness();
    const shot = rows[0].querySelector('button'), loop = rows[2].querySelector('button');
    shot.fire('pointerdown'); shot.fire('pointerup'); shot.fire('lostpointercapture');
    assert.deepEqual(records, [[Ring.OPCODE_PREVIEW_NOTE, 0, 60]]);
    loop.fire('pointerdown'); loop.fire('pointerup'); loop.fire('lostpointercapture');
    assert.deepEqual(records.slice(1), [[Ring.OPCODE_PREVIEW_NOTE, 2, 60], [Ring.OPCODE_RELEASE_PREVIEW]]);
    loop.fire('keydown', { key: ' ', repeat: false }); loop.fire('keyup', { key: ' ' }); loop.fire('click', { detail: 0 });
    assert.deepEqual(records.slice(3), [[Ring.OPCODE_PREVIEW_NOTE, 2, 60], [Ring.OPCODE_RELEASE_PREVIEW]], 'keyboard activation never duplicates the click');
    shot.fire('pointerdown'); shot.fire('pointercancel');
    assert.equal(records.at(-1)[0], Ring.OPCODE_STOP_PREVIEW);
    loop.fire('pointerdown'); loop.fire('blur');
    assert.equal(records.at(-1)[0], Ring.OPCODE_STOP_PREVIEW);
});

test('full outer bridge queues recover with a bounded identity-scoped live clear before accepting more notes', () => {
    for (const shared of [true, false]) {
        const sent = [];
        const sandbox = { SharedArrayBuffer, Int32Array, Atomics, state: { interactiveReady: true, liveInputActive: true, liveRecoveryPending: false, fallbackCommands: [], keysDown: new Map([['KeyZ', 60]]), eventsSent: 0, node: { starplayerIdentity: { session: 3n, build: (1n << 60n) + 2n }, port: { postMessage: message => sent.push(message) } } } };
        vm.createContext(sandbox); vm.runInContext(ring, sandbox); sandbox.Ring = sandbox.StarPlayerRing;
        sandbox.state.commandRing = shared ? sandbox.Ring.createCommandRing(2) : null;
        vm.runInContext(['queueCommand', 'queueInteractive', 'queueMidiMessage', 'flushFallbackCommands'].map(source).join('\n'), sandbox);
        for (let index = 0; index < 1000; index++) sandbox.queueMidiMessage(0x90, 60, 127);
        assert.equal(sandbox.state.liveRecoveryPending, true);
        assert.equal(sandbox.state.keysDown.size, 0);
        assert.ok(sandbox.state.fallbackCommands.length <= 129);
        const applied = [];
        for (let index = 0; index < 10; index++) {
            sandbox.flushFallbackCommands();
            if (shared) sandbox.Ring.drainCommands(sandbox.state.commandRing, (...record) => applied.push(record));
            else while (sent.length) { const batch = sent.shift(); sandbox.Ring.drainFallbackCommands(batch.commands, (...record) => applied.push(record), batch.version); }
        }
        assert.equal(applied.at(-1)[0], sandbox.Ring.OPCODE_STOP_PREVIEW);
        const count = applied.length;
        sandbox.queueMidiMessage(0x90, 67, 127);
        assert.equal(sandbox.state.fallbackCommands.length, 0, 'input waits for the reliable clear acknowledgement');
        sandbox.state.liveRecoveryPending = false;
        sandbox.queueMidiMessage(0x90, 67, 127); sandbox.flushFallbackCommands();
        if (shared) sandbox.Ring.drainCommands(sandbox.state.commandRing, (...record) => applied.push(record));
        else { const batch = sent.shift(); sandbox.Ring.drainFallbackCommands(batch.commands, (...record) => applied.push(record), batch.version); }
        assert.equal(applied.length, count + 1);
        assert.equal(applied.at(-1)[1], sandbox.Ring.packMidiMessage(0x90, 67, 127));
    }
});

test('rejected current acknowledgements roll back overlap controls and stale same-file replies cannot install metadata', () => {
    const harness = instrumentHarness();
    const sandbox = harness;
    const identity = { session: 4n, build: (1n << 60n) + 5n };
    sandbox.state.node = { starplayerIdentity: identity, starplayerInteractiveVersion: 2 };
    sandbox.state.liveInputPending = true;
    sandbox.elements.overlapEnable = { checked: true };
    sandbox.showError = () => {};
    sandbox.updateModPanningAvailability = () => {};
    sandbox.state.pendingLoads = new Map();
    vm.createContext(sandbox);
    vm.runInContext(source('onWorkletMessage'), sandbox);
    const words = new Uint32Array([1, 0, Number(identity.session), 0, Number(identity.build & 0xffffffffn), Number(identity.build >> 32n), 3, 0]);
    sandbox.onWorkletMessage({ type: 'interactiveApplied', words, commands: new Uint32Array([22, 1, 0]), count: 1, accepted: false }, sandbox.state.node);
    assert.equal(sandbox.elements.overlapEnable.checked, false);
    assert.equal(sandbox.state.overlapEnabled, false);
    assert.equal(sandbox.state.letNotesRing[0], true);
    let resolved = false, rejected = false;
    const requested = { session: identity.session + 1n, build: identity.build + 1n };
    sandbox.state.pendingLoads.set(77, { node: sandbox.state.node, identity: requested, resolve() { resolved = true; }, reject() { rejected = true; } });
    sandbox.onWorkletMessage({ type: 'moduleLoaded', requestId: 77, generation: 1, accepted: true, identity, capabilities: 3 }, sandbox.state.node);
    assert.equal(resolved, false); assert.equal(rejected, true);
    assert.deepEqual(sandbox.state.node.starplayerIdentity, identity);
});


const deferred = () => { let resolve, reject; const promise = new Promise((done, fail) => { resolve = done; reject = fail; }); return { promise, resolve, reject }; };
const settle = async () => { for (let index = 0; index < 12; index++) await Promise.resolve(); };

function loadOptionsHarness() {
    const captures = [], activations = [], writes = [], errors = [];
    const state = { currentModuleBytes: new Uint8Array([1, 2]).buffer, node: {}, pendingLoads: new Map(), moduleRevision: 0, activeLoadOptions: { enhancementFlags: 0 }, metadata: { isMod: true, title: 'fixture' }, loadOptionsReloadInProgress: false };
    const playback = { controlRevision: 0, rebind() {} };
    const sandbox = { state, playback, elements: { keyboardEnable: { checked: false } }, loadOptionsFromControls: () => ({ enhancementFlags: 1 }), writeLoadOptionsToControls: value => writes.push(value), renderLoadOptionsSummary() {}, persistPreferences() {}, describeLoadOptionChange: () => ({}), updateModPanningAvailability() {}, clearError() {}, showMessage() {}, showError: error => errors.push(error), captureReplacement: () => { const capture = deferred(); captures.push(capture); return capture.promise; }, activateModuleOnNode: async (...args) => { activations.push(args); return { generation: 3 }; }, scheduleMediaPosition() {}, renderMetadata() {}, setControlsEnabled() {}, describeEnhancementReduction: () => '', setTimeout() {}, requestGarbageCollection() {} };
    vm.createContext(sandbox); vm.runInContext(`async ${source('applyLoadOptions')}`, sandbox);
    return { sandbox, state, playback, captures, activations, writes, errors };
}

test('load-option capture is guarded against a second toggle and rolls back on capture failure', async () => {
    const harness = loadOptionsHarness();
    const first = harness.sandbox.applyLoadOptions('enhancementFlags');
    assert.equal(harness.state.loadOptionsReloadInProgress, true);
    await harness.sandbox.applyLoadOptions('enhancementFlags');
    assert.equal(harness.captures.length, 1);
    harness.captures[0].reject(new Error('capture failed'));
    await first;
    assert.equal(harness.state.loadOptionsReloadInProgress, false);
    assert.equal(harness.writes.at(-1), harness.state.activeLoadOptions);
    assert.match(harness.errors[0], /capture failed/);
    assert.equal(harness.activations.length, 0);
});

test('scripted load-option changes during a fresh load preserve its posted options without capturing the old session', async () => {
    const harness = loadOptionsHarness();
    const postedOptions = { enhancementFlags: 4, headphoneFriendlyModPanning: false, speedAdjust: 2 };
    harness.state.pendingLoads.set(1, { node: {}, fresh: true, loadOptions: { enhancementFlags: 8 } });
    harness.state.pendingLoads.set(2, { node: harness.state.node, fresh: true, loadOptions: postedOptions });
    await harness.sandbox.applyLoadOptions('enhancementFlags');
    assert.equal(harness.state.moduleRevision, 0);
    assert.equal(harness.captures.length, 0); assert.equal(harness.activations.length, 0);
    assert.equal(harness.writes.at(-1), postedOptions);
    assert.deepEqual(harness.errors, []);
    harness.state.pendingLoads.delete(2);
    const allowed = harness.sandbox.applyLoadOptions('enhancementFlags');
    assert.equal(harness.captures.length, 1, 'an old-node request cannot block current loaded-module controls');
    harness.captures[0].resolve('current-cursor'); await allowed;
    assert.equal(harness.activations.length, 1);
});

test('Stop or Pause during capture recaptures the latest applied transport before same-node replacement', async () => {
    for (const opcode of [14, 16]) {
        const harness = loadOptionsHarness();
        const pending = harness.sandbox.applyLoadOptions('enhancementFlags');
        harness.playback.controlRevision += 1; harness.state.lastTransportOpcode = opcode;
        harness.captures[0].resolve('outdated-playing'); await settle();
        assert.equal(harness.activations.length, 0);
        assert.equal(harness.captures.length, 2);
        harness.captures[1].resolve(`latest-${opcode}`); await pending;
        assert.equal(harness.activations[0][3], `latest-${opcode}`);
    }
});

test('same-node capture retries after accepted settings change; transient MIDI and Preview do not change settings revision', async () => {
    const harness = loadOptionsHarness();
    const pending = harness.sandbox.applyLoadOptions('enhancementFlags');
    harness.state.sessionSettingsRevision = 1;
    harness.captures[0].resolve('outdated-settings'); await settle();
    assert.equal(harness.captures.length, 2); assert.equal(harness.activations.length, 0);
    harness.captures[1].resolve('current-settings'); await pending;
    assert.equal(harness.activations[0][3], 'current-settings');
    const sandbox = instrumentHarness();
    Object.assign(sandbox.state, { node: { starplayerIdentity: { session: 1n, build: 1n } }, commandRing: {}, fallbackCommands: [], sessionSettingsRevision: 0 });
    sandbox.Ring.pushCommand = () => true;
    vm.createContext(sandbox); vm.runInContext(source('queueCommand'), sandbox);
    for (const opcode of [sandbox.Ring.OPCODE_LIVE_MIDI, sandbox.Ring.OPCODE_PREVIEW, sandbox.Ring.OPCODE_STOP_PREVIEW]) sandbox.queueCommand(opcode, 0, 0);
    assert.equal(sandbox.state.sessionSettingsRevision, 0);
    sandbox.queueCommand(sandbox.Ring.OPCODE_NOTE_OVERLAP, 1, 0);
    assert.equal(sandbox.state.sessionSettingsRevision, 1);
});

test('pending load transport intents use the expected full identity and are remembered for the ready reply', () => {
    const sent = [], queued = [];
    const identity = { session: 2n, build: (1n << 61n) + 9n };
    const sandbox = { state: { node: { starplayerInteractiveVersion: 2, port: { postMessage: message => sent.push(message) } }, pendingLoads: new Map(), interactiveReady: false }, queueCommand: (...args) => queued.push(args), queueInteractive: (...args) => queued.push(args) };
    sandbox.state.pendingLoads.set(99, { node: sandbox.state.node, identity });
    vm.createContext(sandbox); vm.runInContext(ring, sandbox); sandbox.Ring = sandbox.StarPlayerRing;
    vm.runInContext(source('queueTransport'), sandbox);
    sandbox.queueTransport(sandbox.Ring.OPCODE_HOLD_STOP);
    assert.equal(queued.length, 0);
    assert.equal(sent[0].identity, identity);
    assert.equal(sent[0].requestId, 99);
    assert.equal(sandbox.state.pendingLoads.get(99).transportOpcode, sandbox.Ring.OPCODE_HOLD_STOP);
});


test('a missing interactive capability keeps ordinary fallback transport on legacy triples', () => {
    const sent = [];
    const sandbox = { state: { node: { starplayerInteractiveVersion: 0, port: { postMessage: message => sent.push(message) } }, commandRing: null, fallbackCommands: [{ opcode: 1, argument: 0, extra: 0, identity: { session: 1n, build: 2n } }] } };
    vm.createContext(sandbox); vm.runInContext(ring, sandbox); sandbox.Ring = sandbox.StarPlayerRing;
    vm.runInContext(source('flushFallbackCommands'), sandbox);
    sandbox.flushFallbackCommands();
    assert.equal(sent[0].version, 1);
    assert.deepEqual(Array.from(sent[0].commands), [1, 0, 0]);
    const applied = [];
    sandbox.Ring.drainFallbackCommands(sent[0].commands, (...record) => applied.push(record));
    assert.deepEqual(applied, [[1, 0, 0]]);
});


test('Reset effects prepares every removal with its exact previous slot state for rollback', () => {
    const previous = [{ wire: 1, params: { 0: 3 }, bypassed: false }, { wire: 2, params: { 1: 4 }, bypassed: true }];
    const sent = [];
    const sandbox = { state: { effectsSlotsBuilt: true, insertLayout: new Map([['255:0', previous[0]], ['3:2', previous[1]]]) }, elements: { effectsTarget: {} }, sendInsertMessage: (...args) => sent.push(args), renderEffectSlotRows() {} };
    vm.createContext(sandbox); vm.runInContext(ring, sandbox); sandbox.Ring = sandbox.StarPlayerRing;
    vm.runInContext(source('resetEffects'), sandbox);
    sandbox.resetEffects();
    assert.deepEqual(sent, [['remove', 255, 0, 0, previous[0]], ['remove', 3, 2, 0, previous[1]]]);
    assert.equal(sandbox.state.insertLayout.size, 0);
    assert.equal(sandbox.elements.effectsTarget.value, '255');
});

test('older same-node load acknowledgements never publish readiness over a newer request', () => {
    const identity = { session: 1n, build: 2n }, newer = { session: 2n, build: 3n };
    const node = { starplayerInteractiveVersion: 2, starplayerLatestLoadRequest: 11, starplayerIdentity: { session: 0n, build: 1n } };
    const resolved = [];
    const state = { node, interactiveReady: false, pendingLoads: new Map([
        [10, { node, identity, resolve: message => resolved.push(message.requestId), reject() { throw new Error('valid superseded reply'); } }],
        [11, { node, identity: newer, resolve: message => resolved.push(message.requestId), reject() {} }],
    ]), fallbackCommands: [] };
    const sandbox = { state, updateModPanningAvailability() {}, Ring: { sameIdentity: (left, right) => left?.session === right?.session && left?.build === right?.build } };
    vm.createContext(sandbox); vm.runInContext(source('onWorkletMessage'), sandbox);
    const acknowledge = (requestId, identity) => sandbox.onWorkletMessage({ type: 'moduleLoaded', requestId, identity, generation: requestId, capabilities: 3, accepted: true }, node);
    acknowledge(10, identity);
    assert.equal(state.interactiveReady, false); assert.equal(node.starplayerIdentity.build, 1n); assert.deepEqual(resolved, [10]);
    acknowledge(11, newer);
    assert.equal(state.interactiveReady, true); assert.deepEqual(node.starplayerIdentity, newer);
    // A delayed old reply after the new reply has also lost the authority to publish.
    state.pendingLoads.set(10, { node, identity, resolve() {}, reject() {} }); acknowledge(10, identity);
    assert.deepEqual(node.starplayerIdentity, newer);
});

test('a wholly stale rejected group cannot roll back current controls or clear current live input', () => {
    const sandbox = instrumentHarness();
    sandbox.state.node = { starplayerIdentity: { session: 4n, build: 8n }, starplayerInteractiveVersion: 2 };
    sandbox.state.liveInputPending = true; sandbox.state.pendingLoads = new Map();
    sandbox.elements.overlapEnable = { checked: true };
    sandbox.showError = () => { throw new Error('stale acknowledgement reached current UI'); };
    vm.createContext(sandbox); vm.runInContext(source('onWorkletMessage'), sandbox);
    sandbox.onWorkletMessage({ type: 'interactiveApplied', accepted: false, count: 1, words: new Uint32Array([1,0,4,0,7,0,3,0,0,0]), commands: new Uint32Array([22,1,0]) }, sandbox.state.node);
    assert.equal(sandbox.elements.overlapEnable.checked, true);
    assert.equal(sandbox.state.liveInputPending, true); assert.deepEqual(sandbox.records, []);
});


test('failed posted loads restore controls only with unchanged control and audio-applied identity proof', async () => {
    for (const proof of ['retained', 'changed', 'absent', 'not-ready', 'caps', 'metadata', 'superseded']) {
        const sandbox = instrumentHarness();
        const identity = { session: 4n, build: 8n };
        const sent = [];
        const node = { starplayerIdentity: identity, starplayerCapabilities: 3, starplayerInteractiveVersion: 2, starplayerSampleRate: 48000, port: { postMessage: message => sent.push(message) } };
        sandbox.elements.volume = { value: 100 };
        sandbox.elements.inputDestination.value = 'jam'; sandbox.elements.keyboardEnable.checked = false;
        Object.assign(sandbox.state, { node, metadata: { ...sandbox.state.metadata, identity }, pendingLoads: new Map(), sessionClock: 4n, buildClock: 8n, nextRequestId: 1, liveInputPending: false });
        Object.assign(sandbox, { playback: { effectiveAtEnd: () => false }, SONG_FADE_SECONDS: 3, loadOptionsFromControls: () => ({}), updateModPanningAvailability() {}, setControlsEnabled: enabled => { sandbox.enabled = enabled; } });
        vm.createContext(sandbox); vm.runInContext(source('pagePolicies') + source('restoreUnreadyInteractiveControls') + source('activateModuleOnNode') + source('onWorkletMessage'), sandbox);
        if (proof === 'not-ready') sandbox.state.interactiveReady = false;
        const pending = sandbox.activateModuleOnNode(node, new ArrayBuffer(0));
        const rejected = assert.rejects(pending, /bad audio bytes/);
        assert.equal(sent[0].hostSettings.atEnd, sandbox.Ring.AT_END_FADE_OUT);
        assert.equal(sent[0].hostSettings.fadeFrames, 144000, 'policy replay uses the candidate node rate');
        assert.equal(sandbox.state.interactiveReady, false);
        assert.equal(sandbox.elements.overlapEnable.disabled, true);
        assert.equal(sandbox.elements.instrumentList.children[0].querySelector('input').disabled, true);
        vm.runInContext(source('queueInteractive') + source('requestLiveInput'), sandbox);
        sandbox.elements.overlapEnable.checked = true;
        sandbox.queueInteractive(sandbox.Ring.OPCODE_NOTE_OVERLAP, 1);
        assert.equal(sandbox.elements.overlapEnable.checked, false, 'an unsent disabled control restores accepted state');
        sandbox.elements.inputDestination.value = 'selected'; sandbox.elements.keyboardEnable.checked = true;
        sandbox.requestLiveInput(true);
        assert.equal(sandbox.elements.inputDestination.value, 'jam'); assert.equal(sandbox.elements.keyboardEnable.checked, false);
        if (proof === 'metadata') sandbox.state.metadata.identity = { session: 9n, build: 9n };
        if (proof === 'superseded') { node.starplayerLatestLoadRequest = 99; sandbox.state.interactiveReady = true; }
        const retainedIdentity = proof === 'changed' ? { session: 9n, build: 9n } : proof === 'absent' ? undefined : identity;
        sandbox.onWorkletMessage({ type: 'moduleError', requestId: sent[0].requestId, reason: 'bad audio bytes', retainedIdentity, retainedCapabilities: proof === 'caps' ? 1 : 3 }, node);
        if (proof === 'retained') {
            assert.equal(sandbox.state.interactiveReady, false, 'retention proof alone cannot enable input before policy reconciliation');
            assert.equal(sent.at(-1).type, 'reconcilePolicies');
            sandbox.onWorkletMessage({ type: 'moduleLoaded', requestId: sent[0].requestId, identity, capabilities: 3, accepted: true, policyRevision: 0 }, node);
        }
        await rejected;
        assert.equal(sandbox.state.interactiveReady, proof === 'retained' || proof === 'superseded');
        assert.equal(sandbox.enabled, proof === 'superseded' ? undefined : proof === 'retained');
        assert.equal(sandbox.state.letNotesRing[0], true, 'accepted instrument settings survive failure');
    }
});


test('the current negotiated worklet reports its actual live scheduling lead', () => {
    const node = {};
    const sandbox = { state: { node, effects: [], enhancements: [] }, elements: { quantum: {} }, updateOutputPanel() {}, Ring: { INTERACTIVE_WIRE_VERSION: 2 } };
    vm.createContext(sandbox); vm.runInContext(source('onWorkletMessage'), sandbox);
    const ready = { type: 'ready', interactiveVersion: 2, commandRingVersion: 2, sampleRate: 48000, renderQuantum: 128, leadFrames: 256, leadMillis: 256000 / 48000 };
    sandbox.onWorkletMessage(ready, node);
    assert.equal(sandbox.state.eventLeadFrames, 256); assert.equal(sandbox.state.eventLeadMillis, ready.leadMillis);
    sandbox.onWorkletMessage({ ...ready, leadFrames: 512 }, {});
    assert.equal(sandbox.state.eventLeadFrames, 256, 'a retired graph cannot overwrite current lead');
    const candidate = {};
    sandbox.onWorkletMessage({ ...ready, sampleRate: 22050, leadMillis: 256000 / 22050 }, candidate);
    Object.assign(sandbox.state, { pendingInserts: new Map() });
    Object.assign(sandbox, { releaseHeldKeys() {}, updateLiveInputPanel() {} });
    sandbox.elements.keyboardEnable = { checked: false };
    vm.runInContext(source('installGraph'), sandbox);
    sandbox.installGraph({ node: candidate, context: { sampleRate: 22050 }, channels: 2 });
    assert.equal(sandbox.state.eventLeadFrames, 256);
    assert.equal(sandbox.state.eventLeadMillis, 256000 / 22050, 'candidate readiness supplies the installed graph rate conversion');

});

test('a setting accepted while a candidate is prepared keeps the old graph and its current controls', async () => {
    for (const operation of ['rebuildAudioContext', 'rebuildOutputChannels']) for (const change of [false, true]) {
        const sandbox = instrumentHarness();
        const identity = { session: 4n, build: 8n };
        let oldClosed = false, candidateClosed = false, discarded = false;
        const context = { sampleRate: 48000, destination: {}, close: async () => { oldClosed = true; } };
        const candidateContext = { sampleRate: 22050, destination: {}, resume: async () => {}, close: async () => { candidateClosed = true; }, addEventListener() {} };
        const oldNode = { starplayerIdentity: identity, disconnect() {} };
        const candidate = { connect() {}, disconnect() { discarded = true; } };
        const prepared = deferred(); const replacements = [];
        Object.assign(sandbox.state, { node: oldNode, context, currentModuleBytes: new Uint8Array([1]).buffer, moduleRevision: 1, sessionSettingsRevision: 0, commandRing: {}, fallbackCommands: [], letNotesRing: [false], activeLoadOptions: {} });
        sandbox.state.metadata.identity = identity;
        Object.assign(sandbox.elements, { sampleRate: { value: '22050' }, outputChannels: { value: '2' }, overlapEnable: {} });
        Object.assign(sandbox, {
            AudioContext: function () { return candidateContext; }, playback: { controlRevision: 0, rebind() {} },
            startAudio: async () => {}, formatRate: String, captureReplacement: async () => ({ letNotesRing: [...sandbox.state.letNotesRing] }),
            mixerModeFromControls: () => 0x202, prepareNode: () => prepared.promise, loadOptionsFromControls: () => ({}),
            activateModuleOnNode: async (_node, _bytes, _options, replacement) => { replacements.push(replacement); return { generation: 1, identity }; },
            installGraph: graph => { sandbox.state.node = graph.node; }, scheduleMediaPosition() {}, persistPreferences() {}, refreshOutputDevices: async () => {}, updateOutputPanel() {}, showMessage() {},
            showError: message => assert.fail(message),
        });
        sandbox.Ring.pushCommand = () => true;
        vm.createContext(sandbox);
        vm.runInContext(source('restoreUnreadyInteractiveControls') + source('queueCommand') + source('queueInteractive') + source('onWorkletMessage') + `async ${source(operation)}`, sandbox);
        const pending = sandbox[operation](); await settle();
        if (change) {
            sandbox.queueInteractive(sandbox.Ring.OPCODE_LET_NOTES_RING, 0, 1);
            sandbox.onWorkletMessage({ type: 'interactiveApplied', accepted: true, count: 1, commands: new Uint32Array([23, 0, 1]), words: new Uint32Array([1, 0, 4, 0, 8, 0, 3, 1]) }, oldNode);
            assert.equal(sandbox.state.letNotesRing[0], true);
        }
        prepared.resolve({ node: candidate, context: candidateContext, channels: 2 }); await pending;
        assert.equal(replacements[0].letNotesRing[0], false, 'candidate preparation uses the earlier capture');
        assert.equal(sandbox.state.node, change ? oldNode : candidate);
        assert.equal(discarded, change);
        assert.equal(oldClosed, operation === 'rebuildAudioContext' && !change);
        assert.equal(candidateClosed, operation === 'rebuildAudioContext' && change);
        assert.equal(sandbox.state.letNotesRing[0], change);
    }
});


test('known music-frame seeks clamp 100 percent and fade margin while unknown length and row/order requests keep their values', () => {
    for (const shared of [false, true]) {
        const queued = [], positioned = [];
        const snapshot = { songFlags: 1, songLengthFrames: 1000, songFrame: 27 };
        const sandbox = { state: { latest: snapshot, telemetry: shared ? {} : null }, Ring: { OPCODE_SEEK_FRAME: 5, OPCODE_SEEK_ORDER: 3, OPCODE_SEEK_ROW: 4, SONG_FLAG_LENGTH_KNOWN: 1, readTelemetry: () => snapshot }, playlistPlayback: { cancel() {} }, playback: { userControl() {} }, queueCommand: (...record) => queued.push(record), refreshMediaPosition: frame => positioned.push(frame), scheduleMediaPosition() {} };
        vm.createContext(sandbox); vm.runInContext(source('userSeek'), sandbox);
        for (const [opcode, requested, expected] of [[5, 1000, 999], [5, 5000, 999], [5, 40, 40], [3, 1000, 1000], [4, 5000, 5000]]) {
            assert.equal(sandbox.userSeek(opcode, requested), expected);
            assert.deepEqual(queued.at(-1), [opcode, expected]);
            assert.equal(positioned.at(-1), opcode === 5 ? expected : 27);
        }
        snapshot.songFlags = 0;
        assert.equal(sandbox.userSeek(5, 5000), 5000, 'unknown-length source keeps native seek semantics');
    }
});


test('bulk overlap covers offscreen eligible rows and retains FIFO intent through both bounded bridges', () => {
    for (const shared of [true, false]) {
        const harness = instrumentHarness(Array.from({ length: 150 }, (_, index) => index % 7 === 0 ? 4 : 2));
        const { state, elements, Ring } = harness;
        const identity = { session: 4n, build: 8n }, batches = [];
        Object.assign(harness, { SharedArrayBuffer, Int32Array, Atomics });
        Object.assign(state, { node: { starplayerIdentity: identity, starplayerInteractiveVersion: 2, port: { postMessage: message => batches.push(message) } }, fallbackCommands: [], sessionSettingsRevision: 0, letNotesRing: [] });
        state.commandRing = shared ? Ring.createCommandRing(2) : null;
        vm.createContext(harness);
        vm.runInContext(['queueCommand', 'queueInteractive', 'flushFallbackCommands', 'onWorkletMessage'].map(source).join('\n'), harness);
        elements.instrumentSelectAll.checked = true; harness.setAllInstrumentOverlap();
        elements.instrumentSelectAll.checked = false; harness.setAllInstrumentOverlap();
        const override = state.instrumentRows[149].checkbox;
        override.checked = true; override.fire('change');
        const eligible = state.metadata.instruments.flatMap((instrument, index) => instrument.capability & 2 ? [index] : []);
        assert.equal(state.sessionSettingsRevision, eligible.length * 2 + 1);
        const applied = [];
        const acknowledge = commands => {
            applied.push(...commands);
            harness.onWorkletMessage({ type: 'interactiveApplied', accepted: true, count: commands.length, words: new Uint32Array([1,0,4,0,8,0,3,1]), commands: new Uint32Array(commands.flat()) }, state.node);
        };
        for (let iteration = 0; iteration < 40; iteration++) {
            harness.flushFallbackCommands();
            if (shared) {
                const records = [];
                Ring.drainCommands(state.commandRing, (opcode, argument, extra) => records.push([opcode, argument, extra]));
                if (records.length) acknowledge(records);
            } else {
                for (const batch of batches.splice(0)) {
                    if (batch.type !== 'commandBatch') continue;
                    assert.ok(batch.commands.length <= 16);
                    acknowledge(batch.commands.map(({ opcode, argument, extra }) => [opcode, argument, extra]));
                }
            }
        }
        assert.equal(state.fallbackCommands.length, 0);
        assert.deepEqual(applied.slice(0, eligible.length).map(record => record[1]), eligible);
        assert.ok(applied.slice(0, eligible.length).every(record => record[2] === 1));
        assert.ok(applied.slice(eligible.length, eligible.length * 2).every(record => record[2] === 0));
        assert.equal(applied.at(-1)[1], 149);
        assert.equal(state.letNotesRing[149], true);
        assert.ok(eligible.filter(index => index !== 149).every(index => state.letNotesRing[index] === false));
        assert.equal(elements.instrumentSelectAll.checked, false);
        assert.equal(elements.instrumentSelectAll.indeterminate, true);
        assert.equal(state.overlapEnabled, false, 'bulk edits do not enable global overlap');
    }
});

test('bulk header tracks none/some/all and partial rejection without replacing a held Play node', () => {
    const harness = instrumentHarness(Array(130).fill(11));
    const { state, elements, Ring } = harness;
    state.node = { starplayerIdentity: { session: 4n, build: 8n }, starplayerInteractiveVersion: 2, port: { postMessage() {} } };
    harness.showError = () => {};
    vm.createContext(harness); vm.runInContext(source('onWorkletMessage'), harness);
    const preview = state.instrumentRows[0].preview;
    preview.fire('pointerdown');
    state.letNotesRing = []; harness.updateInstrumentControls();
    assert.equal(elements.instrumentSelectAll.checked, false);
    assert.equal(elements.instrumentSelectAll.indeterminate, false);
    const acknowledge = (commands, accepted = true, build = 8) => harness.onWorkletMessage({ type: 'interactiveApplied', accepted, count: commands.length, words: new Uint32Array([1,0,4,0,build,0,3,1]), commands: new Uint32Array(commands.flat()) }, state.node);
    acknowledge(Array.from({ length: 63 }, (_, index) => [Ring.OPCODE_LET_NOTES_RING, index, 1]));
    assert.equal(state.previewInstrument, 0);
    assert.equal(state.instrumentRows[0].preview, preview);
    assert.equal(elements.instrumentSelectAll.indeterminate, true);
    acknowledge([[Ring.OPCODE_LET_NOTES_RING, 63, 1]], false);
    assert.equal(state.letNotesRing[0], true);
    assert.equal(state.instrumentRows[63].checkbox.checked, false);
    assert.equal(elements.instrumentSelectAll.indeterminate, true);
    acknowledge(Array.from({ length: 63 }, (_, index) => [Ring.OPCODE_LET_NOTES_RING, index + 63, 1]));
    acknowledge(Array.from({ length: 4 }, (_, index) => [Ring.OPCODE_LET_NOTES_RING, index + 126, 1]));
    assert.equal(elements.instrumentSelectAll.checked, true);
    assert.equal(elements.instrumentSelectAll.indeterminate, false);
    elements.instrumentSelectAll.checked = false; harness.setAllInstrumentOverlap();
    assert.equal(harness.records.slice(-130).length, 130);
    const count = harness.records.length;
    acknowledge([[Ring.OPCODE_LET_NOTES_RING, 0, 0]], true, 7);
    assert.equal(elements.instrumentSelectAll.checked, false, 'stale acknowledgement cannot change native pending choice');
    assert.equal(state.letNotesRing[0], true);
    state.interactiveReady = false; elements.instrumentSelectAll.checked = true;
    harness.restoreUnreadyInteractiveControls = () => harness.updateInstrumentControls(); harness.setAllInstrumentOverlap();
    assert.equal(harness.records.length, count);
    assert.equal(elements.instrumentSelectAll.disabled, true);
    state.metadata = { ...state.metadata, instruments: [{ capability: 4, length: 3 }] }; state.interactiveReady = true; harness.renderMetadata();
    assert.equal(elements.instrumentSelectAll.disabled, true);
    assert.equal(elements.instrumentSelectAll.checked, false);
});


test('a new applied session discards pending bulk remainder and ignores late replies for the old module', () => {
    const harness = instrumentHarness(Array(96).fill(2));
    const { state, elements, Ring } = harness;
    const previous = { session: 4n, build: 8n }, current = { session: 5n, build: 9n };
    state.node = { starplayerIdentity: previous, starplayerInteractiveVersion: 2, starplayerLatestLoadRequest: 17, port: { postMessage() {} } };
    Object.assign(state, { fallbackCommands: [], commandRing: null, pendingLoads: new Map(), sessionSettingsRevision: 0 });
    vm.createContext(harness); vm.runInContext(['queueCommand', 'queueInteractive', 'flushFallbackCommands', 'onWorkletMessage'].map(source).join('\n'), harness);
    elements.instrumentSelectAll.checked = true; harness.setAllInstrumentOverlap();
    assert.equal(state.fallbackCommands.length, 80, 'one bridge batch leaves its remainder pending');
    state.pendingLoads.set(17, { node: state.node, identity: current, resolve() {}, reject() { assert.fail('valid load'); } });
    harness.updateModPanningAvailability = () => {};
    harness.onWorkletMessage({ type: 'moduleLoaded', requestId: 17, identity: current, capabilities: 3, accepted: true }, state.node);
    assert.equal(state.fallbackCommands.length, 0, 'pending old-session settings cannot reach the new module');
    state.metadata = { ...state.metadata, instruments: [{ capability: 2, length: 5 }] }; state.letNotesRing = [false]; harness.renderMetadata();
    harness.onWorkletMessage({ type: 'interactiveApplied', accepted: true, count: 1, words: new Uint32Array([1,0,4,0,8,0,3,1]), commands: new Uint32Array([Ring.OPCODE_LET_NOTES_RING, 0, 1]) }, state.node);
    assert.equal(state.letNotesRing[0], false);
    assert.equal(elements.instrumentSelectAll.checked, false);
    assert.equal(elements.instrumentSelectAll.indeterminate, false);
    assert.equal(state.instrumentRows[0].checkbox.checked, false);
});


test('fresh load cancels unsent old-session instrument edits while replacement retains them and accepted state survives failure', async () => {
    for (const replacement of [null, new BigUint64Array([1n, 4n])]) {
        const harness = instrumentHarness();
        const { state, elements, Ring } = harness;
        const previous = { session: 4n, build: 8n }, other = { session: 3n, build: 7n };
        const sent = [];
        state.node = { starplayerIdentity: previous, starplayerInteractiveVersion: 2, starplayerCapabilities: 3, port: { postMessage: message => sent.push(message) } };
        Object.assign(state, { pendingLoads: new Map(), sessionClock: 4n, buildClock: 8n, nextRequestId: 1 }); state.metadata.identity = previous;
        state.fallbackCommands = [
            { opcode: Ring.OPCODE_LET_NOTES_RING, argument: 0, extra: 0, identity: previous },
            { opcode: Ring.OPCODE_LET_NOTES_RING, argument: 1, extra: 1, identity: other },
            { opcode: Ring.OPCODE_MASTER_VOLUME, argument: 123, extra: 0, identity: previous },
        ];
        elements.volume = { value: 100 };
        Object.assign(harness, { playback: { effectiveAtEnd: () => false }, SONG_FADE_SECONDS: 3, updateModPanningAvailability() {}, setControlsEnabled() {}, loadOptionsFromControls: () => ({}), showError() {} });
        vm.createContext(harness); vm.runInContext(['pagePolicies', 'restoreUnreadyInteractiveControls', 'activateModuleOnNode', 'onWorkletMessage'].map(source).join('\n'), harness);
        const pending = harness.activateModuleOnNode(state.node, new ArrayBuffer(0), {}, replacement);
        const rejected = assert.rejects(pending, /expected load failure/);
        assert.equal(state.fallbackCommands.length, replacement ? 3 : 2);
        assert.equal(state.fallbackCommands.some(record => record.opcode === Ring.OPCODE_LET_NOTES_RING && Ring.sameIdentity(record.identity, previous)), !!replacement);
        assert.equal(state.fallbackCommands.some(record => record.opcode === Ring.OPCODE_MASTER_VOLUME), true);
        harness.onWorkletMessage({ type: 'moduleError', requestId: sent[0].requestId, retainedIdentity: previous, retainedCapabilities: 3, reason: 'expected load failure' }, state.node);
        assert.equal(state.interactiveReady, false);
        assert.equal(sent.at(-1).type, 'reconcilePolicies');
        harness.onWorkletMessage({ type: 'moduleLoaded', requestId: sent[0].requestId, identity: previous, capabilities: 3, accepted: true, policyRevision: 0 }, state.node);
        await rejected;
        assert.equal(state.interactiveReady, true);
        assert.equal(state.letNotesRing[0], true, 'accepted selections survive cancelled pending edits and failed load');
        assert.equal(state.instrumentRows[0].checkbox.checked, true);
    }
});


test('selected reference uses committed key metadata and captures native loop release for a held press', () => {
    const harness = instrumentHarness([11, 31]);
    const { state, rows, records, Ring } = harness;
    const first = state.metadata.instruments[0], second = state.metadata.instruments[1];
    first.noteCapabilities = Array(128).fill(0); first.noteCapabilities[61] = 1;
    second.noteCapabilities = Array(128).fill(0); second.noteCapabilities[61] = 3; second.noteCapabilities[62] = 1;
    harness.updateInstrumentControls();
    assert.equal(rows[0].querySelector('button').disabled, true, '60 unmapped despite aggregate legacy capability');
    state.referenceNote = 61; harness.updateInstrumentControls();
    const shot = rows[0].querySelector('button'), loop = rows[1].querySelector('button');
    assert.equal(shot.disabled, false); shot.fire('pointerdown'); shot.fire('pointerup');
    assert.deepEqual(records, [[Ring.OPCODE_PREVIEW_NOTE, 0, 61]]);
    loop.fire('pointerdown'); state.referenceNote = 62; harness.updateInstrumentControls(); loop.fire('pointerup');
    assert.deepEqual(records.slice(1), [[Ring.OPCODE_PREVIEW_NOTE, 1, 61], [Ring.OPCODE_RELEASE_PREVIEW]], 'button-up releases the looping note captured at press');
    assert.equal(state.referenceNote, 62); assert.equal(state.instrumentRows[1].preview, loop, 'chooser change retains control nodes');
    loop.fire('pointerdown'); state.referenceNote = 61; harness.updateInstrumentControls(); loop.fire('pointerup');
    assert.deepEqual(records.slice(3), [[Ring.OPCODE_PREVIEW_NOTE, 1, 62]], 'a one-shot does not become looping while held');
    loop.fire('pointerdown'); state.referenceNote = 127; harness.updateInstrumentControls();
    assert.equal(loop.disabled, false, 'held capture remains until release'); loop.fire('pointerup');
    assert.equal(loop.disabled, true, 'the next unavailable key disables preview after release');
    assert.deepEqual(records.slice(4), [[Ring.OPCODE_PREVIEW_NOTE, 1, 61], [Ring.OPCODE_RELEASE_PREVIEW]]);
});

test('inspection key flags are copied before the inspection module changes', () => {
    let keys = new Uint8Array(128); keys[61] = 3;
    const Loader = { module_instrument_count: () => 1, instrument_name: () => 'mapped', instrument_sample_length: () => 32, instrument_capability: () => 7, instrument_note_capabilities: () => keys, module_title: () => 'retained', module_channel_count: () => 1, module_order_count: () => 1, module_pattern_count: () => 1, module_is_mod: () => false };
    const sandbox = { Loader }; vm.createContext(sandbox); vm.runInContext(source('readMetadata'), sandbox);
    const metadata = sandbox.readMetadata('retained.xm');
    keys.fill(0); keys = new Uint8Array(128); keys[60] = 1;
    assert.equal(metadata.instruments[0].noteCapabilities[61], 3); assert.equal(metadata.instruments[0].noteCapabilities[60], 0);
});
