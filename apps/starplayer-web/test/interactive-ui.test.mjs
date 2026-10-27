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
    fire(name, event = {}) { this.listeners[name]?.({ button: 0, preventDefault() {}, ...event }); }
    querySelectorAll() { return []; }
}
function instrumentHarness() {
    const records = [];
    const elements = Object.fromEntries(['title', 'moduleDetail', 'seekOrder', 'instrumentCount', 'instrumentList', 'keyboardEnable', 'inputDestination'].map(key => [key, new Element()]));
    const state = { metadata: { title: 'native IT', label: 'fixture.it', channels: 64, orders: 1, patterns: 1, instruments: [11, 31, 29, 3, 0, 2, 6].map((capability, index) => ({ capability, name: `instrument${index}` })) }, selectedInstrument: null, interactiveReady: true, letNotesRing: [true], overlapEnabled: false, keysDown: new Map(), context: null };
    const sandbox = { state, elements, document: { createElement: tag => new Element(tag) }, backgroundOutput: { start() {} }, sizeInstrumentColumn() {}, ensureChannelRows() {}, queueInteractive: (...record) => records.push(record), flushFallbackCommands() {}, releaseHeldKeys() {}, syncLiveInput() {}, updateLiveInputPanel() {} };
    vm.createContext(sandbox); vm.runInContext(ring, sandbox); sandbox.Ring = sandbox.StarPlayerRing;
    vm.runInContext(['renderMetadata', 'selectInstrument', 'cleanupLiveInput'].map(source).join('\n'), sandbox);
    sandbox.renderMetadata();
    return { ...sandbox, records, rows: elements.instrumentList.children };
}

test('instrument-only checkboxes remain usable in bypass, mixed maps stay eligible, reference preview is independent', () => {
    const { rows } = instrumentHarness();
    assert.equal(rows[0].children[2].children[0].checked, true);
    assert.equal(rows[0].children[2].children[0].disabled, false);
    assert.equal(rows[1].children[2].children[0].disabled, false);
    assert.match(rows[1].children[3].attributes['aria-label'], /Mixed map/);
    assert.equal(rows[2].children[2].children[0].disabled, true);
    assert.equal(rows[3].children[0].disabled, false, 'another key is playable');
    assert.equal(rows[3].children[1].disabled, true, 'reference60 has no viable sample');
    assert.equal(rows[4].children[0].disabled, true);
    assert.equal(rows[5].children[0].disabled, true, 'native low-map instrument has no live-tunable key');
    assert.equal(rows[5].children[2].children[0].disabled, false, 'song one-shot PCM remains eligible');
    assert.equal(rows[5].children[3].textContent, 'One-shot samples');
    assert.match(rows[6].children[3].attributes['aria-label'], /Mixed map/);
    assert.equal(rows[6].children[2].children[0].disabled, false);
    assert.match(rows[0].children[2].title, /Unchecked preserves native behavior, including IT NNA/);
});

test('one-shot Preview-up finishes; looping Preview-up releases; cancel and focus loss clear live scope', () => {
    const { rows, records, Ring } = instrumentHarness();
    const shot = rows[0].children[1], loop = rows[2].children[1];
    shot.fire('pointerdown'); shot.fire('pointerup'); shot.fire('lostpointercapture');
    assert.deepEqual(records, [[Ring.OPCODE_PREVIEW, 0]]);
    loop.fire('pointerdown'); loop.fire('pointerup'); loop.fire('lostpointercapture');
    assert.deepEqual(records.slice(1), [[Ring.OPCODE_PREVIEW, 2], [Ring.OPCODE_RELEASE_PREVIEW]]);
    loop.fire('keydown', { key: ' ', repeat: false }); loop.fire('keyup', { key: ' ' }); loop.fire('click', { detail: 0 });
    assert.deepEqual(records.slice(3), [[Ring.OPCODE_PREVIEW, 2], [Ring.OPCODE_RELEASE_PREVIEW]], 'keyboard activation never duplicates the click');
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
    const state = { currentModuleBytes: new Uint8Array([1, 2]).buffer, node: {}, moduleRevision: 0, activeLoadOptions: { enhancementFlags: 0 }, metadata: { isMod: true, title: 'fixture' }, loadOptionsReloadInProgress: false };
    const playback = { controlRevision: 0, rebind() {} };
    const sandbox = { state, playback, elements: { keyboardEnable: { checked: false } }, loadOptionsFromControls: () => ({ enhancementFlags: 1 }), writeLoadOptionsToControls: value => writes.push(value), renderLoadOptionsSummary() {}, persistPreferences() {}, describeLoadOptionChange: () => ({}), updateModPanningAvailability() {}, clearError() {}, showMessage() {}, showError: error => errors.push(error), captureReplacement: () => { const capture = deferred(); captures.push(capture); return capture.promise; }, activateModuleOnNode: async (...args) => { activations.push(args); return { generation: 3 }; }, scheduleMediaPosition() {}, renderMetadata() {}, describeEnhancementReduction: () => '', setTimeout() {}, requestGarbageCollection() {} };
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
