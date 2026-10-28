import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import { createPlaybackController } from '../www/playback-controller.js';

const root = new URL('../dist/', import.meta.url);
const bundle = await readFile(new URL('starplayer-worklet.js', root), 'utf8').catch(() => {
    throw new Error('apps/starplayer-web/dist is missing; run `cargo xtask wasm` first');
});
const wasmBytes = await readFile(new URL('starplayer_host_wasm_bg.wasm', root));
const firstFixture = await readFile(new URL('modules/REFLEX.S3M', root));
const secondFixture = await readFile(new URL('modules/PETRI.S3M', root));

function syntheticMod() {
    const headerBytes = 1084;
    const patternBytes = 64 * 4 * 4;
    const sampleFrames = 256;
    const bytes = new Uint8Array(headerBytes + patternBytes + sampleFrames);
    bytes.set(new TextEncoder().encode('headphone test'), 0);
    bytes.set([(sampleFrames / 2) >> 8, (sampleFrames / 2) & 0xFF], 42);
    bytes[45] = 64;
    bytes[950] = 1;
    bytes.set(new TextEncoder().encode('M.K.'), 1080);
    for (let channel = 0; channel < 4; channel += 1) bytes.set([0x01, 0xAC, 0x10, 0x00], headerBytes + channel * 4);
    for (let index = headerBytes + patternBytes; index < bytes.length; index += 1) {
        bytes[index] = index & 1 ? 0x80 : 0x7F;
    }
    return bytes;
}

let Processor = null;
let lastPort = null;
class FakePort {
    constructor() { this.messages = []; this.onmessage = null; }
    postMessage(message) { this.messages.push(structuredClone(message)); }
    dispatch(message) { this.onmessage?.({ data: message }); }
}
globalThis.sampleRate = 48_000;
globalThis.AudioWorkletProcessor = class {
    constructor() { this.port = new FakePort(); lastPort = this.port; }
};
globalThis.registerProcessor = (name, implementation) => {
    assert.equal(name, 'starplayer-player');
    Processor = implementation;
};

// `AudioWorkletGlobalScope` has no `TextDecoder`, and wasm-bindgen's glue builds one the
// moment the bundle is evaluated. Node does have one, so hiding it is the only way this
// harness can see the failure a browser would — the bundle's own prelude has to supply it.
const nodeTextDecoder = globalThis.TextDecoder;
delete globalThis.TextDecoder;
vm.runInThisContext(bundle, { filename: 'starplayer-worklet.js' });
assert.ok(Processor, 'the bundled processor registered');
assert.notEqual(globalThis.TextDecoder, undefined, 'the bundle supplied its own UTF-8 decoder');
assert.equal(new globalThis.TextDecoder('utf-8').decode(nodeTextDecoder === undefined ? new Uint8Array() : new TextEncoder().encode('vibrato · ü𝄞')), 'vibrato · ü𝄞');
const Ring = globalThis.StarPlayerRing;
const commandRing = Ring.createCommandRing();
const telemetry = Ring.createTelemetry();
const scope = Ring.createScope();
const processor = new Processor({ processorOptions: {
    channelCount: 2,
    mixerMode: 0x0000_0202,
    wasmModule: new WebAssembly.Module(wasmBytes),
    commandRing: commandRing.buffer,
    telemetry: telemetry.buffer,
    scope: scope.buffer,
} });
const port = lastPort;
const ready = port.messages.find((message) => message.type === 'ready');
assert.ok(ready);
assert.equal(ready.scopeTransport, 'SharedArrayBuffer');
assert.equal(ready.scopeBucketFrames, 4, 'the engine downsamples four output frames into one tap bucket');
assert.equal(ready.scopeWindowBuckets, Ring.SCOPE_WINDOW_BUCKETS);

port.dispatch({ type: 'loadModule', requestId: 1, bytes: firstFixture });
const loaded = port.messages.find((message) => message.type === 'moduleLoaded' && message.requestId === 1);
assert.ok(loaded, 'the real fixture loaded into the worklet engine');
const stableMemory = loaded.memoryBytes;

function quantum() {
    const left = new Float32Array(128);
    const right = new Float32Array(128);
    assert.equal(processor.process([], [[left, right]]), true);
    let peak = 0;
    for (const sample of left) peak = Math.max(peak, Math.abs(sample));
    for (const sample of right) peak = Math.max(peak, Math.abs(sample));
    return peak;
}

// Ten seconds of real render at the engine quantum, with no browser anywhere: enough to
// get past the first row and through several pattern rows of REFLEX.
const QUANTA_PER_TEN_SECONDS = Math.round(globalThis.sampleRate * 10 / 128);
let loudest = 0;
let audibleQuanta = 0;
for (let index = 0; index < QUANTA_PER_TEN_SECONDS; index += 1) {
    const peak = quantum();
    loudest = Math.max(loudest, peak);
    if (peak > 0.001) audibleQuanta += 1;
}
assert.ok(loudest > 0.01, `the real S3M engine rendered audible samples (peak ${loudest})`);
assert.ok(audibleQuanta > QUANTA_PER_TEN_SECONDS / 2, `and kept rendering them (${audibleQuanta}/${QUANTA_PER_TEN_SECONDS} quanta)`);
let snapshot = Ring.readTelemetry(telemetry);
assert.ok(snapshot.sequence > 0);
assert.ok(snapshot.channelCount > 0);
assert.equal(snapshot.memoryBytes, stableMemory, 'process did not grow wasm memory');

// The scope taps: real voice state, sampled through the engine's rings, published into
// shared memory. `crates/starplayer-engine/src/scope.rs` is what fills them, and the
// goldens prove the same render is byte-identical with the taps on.
const scopeWindow = Ring.readScope(scope);
assert.ok(scopeWindow, 'the worklet published a scope window');
assert.ok(scopeWindow.generation > 0);
assert.equal(scopeWindow.bucketFrames, 4);
assert.equal(scopeWindow.channelCount, snapshot.channelCount, 'one trace per sounding channel');
const scopePeak = scopeWindow.values.slice(0, scopeWindow.channelCount * scopeWindow.windowBuckets)
    .reduce((peak, value) => Math.max(peak, Math.abs(value)), 0);
