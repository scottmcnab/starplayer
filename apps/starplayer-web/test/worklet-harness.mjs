import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const root = new URL('../dist/', import.meta.url);
const bundle = await readFile(new URL('starplayer-worklet.js', root), 'utf8').catch(() => {
    throw new Error('apps/starplayer-web/dist is missing; run `cargo xtask wasm` first');
});
const wasmBytes = await readFile(new URL('starplayer_host_wasm_bg.wasm', root));
const firstFixture = await readFile(new URL('modules/REFLEX.S3M', root));
const secondFixture = await readFile(new URL('modules/MOVEMENT.S3M', root));

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
    postMessage(message) { this.messages.push(message); }
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

globalThis.TextDecoder = nodeTextDecoder;
console.log(`worklet harness: 10 s of real S3M render (peak ${loudest.toFixed(3)}), independent overlapping processors, MOD panning, transport, seek, memory, garbage, scope taps on both transports and bad-file rejection passed`);
