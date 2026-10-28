import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { readFile } from 'node:fs/promises';

const app = await readFile(new URL('../www/app.js', import.meta.url), 'utf8');
const ring = await readFile(new URL('../www/ring.js', import.meta.url), 'utf8');
const worklet = await readFile(new URL('../www/worklet-processor.js', import.meta.url), 'utf8');
function source(name) {
    const start = app.indexOf(`function ${name}(`);
    return app.slice(start, app.indexOf('\n}\n', start) + 3);
}
function pageHarness() {
    const commands = [], messages = [], errors = [];
    const identity = { session: 7n, build: (1n << 55n) + 1n };
    const node = { starplayerIdentity: identity, starplayerInteractiveVersion: 2, starplayerCapabilities: 3, port: { postMessage: message => messages.push(structuredClone(message)) } };
    const state = { node, overlapEnabled: false, uniqueNotesAcrossChannels: false, policyRevision: 0, policyIntentRevisions: {}, policyEdits: [], appliedPolicies: { identity, overlapEnabled: false, uniqueNotesAcrossChannels: false }, sessionSettingsRevision: 0, interactiveReady: true, letNotesRing: [true], pendingLoads: new Map(), pendingStates: new Map(), keysDown: new Map(), fallbackCommands: [], metadata: { identity } };
    const elements = { overlapEnable: { checked: false }, uniqueNotes: { checked: false } };
    const sandbox = { state, elements, showError: error => errors.push(error), queueInteractive: (...command) => commands.push(command), queueCommand: (...command) => commands.push(command), flushFallbackCommands() {}, renderMetadata() {}, setControlsEnabled() {}, updateInstrumentControls() {}, updateLiveInputPanel() {}, updateModPanningAvailability() {} };
    vm.createContext(sandbox); vm.runInContext(ring, sandbox);
    sandbox.Ring = { ...sandbox.StarPlayerRing, OPCODE_UNIQUE_NOTES: 26 };
    vm.runInContext(['pagePolicies', 'publishPendingPolicies', 'setPagePolicy', 'queueTransport', 'onWorkletMessage'].map(source).join('\n'), sandbox);
    function acknowledge(opcode, value, accepted = true, requestedIdentity = identity) {
        const words = new Uint32Array([1, 0, Number(requestedIdentity.session & 0xffffffffn), Number(requestedIdentity.session >> 32n), Number(requestedIdentity.build & 0xffffffffn), Number(requestedIdentity.build >> 32n), 3, Number(accepted), 0, 0]);
        sandbox.onWorkletMessage({ type: 'interactiveApplied', words, commands: [opcode, Number(value), 0], count: 1, accepted }, node);
    }
    return { ...sandbox, commands, messages, errors, identity, acknowledge };
}

test('page desired policies survive older same-identity acknowledgements; latest direct rejection rolls back per policy', () => {
    const page = pageHarness(), { state, Ring } = page;
    page.setPagePolicy(Ring.OPCODE_NOTE_OVERLAP, true);
    page.setPagePolicy(Ring.OPCODE_NOTE_OVERLAP, false);
    page.acknowledge(Ring.OPCODE_NOTE_OVERLAP, true);
    assert.equal(state.appliedPolicies.overlapEnabled, true);
    assert.equal(state.overlapEnabled, false, 'older accepted edit cannot overwrite newer desire');
    assert.equal(page.elements.overlapEnable.checked, false);
    page.setPagePolicy(Ring.OPCODE_UNIQUE_NOTES, true);
    page.acknowledge(Ring.OPCODE_NOTE_OVERLAP, false, false);
    assert.equal(state.overlapEnabled, true, 'latest overlap rejection restores applied overlap even after a uniqueness edit');
    assert.equal(state.uniqueNotesAcrossChannels, true);
    page.acknowledge(Ring.OPCODE_UNIQUE_NOTES, true, false);
    assert.equal(state.uniqueNotesAcrossChannels, false);
    assert.equal(state.overlapEnabled, true);
    assert.equal(state.letNotesRing[0], true);
});

test('stale identities and older rejected edits cannot erase page preferences', () => {
    const page = pageHarness(), { state, Ring } = page;
    page.setPagePolicy(Ring.OPCODE_NOTE_OVERLAP, true);
    page.setPagePolicy(Ring.OPCODE_NOTE_OVERLAP, false);
    page.acknowledge(Ring.OPCODE_NOTE_OVERLAP, true, false);
    assert.equal(state.overlapEnabled, false);
    const revision = state.policyRevision;
    page.acknowledge(Ring.OPCODE_NOTE_OVERLAP, false, true, { session: 6n, build: 1n });
    assert.equal(state.policyRevision, revision);
    assert.equal(state.policyEdits.length, 1, 'stale rejection cannot consume a current edit');
});