assert.ok(scopePeak > 100, `the taps carry the module's signal (peak ${scopePeak})`);
assert.ok(
    scopeWindow.values.slice(scopeWindow.channelCount * scopeWindow.windowBuckets).every((value) => value === 0),
    'channels the module does not use stay silent',
);

Ring.pushCommand(commandRing, Ring.OPCODE_SEEK_ORDER, 1, 0);
quantum();
snapshot = Ring.readTelemetry(telemetry);
assert.equal(snapshot.warnings & 8, 0, 'seek did not hit Engine dyn-source unsupported handling');

Ring.pushCommand(commandRing, Ring.OPCODE_STOP, 0, 0);
quantum();
assert.equal(quantum(), 0, 'Stop ramps to clean silence');
Ring.pushCommand(commandRing, Ring.OPCODE_PLAY, 0, 0);
quantum();

Ring.pushCommand(commandRing, Ring.OPCODE_SET_MIXER_MODE, 0x0000_0221, 0);
port.dispatch({ type: 'flushCommands' });
assert.ok(port.messages.some((message) => message.type === 'mixerModeApplied' && message.active === 0x0000_0221));
quantum();
snapshot = Ring.readTelemetry(telemetry);
assert.equal(snapshot.mixerModeWire, 0x0000_0221);

port.dispatch({ type: 'loadModule', requestId: 2, bytes: secondFixture });
assert.ok(port.messages.some((message) => message.type === 'moduleLoaded' && message.requestId === 2));
quantum();
port.dispatch({ type: 'collectGarbage' });
const garbage = port.messages.findLast((message) => message.type === 'garbageCollected');
assert.ok(garbage.collected >= 1, 'the retired engine Arc reached the garbage channel');
assert.equal(garbage.pending, 0);

const modFixture = syntheticMod();
port.dispatch({ type: 'loadModule', requestId: 3, bytes: modFixture.slice(), headphoneFriendlyModPanning: false });
assert.ok(port.messages.some((message) => message.type === 'moduleLoaded' && message.requestId === 3));
for (let index = 0; index < 3; index += 1) quantum();
snapshot = Ring.readTelemetry(telemetry);
assert.deepEqual(snapshot.channels.slice(0, 4).map((channel) => channel.pan), [-32767, 32767, 32767, -32767]);

port.dispatch({ type: 'loadModule', requestId: 4, bytes: modFixture.slice(), headphoneFriendlyModPanning: true });
assert.ok(port.messages.some((message) => message.type === 'moduleLoaded' && message.requestId === 4));
for (let index = 0; index < 3; index += 1) quantum();
snapshot = Ring.readTelemetry(telemetry);
assert.deepEqual(snapshot.channels.slice(0, 4).map((channel) => channel.pan), [-19660, 19660, 19660, -19660]);

// A malformed file is the one path that carries a string out of wasm, so it is also the
// one that proves the decoder shim works end to end.
port.dispatch({ type: 'loadModule', requestId: 5, bytes: new Uint8Array(64).fill(0x41) });
const rejected = port.messages.findLast((message) => message.type === 'moduleError');
assert.equal(rejected.requestId, 5);
assert.ok(rejected.reason.length > 10, `a readable reason crossed the wasm boundary: ${rejected.reason}`);
assert.ok(quantum() >= 0, 'the engine still renders after a rejected load');

// Output-channel rebuilds construct a candidate while the live node remains connected.
// Both processors inhabit one AudioWorkletGlobalScope, but must own independent wasm
// instances: wasm-bindgen's stock no-modules singleton would reset the first Rust HOST
// and sometimes grow its memory under the live processor's next callback.
const secondCommandRing = Ring.createCommandRing();
const secondTelemetry = Ring.createTelemetry();
const secondProcessor = new Processor({ processorOptions: {
    channelCount: 1,
    mixerMode: 0x0000_0102,
    wasmModule: new WebAssembly.Module(wasmBytes),
    commandRing: secondCommandRing.buffer,
    telemetry: secondTelemetry.buffer,
} });
const secondPort = lastPort;
assert.notEqual(secondProcessor.memory, processor.memory, 'overlapping processors own independent WebAssembly.Memory objects');
secondPort.dispatch({ type: 'loadModule', requestId: 6, bytes: firstFixture.slice() });
assert.ok(secondPort.messages.some((message) => message.type === 'moduleLoaded' && message.requestId === 6));
assert.equal(secondProcessor.process([], [[new Float32Array(128)]]), true);
assert.equal(Ring.readTelemetry(secondTelemetry).moduleGeneration, 1, 'the candidate has its own Rust HOST');
quantum();
assert.equal(Ring.readTelemetry(telemetry).moduleGeneration, 4, 'the live processor retained its Rust HOST');
assert.equal(port.messages.some((message) => message.type === 'fault'), false, 'the live processor saw no candidate memory growth');
assert.equal(secondPort.messages.some((message) => message.type === 'fault'), false, 'the candidate stayed stable too');

// The `postMessage` fallback carries the same window on the telemetry message rather
// than a second post, and never touches shared memory (architecture Q1 resolution).
const fallbackProcessor = new Processor({ processorOptions: {
    channelCount: 2,
    mixerMode: 0x0000_0202,
    wasmModule: new WebAssembly.Module(wasmBytes),
    commandRing: null,
    telemetry: null,
    scope: null,
} });
const fallbackPort = lastPort;
assert.equal(fallbackPort.messages.find((message) => message.type === 'ready').scopeTransport, 'postMessage');
fallbackPort.dispatch({ type: 'loadModule', requestId: 7, bytes: firstFixture.slice() });
assert.ok(fallbackPort.messages.some((message) => message.type === 'moduleLoaded' && message.requestId === 7));
for (let index = 0; index < 200; index += 1) {
    fallbackProcessor.process([], [[new Float32Array(128), new Float32Array(128)]]);
}
const posted = fallbackPort.messages.findLast((message) => message.type === 'telemetry');
assert.ok(posted, 'the fallback path posted a telemetry batch');
assert.ok(posted.scope, 'and the scope window rode along on it');
assert.equal(posted.scope.bucketFrames, 4);
assert.equal(posted.scope.channelCount, posted.channelCount);
assert.equal(posted.scope.values.length, posted.scope.channelCount * posted.scope.windowBuckets, 'active channels only');
const fallbackPeak = posted.scope.values.reduce((peak, value) => Math.max(peak, Math.abs(value)), 0);
assert.ok(fallbackPeak > 100, `the fallback taps carry the signal too (peak ${fallbackPeak})`);