test('pending current and candidate installations receive latest page revisions and newest transport intent', () => {
    const page = pageHarness(), { state, Ring } = page;
    const candidateMessages = [];
    const candidate = { port: { postMessage: message => candidateMessages.push(message) } };
    state.pendingLoads.set(10, { node: state.node, identity: { session: 8n, build: 2n } });
    state.pendingLoads.set(11, { node: candidate, identity: { session: 7n, build: 3n } });
    state.interactiveReady = false;
    page.setPagePolicy(Ring.OPCODE_NOTE_OVERLAP, true);
    page.setPagePolicy(Ring.OPCODE_UNIQUE_NOTES, true);
    assert.equal(page.commands.length, 0, 'no direct controls target the old installed identity');
    assert.equal(page.messages.at(-1).policies.policyRevision, 2);
    assert.equal(candidateMessages.at(-1).policies.uniqueNotesAcrossChannels, true);
    page.queueTransport(Ring.OPCODE_RESUME); page.queueTransport(Ring.OPCODE_HOLD_STOP);
    assert.equal(page.messages.at(-1).opcode, Ring.OPCODE_HOLD_STOP);
    assert.equal(candidateMessages.at(-1).opcode, Ring.OPCODE_HOLD_STOP);
    assert.equal(state.pendingLoads.get(11).transportOpcode, Ring.OPCODE_HOLD_STOP);
    page.onWorkletMessage({ type: 'loadReplayReady', requestId: 11, identity: { session: 7n, build: 2n }, accepted: true }, candidate);
    assert.equal(candidateMessages.at(-1).type, 'loadTransport', 'stale ready cannot confirm a new build');
    page.onWorkletMessage({ type: 'loadReplayReady', requestId: 11, identity: { session: 7n, build: 3n }, accepted: true }, candidate);
    assert.equal(candidateMessages.at(-1).type, 'completeLoad');
    assert.equal(candidateMessages.at(-1).policies.policyRevision, 2);
});

test('failed posted loads reconcile proven retained identity before enabling interaction', () => {
    const page = pageHarness(), { state, identity } = page;
    let rejected = '';
    const next = { session: 8n, build: identity.build + 1n };
    state.node.starplayerLatestLoadRequest = 40;
    state.pendingLoads.set(40, { node: state.node, identity: next, previousIdentity: identity, previousReady: true, reject: error => { rejected = error.message; }, resolve() { assert.fail('failed load must reject its caller'); } });
    state.interactiveReady = false;
    state.overlapEnabled = true; state.policyRevision = 3;
    page.onWorkletMessage({ type: 'moduleError', requestId: 40, retainedIdentity: identity, retainedCapabilities: 3, reason: 'bad bytes' }, state.node);
    assert.equal(state.interactiveReady, false);
    assert.equal(state.overlapEnabled, true);
    assert.equal(page.messages.at(-1).type, 'reconcilePolicies');
    assert.equal(page.messages.at(-1).policies.overlapEnabled, true);
    assert.equal(rejected, '');
    page.onWorkletMessage({ type: 'moduleLoaded', requestId: 40, identity, accepted: true, capabilities: 3, policyRevision: 3, overlapEnabled: true, uniqueNotesAcrossChannels: false }, state.node);
    assert.equal(state.interactiveReady, true);
    assert.equal(rejected, 'bad bytes');
    assert.equal(state.appliedPolicies.overlapEnabled, true);
});