// ── live input: opcode 10 on both transports (task E6) ──────────────────────────────
//
// The whole path, with no browser: install the live-input source from a port message,
// push a framed MIDI message as an ordinary wire record, and hear the module's own
// instrument sound beside its pattern data. Both transports, because the
// `postMessage` fallback is where architecture 9.1's Q1 resolution says these events
// still have to cross.

/// One processor with the S3M loaded, for a live-input run that must not disturb the
/// assertions above.
function liveInputProcessor(shared) {
    const ring = shared ? Ring.createCommandRing() : null;
    const telemetryBlock = shared ? Ring.createTelemetry() : null;
    const built = new Processor({ processorOptions: {
        channelCount: 2,
        mixerMode: 0x0000_0202,
        wasmModule: new WebAssembly.Module(wasmBytes),
        commandRing: ring ? ring.buffer : null,
        telemetry: telemetryBlock ? telemetryBlock.buffer : null,
        scope: null,
    } });
    const builtPort = lastPort;
    builtPort.dispatch({ type: 'loadModule', requestId: 100, bytes: firstFixture.slice() });
    assert.ok(builtPort.messages.some((message) => message.type === 'moduleLoaded' && message.requestId === 100));
    return { processor: built, port: builtPort, ring };
}

/// Render `count` quanta and report the loudest sample in them.
function renderPeak(target, count) {
    let highest = 0;
    for (let index = 0; index < count; index += 1) {
        const left = new Float32Array(128);
        const right = new Float32Array(128);
        assert.equal(target.process([], [[left, right]]), true);
        for (const sample of left) highest = Math.max(highest, Math.abs(sample));
        for (const sample of right) highest = Math.max(highest, Math.abs(sample));
    }
    return highest;
}

function sendMidi(live, status, data1, data2) {
    const argument = Ring.packMidiMessage(status, data1, data2);
    if (live.ring !== null) {
        assert.equal(Ring.pushCommand(live.ring, Ring.OPCODE_MIDI_EVENT, argument, 0), true);
    } else {
        live.port.dispatch({ type: 'commandBatch', commands: [Ring.OPCODE_MIDI_EVENT, argument, 0] });
    }
}

for (const shared of [true, false]) {
    const transport = shared ? 'SharedArrayBuffer' : 'postMessage';
    const live = liveInputProcessor(shared);
    // Let the module play first, then verify installing live input leaves it playing.
    assert.ok(renderPeak(live.processor, 400) > 0.01, `${transport}: the module plays before live input`);

    const memoryBeforeInstall = live.processor.memory.buffer.byteLength;
    live.port.dispatch({ type: 'midiInput', enabled: true });
    const applied = live.port.messages.findLast((message) => message.type === 'midiInputApplied');
    assert.ok(applied, `${transport}: the worklet answered the install`);
    assert.equal(applied.active, true);
    assert.equal(applied.leadFrames, 256, 'two render quanta, as master-plan decision 6 chose');
    assert.ok(Math.abs(applied.leadMillis - 256 * 1000 / globalThis.sampleRate) < 0.001, `${applied.leadMillis} ms`);
    const stableAfterInstall = applied.memoryBytes;
    assert.ok(stableAfterInstall >= memoryBeforeInstall, 'the rack is allocated in the message task, never in process()');

    // Toggling jam mode rebuilds the module's sequencer at the sounding song position,
    // exactly as a seek does: the song keeps its place, but the voices that were ringing
    // are gone and the module is audible again from its next *note*, which in this
    // fixture is several rows away. The window covers that rather than pretending the
    // toggle is gapless.
    assert.ok(renderPeak(live.processor, 400) > 0.001, `${transport}: the module keeps playing in jam mode`);

    sendMidi(live, 0xC0, 0, 0);          // program change: MIDI channel 0 takes instrument 0
    sendMidi(live, 0x90, 60, 100);       // note on, middle C at the engine's reference note
    const sounded = renderPeak(live.processor, 60);
    assert.ok(sounded > 0.001, `${transport}: the live note sounded beside the module (peak ${sounded})`);

    sendMidi(live, 0x80, 60, 0);         // note off
    sendMidi(live, 0xB0, 120, 0);        // all sound off
    renderPeak(live.processor, 8);
    assert.ok(renderPeak(live.processor, 16) > 0.001, `${transport}: all-sound-off leaves the module playing`);

    // Design goal 5 on the shared-memory path: nothing in `process()` may allocate, and a
    // growing `WebAssembly.Memory` is how that would show. The processor itself posts a
    // `fault` the moment it sees one.
    assert.equal(live.processor.memory.buffer.byteLength, stableAfterInstall, `${transport}: process() did not grow wasm memory`);
    assert.equal(live.port.messages.some((message) => message.type === 'fault'), false, `${transport}: no growth fault`);

    live.port.dispatch({ type: 'midiInput', enabled: false });
    const removed = live.port.messages.findLast((message) => message.type === 'midiInputApplied');
    assert.equal(removed.active, false, `${transport}: live input goes off again`);
    assert.ok(renderPeak(live.processor, 400) > 0.01, `${transport}: disabling jam does not interrupt the module`);
}