function replayHarness() {
    let Processor;
    const sandbox = { AudioWorkletProcessor: class {}, registerProcessor: (_name, value) => { Processor = value; } };
    vm.createContext(sandbox); vm.runInContext(ring, sandbox);
    const Ring = { ...sandbox.StarPlayerRing, OPCODE_UNIQUE_NOTES: 26 };
    sandbox.StarPlayerRing = Ring; vm.runInContext(worklet, sandbox);
    const processor = Object.create(Processor.prototype);
    const identity = { session: 2n, build: 3n }, queued = [], executed = [], messages = [];
    const status = new Uint32Array(10); let quanta = 0, rejected = 0, playing = false, releaseEpoch = 0;
    const policy = { overlap: false, unique: false };
    processor.interactiveVersion = 2; processor.interactiveStatus = status;
    processor.pendingAcks = []; processor.nextBarrierLow = 1; processor.nextBarrierHigh = 0;
    processor.pendingControlSerial = 0; processor.controlAck = { count: 0 }; processor.controlFailed = false;
    processor.fallbackBatches = []; processor.deferredLoads = new Map(); processor.pendingReplay = null;
    processor.port = { postMessage: message => messages.push(structuredClone(message)) };
    processor.bindViews = () => { processor.stableMemoryBytes = 100; };
    processor.wasm = {
        current_session: () => identity.session, current_build: () => identity.build,
        dropped_commands: () => rejected, collect_garbage() {}, quanta_rendered: () => quanta,
        pending_control_commands: () => queued.length,
        enqueue_interactive_words(opcode, argument) { queued.push({ opcode, argument }); return true; },
        flush_commands() {},
        queue_applied_barrier_words(low, high) { queued.push({ barrier: [low, high] }); return true; },
        finish_replacement_transport(_session, _build, opcode) { queued.push({ restore: true, opcode, releaseEpoch }); return true; },
        finish_load_transport(_session, _build, opcode) { queued.push({ opcode, releaseEpoch }); return true; },
        cancel_load_transport() { releaseEpoch += 1; return true; },
        install_insert(target, slot, wire) { queued.push({ install: [target, slot, wire] }); },
    };
    function quantum() {
        quanta += 1;
        while (queued.length) {
            const action = queued.shift();
            if (action.releaseEpoch !== undefined && action.releaseEpoch !== releaseEpoch) { executed.push({ ...action, skipped: true }); continue; }
            executed.push(action);
            if (action.barrier) { status[0] = action.barrier[0]; status[1] = action.barrier[1]; }
            if (action.opcode === Ring.OPCODE_NOTE_OVERLAP) policy.overlap = action.argument !== 0;
            if (action.opcode === Ring.OPCODE_UNIQUE_NOTES) policy.unique = action.argument !== 0;
            if (action.restore) playing = ![Ring.OPCODE_PAUSE, Ring.OPCODE_HOLD_STOP].includes(action.opcode);
            if (action.opcode === Ring.OPCODE_INTERACTIVE_PLAY || action.opcode === Ring.OPCODE_RESUME) playing = true;
            if (action.opcode === Ring.OPCODE_PAUSE || action.opcode === Ring.OPCODE_HOLD_STOP) playing = false;
        }
        status[2] = 2; status[4] = 3; status[6] = 3; status[7] = 1; status[8] = rejected;
        processor.publishApplied();
        return { playing, ...policy };
    }
    const loaded = { type: 'moduleLoaded', requestId: 90, identity, replacement: true };
    const settings = { volume: 60000, mutes: Array.from({ length: 64 }, (_, index) => index), inserts: [{ target: 255, slot: 0, wire: 1, params: { 0: 10, 1: 20 }, bypassed: true }], policyRevision: 0, overlapEnabled: false, uniqueNotesAcrossChannels: false };
    return { processor, Ring, identity, loaded, settings, queued, executed, messages, quantum, reject: () => { rejected += 1; }, get quanta() { return quanta; } };
}
function drainReplay(harness) {
    for (let index = 0; index < 30 && harness.processor.pendingReplay?.phase !== 'confirm'; index += 1) {
        const observed = harness.quantum();
        assert.equal(observed.playing, false, 'no actual Playing quantum during multi-quantum replay');
        if (harness.processor.pendingReplay?.phase === 'actions') harness.processor.onMessage({ type: 'continueReplay', requestId: 90 });
    }
    assert.equal(harness.processor.pendingReplay?.phase, 'confirm');
}