// NO-W1: use the same packaged Rust WASM and worklet with negotiated v2 commands.
for (const shared of process.env.STARPLAYER_NO08_TRANSPORT === 'fallback' ? [false] : [true, false]) {
    const ring = shared ? Ring.createCommandRing(2) : null;
    const target = new Processor({ processorOptions: { channelCount: 2, mixerMode: 0x202, interactiveVersion: 2, seedSession: 0n, seedBuild: 0n, wasmModule: new WebAssembly.Module(wasmBytes), commandRing: ring?.buffer } });
    const targetPort = lastPort;
    const targetReady = targetPort.messages.find(message => message.type === 'ready');
    assert.equal(targetReady.leadFrames, 256);
    assert.ok(Math.abs(targetReady.leadMillis - 256 * 1000 / sampleRate) < 0.001);
    const identity = { session: (1n << 59n) + 11n, build: (1n << 62n) + 37n };
    const left = new Float32Array(128), right = new Float32Array(128);
    const pump = (count = 1) => { let peak = 0; for (let index = 0; index < count; index++) { target.process([], [[left, right]]); for (const sample of left) peak = Math.max(peak, Math.abs(sample)); } return peak; };
    const send = (opcode, argument = 0, extra = 0, generation = identity) => {
        if (ring) assert.equal(Ring.pushCommand(ring, opcode, argument, extra, generation), true);
        else targetPort.dispatch({ type: 'commandBatch', version: 2, commands: [{ opcode, argument, extra, identity: generation }] });
    };
    targetPort.dispatch({ type: 'loadModule', requestId: 501, bytes: syntheticMod(), identity });
    assert.equal(targetPort.messages.some(message => message.type === 'moduleLoaded'), false, 'prepared is not audio-applied');
    pump(4);
    const installed = targetPort.messages.find(message => message.type === 'moduleLoaded');
    assert.ok(installed?.accepted, JSON.stringify(targetPort.messages, (_, value) => typeof value === 'bigint' ? String(value) : value));
    assert.deepEqual(installed.identity, identity);
    assert.equal(installed.capabilities, 3);
    assert.equal(target.telemetryWords[2], 4, 'reserved live lanes do not inflate visible tracker width');
    targetPort.dispatch({ type: 'loadModule', requestId: 502, bytes: new Uint8Array(64).fill(65), identity: { session: identity.session + 1n, build: identity.build + 1n } });
    const retainedError = targetPort.messages.findLast(message => message.type === 'moduleError');
    assert.equal(retainedError.requestId, 502);
    assert.deepEqual(retainedError.retainedIdentity, identity, 'failed audio decode proves unchanged control and audio-applied identity');
    assert.equal(retainedError.retainedCapabilities, 3);
    const loadIdentified = target.wasm.load_module_identified;
    target.wasm.load_module_identified = (...arguments_) => { target.memory.grow(1); return loadIdentified(...arguments_); };
    targetPort.dispatch({ type: 'loadModule', requestId: 503, bytes: new Uint8Array(64).fill(65), identity: { session: identity.session + 1n, build: identity.build + 1n } });
    target.wasm.load_module_identified = loadIdentified;
    assert.deepEqual(targetPort.messages.findLast(message => message.type === 'moduleError').retainedIdentity, identity, 'growth before failure rebases proof and output views');
    assert.ok(target.channelViews[0].length > 0); pump(2);

    targetPort.dispatch({ type: 'midiInput', enabled: true });
    assert.ok(targetPort.messages.some(message => message.type === 'midiInputError'));
    assert.equal(target.wasm.current_build(), identity.build);
    // Atomic rejection and UI identity are independent of mixed record order.
    const staleIdentity = { ...identity, build: identity.build - 1n };
    const validEnable = { opcode: Ring.OPCODE_NOTE_OVERLAP, argument: 1, extra: 0, identity };
    const staleBox = { opcode: Ring.OPCODE_LET_NOTES_RING, argument: 0, extra: 1, identity: staleIdentity };
    const originalBarrier = target.wasm.queue_applied_barrier_words;
    for (const barrierFailure of [false, true]) {
        if (barrierFailure) target.wasm.queue_applied_barrier_words = () => false;
        for (const records of [[validEnable, staleBox], [staleBox, validEnable], [staleBox]]) {
            if (ring) for (const record of records) Ring.pushCommand(ring, record.opcode, record.argument, record.extra, record.identity);
            else targetPort.dispatch({ type: 'commandBatch', version: 2, commands: records });
            pump(4);
            const acknowledgement = targetPort.messages.findLast(message => message.type === 'interactiveApplied');
            assert.equal(acknowledgement?.accepted, false);
            const words = acknowledgement.words;
            const acknowledged = { session: BigInt(words[2]) | (BigInt(words[3]) << 32n), build: BigInt(words[4]) | (BigInt(words[5]) << 32n) };
            assert.deepEqual(acknowledged, records.length === 1 ? staleIdentity : identity, 'current members receive rollback; wholly stale groups retain stale identity');
            assert.equal(target.wasm.replacement_state()[6], 0n, 'rejected group must leave overlap unchanged');
        }
    }
    // NO-08: bypass producer-side filtering to exercise a foreign/malformed v2 group.
    target.wasm.queue_applied_barrier_words = originalBarrier;
    const rawGroup = records => {
        if (ring) {
            for (const record of records) {
                const write = Atomics.load(ring.words, 0);
                const base = ring.headerWords + (write % Ring.COMMAND_CAPACITY) * ring.recordWords;
                const supplied = record.identity ?? identity;
                ring.words.set([record.opcode, record.argument, record.extra, Number(supplied.session & 0xffffffffn), Number(supplied.session >> 32n), Number(supplied.build & 0xffffffffn), Number(supplied.build >> 32n)], base);
                Atomics.store(ring.words, 0, (write + 1) % (Ring.COMMAND_CAPACITY * 1024));
            }
        } else targetPort.dispatch({ type: 'commandBatch', version: 2, commands: records });
    };
    for (const malformed of [
        { opcode: Ring.OPCODE_NOTE_OVERLAP, argument: 2, extra: 0, identity },
        { opcode: 99, argument: 0, extra: 0, identity },
    ]) {
        for (const records of [[validEnable, malformed], [malformed, validEnable], [malformed]]) {
            const acknowledgements = targetPort.messages.filter(message => message.type === 'interactiveApplied').length;
            rawGroup(records); pump(4);
            const replies = targetPort.messages.filter(message => message.type === 'interactiveApplied');
            assert.equal(replies.length, acknowledgements + 1, 'malformed-only group must receive explicit rejection');
            assert.equal(replies.at(-1).accepted, false, 'malformed member rejects its whole group');
            assert.deepEqual([...replies.at(-1).words.slice(2, 6)], [Number(identity.session & 0xffffffffn), Number(identity.session >> 32n), Number(identity.build & 0xffffffffn), Number(identity.build >> 32n)]);
            assert.equal(target.wasm.replacement_state()[6], 0n, 'valid member must not partially apply');
        }
    }
    if (!ring) {
        for (const records of [[validEnable, { opcode: 14, argument: 0, extra: 0, identity: { session: 1, build: identity.build } }], [{ opcode: 14, argument: 0, extra: 0, identity: null }], [{ opcode: 14, argument: 1n, extra: 0, identity }], [{ opcode: 14, argument: 0, extra: 1n, identity }]]) {
            const count = targetPort.messages.filter(message => message.type === 'interactiveApplied').length;
            rawGroup(records); pump(4);
            const replies = targetPort.messages.filter(message => message.type === 'interactiveApplied');
            assert.equal(replies.length, count + 1); assert.equal(replies.at(-1).accepted, false);
            assert.equal(target.wasm.replacement_state()[6], 0n);
        }
    }
    target.wasm.queue_applied_barrier_words = originalBarrier;
    send(Ring.OPCODE_NOTE_OVERLAP, 1); pump(4);
    assert.ok(targetPort.messages.findLast(message => message.type === 'interactiveApplied')?.accepted, 'valid retry must apply');
    assert.equal(target.wasm.replacement_state()[6], 1n);
    send(Ring.OPCODE_NOTE_OVERLAP, 0); pump(4);
    const gainWire = JSON.parse(target.wasm.effects_json()).find(effect => effect.name === 'gain').wire;
    for (const invalid of [{ identity: { ...identity, build: identity.build - 1n } }, { version: 1 }, { target: 64 }, { slot: 4 }, { kind: 999 }, { action: 'unknown' }]) {
        targetPort.dispatch({ type: 'inserts', version: 2, identity, requestId: 600, action: 'install', target: 255, slot: 0, kind: gainWire, ...invalid });
        assert.equal(targetPort.messages.at(-1).type, 'insertsError');
    }
    const preparationOrder = [];
    const originalFlush = target.wasm.flush_commands, originalInstall = target.wasm.install_insert;
    target.wasm.flush_commands = (...args) => { preparationOrder.push('flush'); return originalFlush(...args); };
    target.wasm.install_insert = (...args) => { preparationOrder.push('install'); return originalInstall(...args); };
    send(Ring.OPCODE_MASTER_VOLUME, 65535);
    targetPort.dispatch({ type: 'inserts', version: 2, identity, requestId: 601, action: 'install', target: 255, slot: 0, kind: gainWire });
    // The fallback batch may already own a pending barrier. A busy rejection happens
    // before mutation; retry after that batch has reached audio.
    if (targetPort.messages.some(message => message.type === 'insertsError' && message.requestId === 601)) { pump(4); targetPort.dispatch({ type: 'inserts', version: 2, identity, requestId: 601, action: 'install', target: 255, slot: 0, kind: gainWire }); }
    assert.equal(targetPort.messages.some(message => message.type === 'insertsApplied' && message.requestId === 601), false, 'prepared insert is not audio applied');
    pump(4);
    assert.ok(targetPort.messages.findLast(message => message.type === 'insertsApplied' && message.requestId === 601)?.accepted);
    assert.ok(preparationOrder.indexOf('flush') < preparationOrder.indexOf('install'));
    target.wasm.flush_commands = originalFlush; target.wasm.install_insert = originalInstall;
    for (const slot of [1, 2]) targetPort.dispatch({ type: 'inserts', version: 2, identity, requestId: 603 + slot, action: 'install', target: 255, slot, kind: gainWire });
    pump(4);
    for (const slot of [1, 2]) assert.ok(targetPort.messages.findLast(message => message.type === 'insertsApplied' && message.requestId === 603 + slot)?.accepted, 'multiple pending preparations fit complete transactions');
    for (const slot of [0, 1, 2]) targetPort.dispatch({ type: 'inserts', version: 2, identity, requestId: 620 + slot, action: 'remove', target: 255, slot });
    pump(4);
    for (const slot of [0, 1, 2]) assert.ok(targetPort.messages.findLast(message => message.type === 'insertsApplied' && message.requestId === 620 + slot)?.accepted, 'Reset can clear several slots before their acknowledgements');
    target.nextBarrierLow = 0xfffffffe; target.nextBarrierHigh = 0x200000; // barrier serials above2^53, including low-word wrap
    send(Ring.OPCODE_PREVIEW, 0);
    assert.ok(pump(12) > 0.001, 'first Preview is audible before first Play');
    // Stop submitted immediately after message-task insert removal must not be lost
    // behind that preparation's applied barrier, on either transport.
    send(Ring.OPCODE_INTERACTIVE_PLAY); pump(4);
    targetPort.dispatch({ type: 'inserts', version: 2, identity, requestId: 640, action: 'remove', target: 255, slot: 0 });
    send(Ring.OPCODE_STOP_PREVIEW); send(Ring.OPCODE_HOLD_STOP); pump(4);
    assert.equal(target.wasm.replacement_state()[4], 0n, 'pending insert removal cannot swallow Stop');
    assert.ok(targetPort.messages.findLast(message => message.type === 'interactiveApplied')?.accepted);
    send(Ring.OPCODE_INTERACTIVE_PLAY); pump(4);
    const playingBeforeLegacyStop = target.wasm.replacement_state();
    send(Ring.OPCODE_STOP); pump(4);
    const playingAfterLegacyStop = target.wasm.replacement_state();
    assert.equal(playingAfterLegacyStop[4], 1n, 'v2 rejects legacy Stop2; explicit Stop16 owns interactive transport');
    assert.equal(playingAfterLegacyStop[9], playingBeforeLegacyStop[9], 'rejected legacy Stop does not fade the live output gain');
    assert.ok(playingAfterLegacyStop[41] > playingBeforeLegacyStop[41], 'rejected legacy Stop does not hold sequencing');
    send(Ring.OPCODE_PAUSE); pump(4);
    const paused = target.wasm.replacement_state(); // harness control task, outside process
    assert.equal(paused[4], 2n);
    send(Ring.OPCODE_PREVIEW, 0);
    assert.ok(pump(12) > 0.001, 'Preview remains audible while paused');
    const held = target.wasm.replacement_state();
    assert.equal(held[41], paused[41], 'audition did not advance the held song frame');
    rawGroup([...Array.from({ length: 5 }, () => ({ opcode: Ring.OPCODE_RESUME, argument: 0, extra: 0, identity })), { opcode: Ring.OPCODE_ALL_SOUND_OFF, argument: 0, extra: 0, identity }, { opcode: Ring.OPCODE_PAUSE, argument: 0, extra: 0, identity }]);
    pump();
    const burst = Ring.decodeWasmTelemetry(target.telemetryWords, 0, 128, 0, 0);
    assert.equal(burst.transport, 'paused'); assert.equal(burst.live.active, 0, 'burst publishes final live clear immediately');
    assert.equal(target.wasm.replacement_state()[41], held[41], 'burst publishes final held cursor'); pump(3);
    // Stale commands using old opcode VALUES also carry/reject full identity.
    const stale = { ...identity, build: identity.build - 1n };
    for (const [opcode, argument, extra] of [[Ring.OPCODE_SEEK_ROW, 20, 0], [Ring.OPCODE_MUTE_CHANNEL, 0, 1], [Ring.OPCODE_INSERT_BYPASS, 255, 1], [Ring.OPCODE_SET_MIXER_MODE, 0x201, 0], [Ring.OPCODE_LIVE_MIDI, Ring.packMidiMessage(0x90, 60, 100), 0]]) { send(opcode, argument, extra, stale); pump(4); }
    assert.equal(target.wasm.replacement_state()[41], paused[41]);
    send(Ring.OPCODE_SET_MIXER_MODE, 0x201); pump(4);
    assert.equal(target.wasm.current_build(), identity.build, 'v2 opcode7 is rejected; replacement messages own mixer builds');
    assert.ok(targetPort.messages.some(message => message.type === 'interactiveApplied' && !message.accepted));
    send(Ring.OPCODE_HOLD_STOP); pump(4);
    send(Ring.OPCODE_PREVIEW, 0); assert.ok(pump(12) > 0.001, 'Preview remains audible after Stop');
    send(Ring.OPCODE_AT_END, Ring.AT_END_STOP); pump(4); send(Ring.OPCODE_SEEK_ROW, 63); pump(4); send(Ring.OPCODE_INTERACTIVE_PLAY); pump(100);
    assert.ok((target.telemetryWords[15] & 4) !== 0, 'natural end has its explicit transport indication');
    send(Ring.OPCODE_PREVIEW, 0); assert.ok(pump(12) > 0.001, 'Preview remains audible after natural end');
    const ended = [];
    const observe = () => Ring.decodeWasmTelemetry(target.telemetryWords, target.memory.buffer.byteLength, 128, 0, target.wasm.quanta_rendered());
    const controller = createPlaybackController({ resolver: {}, activate: async () => ({ graph: target, generation: observe().moduleGeneration }), readSnapshot: () => ({ graph: target, snapshot: observe() }), onTrackEnded: entry => ended.push(entry), clock: { setInterval: () => 1, clearInterval: () => {} } });
    await controller.play({ kind: 'bytes', buffer: new Uint8Array() }, { origin: 'playlist', entryId: 'no08' });
    controller.tick(); assert.deepEqual(ended, ['no08']);
    controller.userControl('pause'); send(Ring.OPCODE_PAUSE); pump(4); controller.tick();
    controller.userControl('play'); send(Ring.OPCODE_RESUME); pump(); controller.tick();
    assert.equal(observe().songFrame, 0, 'next Resume consumes armed rewind after Pause');
    assert.equal(observe().naturalEnd, false);
    pump(12); controller.tick();
    assert.equal(observe().transport, 'playing'); assert.deepEqual(ended, ['no08'], 'no second playlist ended event');
    controller.dispose();
    // Restore the natural Stopped handoff used by the following recreation checks.
    send(Ring.OPCODE_SEEK_ROW, 63); pump(4); pump(100);
    assert.equal(observe().naturalEnd, true);
    targetPort.dispatch({ type: 'captureState', requestId: 502, identity }); pump(4);
    let capture = targetPort.messages.findLast(message => message.type === 'stateReady');
    if (!capture) { targetPort.dispatch({ type: 'captureState', requestId: 502, identity }); pump(4); capture = targetPort.messages.findLast(message => message.type === 'stateReady'); }
    assert.ok(capture?.accepted);
    targetPort.dispatch({ type: 'readState', requestId: 502, identity, serial: BigInt(capture.serial) });
    const saved = targetPort.messages.findLast(message => message.type === 'stateCaptured');
    assert.ok(saved?.replacement instanceof BigUint64Array, 'opaque codec crosses WASM without Number conversion');
    const nextIdentity = { session: identity.session, build: identity.build + 1n };
    const recreated = new Processor({ processorOptions: { channelCount: 2, mixerMode: 0x202, interactiveVersion: 2, wasmModule: new WebAssembly.Module(wasmBytes) } });
    const recreatedPort = lastPort;
    recreatedPort.dispatch({ type: 'loadModule', requestId: 503, bytes: syntheticMod(), identity: nextIdentity, replacement: saved.replacement, hostSettings: { volume: 65535, mutes: Array.from({ length: 64 }, (_, index) => index), inserts: [], atEnd: Ring.AT_END_STOP, fadeFrames: 4800 } });
    assert.equal(recreatedPort.messages.some(message => message.type === 'moduleLoaded'), false);
    let delivered = 0;
    for (let index = 0; index < 32; index++) {
        recreated.process([], [[left, right]]);
        while (delivered < recreatedPort.messages.length) {
            const message = recreatedPort.messages[delivered++];
            if (message.type === 'loadReplayPending') recreatedPort.dispatch({ type: 'continueReplay', requestId: message.requestId });
                if (message.type === 'loadReplayReady') recreatedPort.dispatch({ type: 'completeLoad', requestId: message.requestId, identity: message.identity, policies: { policyRevision: message.policyRevision, overlapEnabled: false, uniqueNotesAcrossChannels: false } });
        }
    }
    assert.ok(recreatedPort.messages.find(message => message.type === 'moduleLoaded')?.accepted);
    assert.equal(recreated.wasm.replacement_state()[1], identity.session);
    assert.equal(recreated.wasm.replacement_state()[4], 0n, 'clear-and-hold survives actual WASM recreation');
    recreatedPort.dispatch({ type: 'commandBatch', version: 2, commands: [{ opcode: Ring.OPCODE_INTERACTIVE_PLAY, argument: 0, extra: 0, identity: nextIdentity }] });
    for (let index = 0; index < 4; index++) recreated.process([], [[left, right]]);
    recreatedPort.dispatch({ type: 'commandBatch', version: 2, commands: [{ opcode: Ring.OPCODE_SEEK_ROW, argument: 63, extra: 0, identity: nextIdentity }] });
    for (let index = 0; index < 100; index++) recreated.process([], [[left, right]]);
    assert.ok((recreated.telemetryWords[15] & 4) !== 0, 'recreated host applies retained at-end policy before readiness, rather than silently wrapping');

    // Capture a genuinely Playing source, then stop/pause while installation is
    // deferred behind an earlier control barrier, before the readiness barrier.
    send(Ring.OPCODE_INTERACTIVE_PLAY); pump(4);
    const playingState = target.wasm.replacement_state();
    assert.equal(playingState[4], 1n);
    for (const opcode of [Ring.OPCODE_PAUSE, Ring.OPCODE_HOLD_STOP]) {
        const pendingProcessor = new Processor({ processorOptions: { channelCount: 2, mixerMode: 0x202, interactiveVersion: 2, wasmModule: new WebAssembly.Module(wasmBytes) } });
        const pendingPort = lastPort;
        const initialIdentity = { session: identity.session, build: identity.build + 2n };
        pendingPort.dispatch({ type: 'loadModule', requestId: 610, bytes: syntheticMod(), identity: initialIdentity, replacement: playingState });
        for (let index = 0; index < 4; index++) pendingProcessor.process([], [[left, right]]);
        const currentState = pendingProcessor.wasm.replacement_state();
        pendingPort.dispatch({ type: 'commandBatch', version: 2, commands: [{ opcode: Ring.OPCODE_MASTER_VOLUME, argument: 65535, extra: 0, identity: initialIdentity }] });
        const replacementIdentity = { session: initialIdentity.session, build: initialIdentity.build + 1n };
        pendingPort.dispatch({ type: 'loadModule', requestId: 611, bytes: syntheticMod(), identity: replacementIdentity, replacement: currentState, hostSettings: { volume: 65535, mutes: Array.from({ length: 64 }, (_, index) => index), inserts: [] } });
        assert.ok(pendingPort.messages.some(message => message.type === 'loadDeferred' && message.requestId === 611));
        pendingPort.dispatch({ type: 'loadTransport', requestId: 611, identity: replacementIdentity, opcode, argument: 0, extra: 0 });
        for (let index = 0; index < 4; index++) pendingProcessor.process([], [[left, right]]);
        pendingPort.dispatch({ type: 'continueLoad', requestId: 611 });
        let read = 0;
        for (let index = 0; index < 32; index++) {
            pendingProcessor.process([], [[left, right]]);
            while (read < pendingPort.messages.length) {
                const message = pendingPort.messages[read++];
                if (message.type === 'loadReplayPending') pendingPort.dispatch({ type: 'continueReplay', requestId: message.requestId });
                if (message.type === 'loadReplayReady') pendingPort.dispatch({ type: 'completeLoad', requestId: message.requestId, identity: message.identity, policies: { policyRevision: message.policyRevision, overlapEnabled: false, uniqueNotesAcrossChannels: false } });
            }
        }
        assert.ok(pendingPort.messages.findLast(message => message.type === 'moduleLoaded' && message.requestId === 611)?.accepted);
        const restored = pendingProcessor.wasm.replacement_state();
        assert.equal(restored[4], opcode === Ring.OPCODE_PAUSE ? 2n : 0n);
        assert.equal(restored[41], currentState[41], 'pending user intent stops sequencing before readiness, with no resurrected Playing interval');
        pendingPort.dispatch({ type: 'loadTransport', requestId: 611, identity: initialIdentity, opcode: Ring.OPCODE_INTERACTIVE_PLAY, argument: 0, extra: 0 });
        pendingProcessor.process([], [[left, right]]);
        assert.equal(pendingProcessor.wasm.replacement_state()[4], restored[4], 'delayed old intent cannot restart the new build');
    }
    const partial = new Processor({ processorOptions: { channelCount: 2, mixerMode: 0x202, interactiveVersion: 2, wasmModule: new WebAssembly.Module(wasmBytes) } });
    const partialPort = lastPort;
    partialPort.dispatch({ type: 'loadModule', requestId: 620, bytes: syntheticMod(), identity });
    for (let index = 0; index < 4; index++) partial.process([], [[left, right]]);
    partialPort.dispatch({ type: 'loadModule', requestId: 621, bytes: syntheticMod(), identity: { session: identity.session + 1n, build: identity.build + 1n }, hostSettings: { volume: 65535, mutes: null, inserts: [] } });
    const partialError = partialPort.messages.findLast(message => message.type === 'moduleError');
    assert.equal(partialError.requestId, 621);
    assert.equal(partialError.retainedIdentity, undefined, 'queued new installation cannot claim old audio retention after replay preparation fails');

}

globalThis.TextDecoder = nodeTextDecoder;
console.log(`worklet harness: 10 s of real S3M render (peak ${loudest.toFixed(3)}), independent overlapping processors, MOD panning, transport, seek, memory, garbage, scope taps on both transports, live MIDI input on both transports and bad-file rejection passed`);

// NO-08: exact real-DSP output under musically invalid wire seeks, on both transports.
for (const shared of [true, false]) {
    function seekRun(request) {
        const ring = shared ? Ring.createCommandRing(2) : null;
        const target = new Processor({ processorOptions: { channelCount: 2, mixerMode: 0x202, interactiveVersion: 2, wasmModule: new WebAssembly.Module(wasmBytes), commandRing: ring?.buffer } });
        const targetPort = lastPort;
        const identity = { session: 900n, build: 901n };
        const left = new Float32Array(128), right = new Float32Array(128);
        let replayRead = 0;
        const pump = count => { const output = []; for (let index = 0; index < count; index++) {
            target.process([], [[left, right]]); output.push(...left, ...right);
            while (replayRead < targetPort.messages.length) {
                const message = targetPort.messages[replayRead++];
                if (message.type === 'loadReplayPending') targetPort.dispatch({ type: 'continueReplay', requestId: message.requestId });
                if (message.type === 'loadReplayReady') targetPort.dispatch({ type: 'completeLoad', requestId: message.requestId, identity: message.identity, policies: { policyRevision: message.policyRevision, overlapEnabled: false, uniqueNotesAcrossChannels: false } });
            }
        } return output; };
        const send = (opcode, argument = 0) => {
            if (ring) assert.equal(Ring.pushCommand(ring, opcode, argument, 0, identity), true);
            else targetPort.dispatch({ type: 'commandBatch', version: 2, commands: [{ opcode, argument, extra: 0, identity }] });
        };
        const bytes = syntheticMod(); bytes[48] = 0; bytes[49] = 128; // native looping song and Preview
        const delay = JSON.parse(target.wasm.effects_json()).find(effect => effect.name === 'delay').wire;
        targetPort.dispatch({ type: 'loadModule', requestId: 800, bytes, identity, hostSettings: { volume: 10000, mutes: [], inserts: [0, 255].map(target => ({ target, slot: 0, wire: delay, params: { 0: 10, 1: 75, 2: 70 }, bypassed: false })) } });
        pump(4); assert.ok(targetPort.messages.find(message => message.type === 'moduleLoaded')?.accepted);
        send(Ring.OPCODE_INTERACTIVE_PLAY); pump(16);
        send(Ring.OPCODE_PAUSE); pump(4); send(Ring.OPCODE_PREVIEW); pump(8);
        const before = target.wasm.replacement_state()[41];
        if (request) send(...request);
        const output = pump(16);
        const state = Ring.decodeWasmTelemetry(target.telemetryWords, 0, 128, 0, 0);
        assert.equal(targetPort.messages.some(message => message.type === 'fault'), false);
        return { output, state, before };
    }
    const reference = seekRun(null);
    assert.ok(reference.output.some(sample => sample !== 0));
    for (const request of [[Ring.OPCODE_SEEK_ORDER, 999], [Ring.OPCODE_SEEK_ROW, 999], [Ring.OPCODE_SEEK_FRAME, 99999999]]) {
        const actual = seekRun(request);
        assert.deepEqual(actual.output, reference.output, `invalid seek ${request[0]} preserves all actual delay/voice PCM`);
        assert.equal(actual.state.songFrame, reference.state.songFrame); assert.equal(actual.state.live.active, reference.state.live.active);
    }
    const valid = seekRun([Ring.OPCODE_SEEK_ROW, 3]);
    assert.notDeepEqual(valid.output, reference.output); assert.equal(valid.state.live.active, reference.state.live.active); assert.equal(valid.state.channels[0].active, false);
}
console.log('NO08 malformed groups, natural-end Pause/Resume controller and exact DSP seek PCM passed on SAB and fallback');