for (const opcode of [0, 13, 15, 14, 16]) test(`multi-quantum saved Playing waits for latest applied policy before transport ${opcode}`, () => {
    const harness = replayHarness(), { processor, identity, settings, Ring } = harness;
    processor.startReplay(settings, harness.loaded);
    assert.ok(processor.pendingReplay.actions.length > 64);
    harness.quantum();
    processor.onMessage({ type: 'loadPolicies', requestId: 90, identity, policies: { policyRevision: 1, overlapEnabled: true, uniqueNotesAcrossChannels: true } });
    processor.onMessage({ type: 'loadTransport', requestId: 90, identity, opcode: Ring.OPCODE_INTERACTIVE_PLAY, argument: 0, extra: 0 });
    if (opcode) processor.onMessage({ type: 'loadTransport', requestId: 90, identity, opcode, argument: 0, extra: 0 });
    else delete harness.loaded.transportOpcode;
    drainReplay(harness);
    assert.ok(harness.quanta > 3);
    assert.equal(harness.executed.some(action => action.restore), false);
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 2, overlapEnabled: true, uniqueNotesAcrossChannels: false } });
    assert.equal(harness.queued.some(action => action.restore), false, 'new desired revision starts another applied policy barrier');
    drainReplay(harness);
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 1 } });
    assert.equal(harness.queued.some(action => action.restore), false, 'older same-identity revision cannot confirm newer applied policy');
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity: { session: 2n, build: 2n }, policies: { policyRevision: 2 } });
    assert.equal(harness.queued.some(action => action.restore), false, 'wrong identity cannot release saved Playing');
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 2, overlapEnabled: true, uniqueNotesAcrossChannels: false } });
    const observed = harness.quantum();
    assert.equal(observed.playing, ![14, 16].includes(opcode));
    assert.equal(observed.overlap, true); assert.equal(observed.unique, false);
    const restoreIndex = harness.executed.findIndex(action => action.restore);
    assert.ok(restoreIndex > harness.executed.findLastIndex(action => action.opcode === Ring.OPCODE_UNIQUE_NOTES));
    assert.equal(harness.messages.findLast(message => message.type === 'moduleLoaded')?.policyRevision, 2);
});

test('rejected or superseded replay cannot restore saved Playing', () => {
    for (const failure of ['rejected', 'superseded', 'barrier']) {
        const harness = replayHarness(), { processor, settings, identity } = harness;
        if (failure === 'barrier') processor.wasm.queue_applied_barrier_words = () => false;
        processor.startReplay(settings, harness.loaded);
        if (failure === 'rejected') harness.reject();
        if (failure === 'superseded') harness.loaded.cancelled = true;
        for (let index = 0; index < 20 && processor.pendingReplay; index++) {
            harness.quantum();
            if (processor.pendingReplay?.phase === 'actions') processor.continueReplay();
            if (processor.pendingReplay?.phase === 'confirm') processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 0 } });
        }
        assert.equal(harness.executed.some(action => action.restore), false, failure);
        assert.equal(processor.pendingReplay, null, 'failure finishes without a permanently pending replay');
        assert.ok(harness.messages.some(message => message.type === 'moduleError'));
    }
});

test('a policy revision during candidate construction retries with a fresh saved cursor and keeps latest intent', async () => {
    const page = pageHarness(), { state } = page;
    const oldNode = state.node, connected = [], disconnected = [], captures = [], candidates = [];
    let unblock;
    const preparation = new Promise(resolve => { unblock = resolve; });
    Object.assign(state, { context: { destination: {} }, currentModuleBytes: new Uint8Array([1, 2]).buffer, moduleRevision: 2, insertLayout: new Map(), outputChannelCount: 2 });
    Object.assign(page.elements, { outputChannels: { value: '1' } });
    Object.assign(page, {
        startAudio: async () => {}, playback: { controlRevision: 0, rebind() {} },
        captureReplacement: async () => { const saved = { cursor: captures.length, policies: page.pagePolicies() }; captures.push(saved); return saved; },
        prepareNode: async () => {
            const candidate = { port: { postMessage() {} }, connect() { connected.push(candidate); }, disconnect() { disconnected.push(candidate); } };
            candidates.push(candidate);
            if (candidates.length === 1) await preparation;
            return { node: candidate };
        },
        activateModuleOnNode: async () => ({ identity: page.identity, generation: 1 }),
        mixerModeFromControls: () => 0, loadOptionsFromControls: () => ({}),
        installGraph: graph => { state.node = graph.node; }, scheduleMediaPosition() {}, persistPreferences() {}, updateOutputPanel() {}, showMessage() {},
    });
    // pageHarness returns its context properties; the VM needs the same values.
    const sandbox = { ...page, state, elements: page.elements };
    vm.createContext(sandbox);
    vm.runInContext(['pagePolicies', 'publishPendingPolicies', 'setPagePolicy'].map(source).join('\n') + `\nasync ${source('rebuildOutputChannels')}`, sandbox);
    const pending = sandbox.rebuildOutputChannels();
    for (let index = 0; index < 10; index++) await Promise.resolve();
    sandbox.setPagePolicy(page.Ring.OPCODE_UNIQUE_NOTES, true);
    unblock(); await pending;
    assert.equal(captures.length, 2, 'candidate retry recaptures the latest old-node cursor');
    assert.equal(captures[1].policies.uniqueNotesAcrossChannels, true);
    assert.equal(disconnected.includes(candidates[0]), true);
    assert.equal(connected.includes(candidates[0]), false);
    assert.equal(state.node, candidates[1]);
    assert.equal(oldNode === state.node, false);
    assert.equal(state.uniqueNotesAcrossChannels, true);
});

for (const transport of ['saved', 'play', 'resume']) test(`late policy before release quantum revokes ${transport} until the newest policy audio-applies`, () => {
    const harness = replayHarness(), { processor, settings, identity, Ring } = harness;
    harness.loaded.replacement = transport === 'saved';
    if (transport !== 'saved') harness.loaded.transportOpcode = transport === 'play' ? Ring.OPCODE_INTERACTIVE_PLAY : Ring.OPCODE_RESUME;
    processor.startReplay(settings, harness.loaded); drainReplay(harness);
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 0 } });
    assert.equal(processor.pendingReplay.phase, 'releasing');
    processor.onMessage({ type: 'loadPolicies', requestId: 90, identity, policies: { policyRevision: 1, overlapEnabled: true, uniqueNotesAcrossChannels: true } });
    const first = harness.quantum();
    assert.equal(first.playing, false, 'obsolete queued release cannot produce the first Playing sample');
    assert.equal(harness.messages.some(message => message.type === 'moduleLoaded'), false, 'obsolete release acknowledgement cannot enable interaction');
    drainReplay(harness);
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 1, overlapEnabled: true, uniqueNotesAcrossChannels: true } });
    const released = harness.quantum();
    assert.equal(released.playing, true); assert.equal(released.overlap, true); assert.equal(released.unique, true);
    assert.equal(harness.messages.findLast(message => message.type === 'moduleLoaded').policyRevision, 1);
});

for (const opcode of [14, 16]) test(`newest transport ${opcode} revokes a queued saved Playing release at the same policy revision`, () => {
    const harness = replayHarness(), { processor, settings, identity } = harness;
    processor.startReplay(settings, harness.loaded); drainReplay(harness);
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 0 } });
    processor.onMessage({ type: 'loadTransport', requestId: 90, identity, opcode, argument: 0, extra: 0 });
    assert.equal(harness.quantum().playing, false);
    assert.equal(harness.messages.filter(message => message.type === 'moduleLoaded').length, 1, 'only the latest transport release may acknowledge readiness');
    assert.equal(harness.executed.filter(action => action.restore && !action.skipped).at(-1).opcode, opcode);
});

test('a superseding load revokes saved Playing that was already queued for the next quantum', () => {
    const harness = replayHarness(), { processor, settings, identity } = harness;
    processor.startReplay(settings, harness.loaded); drainReplay(harness);
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 0 } });
    processor.onMessage({ type: 'loadModule', requestId: 91, identity: { session: 3n, build: 4n } });
    assert.equal(harness.quantum().playing, false);
    assert.equal(harness.messages.some(message => message.type === 'moduleLoaded'), false);
    assert.equal(processor.pendingReplay, null);
    assert.ok(harness.messages.some(message => message.type === 'moduleError' && message.requestId === 90));
});

for (const opcode of [13, 14, 16]) test(`failed load recovery retains pending transport ${opcode} and accepts newer retained-identity intent`, () => {
    const page = pageHarness(), { state, identity } = page;
    const candidate = { session: 8n, build: identity.build + 1n };
    state.node.starplayerLatestLoadRequest = 91;
    state.pendingLoads.set(91, { node: state.node, identity: candidate, previousIdentity: identity, previousReady: true, reject() {}, resolve() { assert.fail('failed load must stay failed'); } });
    state.interactiveReady = false;
    page.queueTransport(opcode);
    page.onWorkletMessage({ type: 'moduleError', requestId: 91, retainedIdentity: identity, retainedCapabilities: 3, reason: 'bad file' }, state.node);
    assert.equal(page.messages.at(-1).type, 'reconcilePolicies');
    assert.equal(page.messages.at(-1).transportOpcode, opcode);
    assert.deepEqual(page.messages.at(-1).identity, identity);
    page.queueTransport(page.Ring.OPCODE_PAUSE);
    assert.equal(page.messages.at(-1).opcode, page.Ring.OPCODE_PAUSE);
    assert.equal(page.messages.at(-1).identity.session, identity.session);
    assert.equal(page.messages.at(-1).identity.build, identity.build);
    const harness = replayHarness();
    harness.processor.onMessage({ ...page.messages.findLast(message => message.type === 'reconcilePolicies'), requestId: 90, identity: harness.identity });
    assert.equal(harness.processor.pendingReplay.loaded.transportOpcode, opcode);
    drainReplay(harness);
    harness.processor.onMessage({ type: 'completeLoad', requestId: 90, identity: harness.identity, policies: { policyRevision: 0 } });
    assert.equal(harness.quantum().playing, opcode === 13);
});