// NO08-A1: malformed fallback opcode types must reach the existing whole-group
// rejection path without numeric coercion or an exception at the actual WASM export.
const malformedOpcodeFailures = [];
for (const opcode of [14.5, '14', 14n]) {
    for (const order of ['alone', 'before', 'after']) {
        const label = `${typeof opcode}:${String(opcode)}/${order}`;
        try {
            const target = new Processor({ processorOptions: { channelCount: 2, mixerMode: 0x202, interactiveVersion: 2, wasmModule: new WebAssembly.Module(wasmBytes) } });
            const targetPort = lastPort;
            const identity = { session: (1n << 59n) + 91n, build: (1n << 62n) + 97n };
            const left = new Float32Array(128), right = new Float32Array(128);
            const pump = () => { for (let index = 0; index < 4; index++) target.process([], [[left, right]]); };
            targetPort.dispatch({ type: 'loadModule', requestId: 900, bytes: syntheticMod(), identity }); pump();
            assert.ok(targetPort.messages.find(message => message.type === 'moduleLoaded')?.accepted);
            const malformed = { opcode, argument: 0, extra: 0, identity };
            const valid = { opcode: Ring.OPCODE_NOTE_OVERLAP, argument: 1, extra: 0, identity };
            const records = order === 'alone' ? [malformed] : order === 'before' ? [malformed, valid] : [valid, malformed];
            const count = targetPort.messages.filter(message => message.type === 'interactiveApplied').length;
            let exception = null;
            try { targetPort.dispatch({ type: 'commandBatch', version: 2, commands: records }); } catch (error) { exception = String(error); }
            pump();
            const replies = targetPort.messages.filter(message => message.type === 'interactiveApplied').slice(count);
            const state = target.wasm.replacement_state();
            console.log('NO08-A1 actual fallback', JSON.stringify({ label, exception, acknowledgements: replies.length, accepted: replies[0]?.accepted, transport: String(state[4]), overlap: String(state[6]) }));
            assert.equal(exception, null, 'malformed opcode must not throw at WASM admission');
            assert.equal(targetPort.messages.some(message => message.type === 'fault'), false);
            assert.equal(replies.length, 1, 'exactly one explicit group rejection');
            assert.equal(replies[0].accepted, false);
            assert.deepEqual([...replies[0].words.slice(2, 6)], [Number(identity.session & 0xffffffffn), Number(identity.session >> 32n), Number(identity.build & 0xffffffffn), Number(identity.build >> 32n)]);
            assert.equal(state[4], 0n, 'malformed opcode must not be coerced to Pause');
            assert.equal(state[6], 0n, 'valid member must not partially change overlap');
            targetPort.dispatch({ type: 'commandBatch', version: 2, commands: [valid] }); pump();
            assert.equal(targetPort.messages.filter(message => message.type === 'interactiveApplied').length, count + 2);
            assert.equal(targetPort.messages.findLast(message => message.type === 'interactiveApplied').accepted, true);
            assert.equal(target.wasm.replacement_state()[6], 1n, 'valid retry applies');
        } catch (error) { malformedOpcodeFailures.push(`${label}: ${error.message}`); }
    }
}
assert.deepEqual(malformedOpcodeFailures, [], 'all malformed fallback opcode types/orders reject atomically');
console.log('NO08-A1 malformed fallback opcode types reject once with current identity, no partial state, and valid retry');