for (const opcode of [14, 16]) test(`newest transport ${opcode} remains authoritative through a late-policy replay retry`, () => {
    const harness = replayHarness(), { processor, settings, identity } = harness;
    processor.startReplay(settings, harness.loaded); drainReplay(harness);
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 0 } });
    processor.onMessage({ type: 'loadPolicies', requestId: 90, identity, policies: { policyRevision: 1, overlapEnabled: true, uniqueNotesAcrossChannels: true } });
    processor.onMessage({ type: 'loadTransport', requestId: 90, identity, opcode, argument: 0, extra: 0 });
    assert.equal(harness.quantum().playing, false);
    drainReplay(harness);
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 1, overlapEnabled: true, uniqueNotesAcrossChannels: true } });
    const final = harness.quantum();
    assert.equal(final.playing, false); assert.equal(final.overlap, true); assert.equal(final.unique, true);
    assert.equal(harness.executed.filter(action => action.restore && !action.skipped).at(-1).opcode, opcode);
});

for (const late of ['pause', 'stop', 'play-stop-play', 'already-applied']) test(`page delivery of an audio acknowledgement reconciles ${late} by transport revision`, () => {
    const harness = replayHarness(), { processor, settings, identity, Ring } = harness;
    const page = pageHarness(), { state } = page;
    state.node.starplayerIdentity = identity;
    state.node.starplayerLatestLoadRequest = 90;
    state.interactiveReady = false;
    state.pendingLoads.set(90, { node: state.node, identity, resolve() {}, reject() { assert.fail('valid completion'); } });
    let sent = 0;
    const deliverIntents = () => {
        while (sent < page.messages.length) {
            const message = page.messages[sent++];
            if (message.type === 'loadTransport') processor.onMessage(message);
        }
    };
    page.queueTransport(Ring.OPCODE_INTERACTIVE_PLAY);
    processor.startReplay(settings, harness.loaded); deliverIntents(); drainReplay(harness);
    processor.onMessage({ type: 'completeLoad', requestId: 90, identity, policies: { policyRevision: 0 } });
    assert.equal(harness.quantum().playing, true);
    const acknowledged = harness.messages.findLast(message => message.type === 'moduleLoaded');
    assert.equal(acknowledged.transportRevision, 1);
    assert.equal(processor.pendingReplay, null, 'audio has posted completion but the page still has its pending load');
    if (late === 'pause') page.queueTransport(Ring.OPCODE_PAUSE);
    if (late === 'stop') page.queueTransport(Ring.OPCODE_HOLD_STOP);
    if (late === 'play-stop-play') { page.queueTransport(Ring.OPCODE_HOLD_STOP); page.queueTransport(Ring.OPCODE_INTERACTIVE_PLAY); }
    deliverIntents();
    assert.equal(page.commands.length, 0, 'the page still routes intent through the pending request until it consumes completion');
    page.onWorkletMessage(acknowledged, state.node);
    if (late === 'already-applied') {
        assert.equal(page.commands.length, 0, 'an already audio-applied Play must not be queued a second time');
    } else {
        const opcode = late === 'pause' ? Ring.OPCODE_PAUSE : late === 'stop' ? Ring.OPCODE_HOLD_STOP : Ring.OPCODE_INTERACTIVE_PLAY;
        assert.equal(page.commands.length, 1, 'only the newest missing intent is submitted to the installed identity');
        assert.deepEqual(page.commands[0], [opcode, 0, 0, identity]);
        processor.stageLoadTransport(opcode, identity);
        assert.equal(harness.quantum().playing, opcode === Ring.OPCODE_INTERACTIVE_PLAY);
    }
});
