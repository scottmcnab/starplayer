// A Node harness for the A4-N1 development Web Receiver, in the shape of
// `apps/starplayer-web/test/worklet-harness.mjs`: no browser. It stubs the CAF receiver
// SDK, loads the real wasm-bindgen glue the way a browser's `<script>` tag would (a
// classic-script evaluation whose top-level `let wasm_bindgen = ...` joins the shared
// global lexical environment `receiver.js` reads from as a free variable — see the long
// comment at the top of `receiver.js`), and dispatches the same custom messages a sender
// would.
//
// What it does not exercise: `AudioContext`/`AudioWorklet` do not exist in Node, so the
// `audioContext`/`audioWorklet` probes and the worklet half of `bench` all take their
// documented "not supported, no throw" path here — proving exactly the degrade-rather-
// than-throw behaviour the report format exists for. A real browser is required for the
// rest, which is why A4-N1 is verified by the owner on real hardware, not by this file.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const root = new URL('../dist/', import.meta.url);

const glueSource = await readFile(new URL('starplayer_host_wasm.js', root), 'utf8').catch(() => {
    throw new Error('apps/starplayer-cast-probe/dist is missing; run `cargo xtask cast-probe` first');
});
const wasmBytes = await readFile(new URL('starplayer_host_wasm_bg.wasm', root));

// `receiver.js`'s page-thread bench calls `wasm_bindgen({ module_or_path:
// 'starplayer_host_wasm_bg.wasm' })`, which the glue turns into `fetch(module_or_path)`.
// That is a bare relative string with no document to resolve it against, so this stub
// resolves the two filenames the glue and `receiver.js` ever ask for directly off disk —
// everything else is a harness bug, not a receiver one, and fails loudly.
globalThis.fetch = async (input) => {
    const url = String(input);
    if (url.endsWith('starplayer_host_wasm_bg.wasm')) {
        return new Response(wasmBytes, { status: 200, headers: { 'Content-Type': 'application/wasm' } });
    }
    if (url.endsWith('starplayer_host_wasm.js')) {
        return new Response(glueSource, { status: 200, headers: { 'Content-Type': 'text/javascript' } });
    }
    throw new Error(`probe-harness: unexpected fetch(${url})`);
};

// ── stub the CAF receiver SDK ───────────────────────────────────────────────────────
const NAMESPACE = 'urn:x-cast:com.starplayer.probe';
let listener = null;
let startOptions = null;
const sentMessages = [];
globalThis.cast = {
    framework: {
        CastReceiverContext: {
            getInstance() {
                return {
                    start(options) {
                        startOptions = options;
                    },
                    addCustomMessageListener(namespace, callback) {
                        assert.equal(namespace, NAMESPACE, 'the receiver listens on its own namespace');
                        listener = callback;
                    },
                    sendCustomMessage(namespace, senderId, data) {
                        assert.equal(namespace, NAMESPACE);
                        sentMessages.push({ senderId, data });
                    },
                };
            },
        },
    },
};

// The real glue, evaluated the way a classic `<script>` tag would evaluate it: as a
// top-level script, not a module, so `let wasm_bindgen = ...` lands in the realm's shared
// global lexical environment rather than a module's own isolated scope.
vm.runInThisContext(glueSource, { filename: 'starplayer_host_wasm.js' });

const { init, NAMESPACE: exportedNamespace } = await import(new URL('receiver.js', root));
assert.equal(exportedNamespace, NAMESPACE);
init();
assert.ok(listener, 'init() registered a custom message listener');
assert.deepEqual(startOptions, { disableIdleTimeout: true });

function dispatch(message, senderId = 'sender-1') {
    sentMessages.length = 0;
    listener({ senderId, data: message });
    // Every reply here is synchronous-enough that a microtask flush is all dispatch needs;
    // `sendCustomMessage` itself is a synchronous stub. Callers that need to wait on an
    // async reply (`bench`, `chunk`) use `waitForMessage` below instead.
}

async function waitForMessage(predicate, { timeoutMs = 20_000 } = {}) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        const found = sentMessages.find((entry) => predicate(entry.data));
        if (found) return found.data;
        if (Date.now() > deadline) throw new Error('probe-harness: timed out waiting for a reply');
        await new Promise((resolve) => setTimeout(resolve, 5));
    }
}

// ── 1. `probe` degrades rather than throws ──────────────────────────────────────────
dispatch({ type: 'probe' });
const report = await waitForMessage((data) => data.type === 'report');

assert.equal(typeof report.userAgent, 'string');
assert.equal(typeof report.wasm, 'object');
assert.equal(typeof report.wasm.supported, 'boolean');
assert.equal(typeof report.wasm.instantiated, 'boolean');
assert.ok(report.wasm.supported, 'Node has WebAssembly');
assert.ok(report.wasm.instantiated, 'the 1-function probe module instantiated');
assert.equal(report.wasm.returnedValue, 42, 'the hand-written module returns 42');

assert.equal(typeof report.audioContext, 'object');
assert.equal(report.audioContext.supported, false, 'Node has no AudioContext');
assert.equal(report.audioContext.sampleRate, null);
assert.equal(report.audioContext.state, null);
assert.equal(report.audioContext.baseLatency, null);

assert.equal(typeof report.audioWorklet, 'object');
assert.equal(report.audioWorklet.supported, false, 'Node has no AudioWorkletNode');
assert.equal(report.audioWorklet.registered, false);
assert.equal(report.audioWorklet.toneHeard, false);
assert.equal(report.audioWorklet.error, null, 'unsupported is reported, not thrown');

assert.equal(typeof report.scriptProcessor, 'object');
assert.equal(report.scriptProcessor.supported, false);

assert.equal(typeof report.sharedArrayBuffer, 'object');
assert.equal(report.sharedArrayBuffer.supported, typeof SharedArrayBuffer !== 'undefined');

assert.equal(report.crossOriginIsolated, false, 'Node has no `window`');
assert.equal(report.memory, null, 'Node has no `performance.memory`');
assert.equal(typeof report.hardwareConcurrency, 'number');
assert.equal(typeof report.serviceWorker, 'object');
assert.equal(typeof report.serviceWorker.supported, 'boolean');
assert.equal(typeof report.serviceWorker.controller, 'boolean');
assert.equal(report.maxMessageBytes, null, 'no chunk has arrived yet');
console.log('ok: `probe` reports every field, fully degraded, on a runtime with no audio stack');

// ── 2. a chunked transfer reassembles byte-for-byte ─────────────────────────────────
const { syntheticMod } = await import(new URL('synthetic-mod.mjs', root));
const moduleBytes = syntheticMod();
const chunkCount = 4;
const chunkSize = Math.ceil(moduleBytes.length / chunkCount);

function toBase64(bytes) {
    return Buffer.from(bytes).toString('base64');
}

let lastAck = null;
let transferReport = null;
for (let index = 0; index < chunkCount; index += 1) {
    const start = index * chunkSize;
    const slice = moduleBytes.subarray(start, Math.min(start + chunkSize, moduleBytes.length));
    dispatch({ type: 'chunk', index, total: chunkCount, size: slice.length, data: toBase64(slice) });
    lastAck = await waitForMessage((data) => data.type === 'chunk-ack' && data.index === index);
    assert.equal(lastAck.received, index + 1);
    assert.equal(typeof lastAck.elapsedMs, 'number');
    assert.ok(lastAck.elapsedMs >= 0);
}
transferReport = await waitForMessage((data) => data.type === 'report' && data.transfer !== undefined);
assert.equal(transferReport.transfer.bytes, moduleBytes.length);
assert.equal(transferReport.transfer.chunkSize, chunkSize);
assert.ok(transferReport.transfer.elapsedMs >= 0);
assert.ok(Number.isFinite(transferReport.transfer.bytesPerSecond));
assert.equal(transferReport.maxMessageBytes, chunkSize, 'the largest chunk seen so far is reported');

// A second, immediate `probe` proves the reassembled module bytes are held for `bench`
// (verified indirectly below) and that `maxMessageBytes` survives across a `probe`.
dispatch({ type: 'probe' });
const secondReport = await waitForMessage((data) => data.type === 'report' && data.transfer === undefined);
assert.equal(secondReport.maxMessageBytes, chunkSize);
console.log(`ok: a ${chunkCount}-chunk transfer of the synthetic module (${moduleBytes.length} bytes) reassembles and reports`);

// ── 3. `bench` runs the real wasm host and produces sane timings ───────────────────
dispatch({ type: 'bench', quanta: 8 });
const benchReport = await waitForMessage((data) => data.type === 'bench-report');
assert.equal(benchReport.quanta, 8);
assert.equal(benchReport.frames, 128, 'RENDER_QUANTUM is 128 frames');
assert.equal(benchReport.module, 'uploaded', 'the chunk transfer above replaced the synthetic default');
for (const key of ['min', 'average', 'max', 'p95']) {
    assert.ok(Number.isFinite(benchReport.microsecondsPerQuantum[key]), `microsecondsPerQuantum.${key} is finite`);
    assert.ok(benchReport.microsecondsPerQuantum[key] >= 0, `microsecondsPerQuantum.${key} is non-negative`);
}
assert.ok(benchReport.microsecondsPerQuantum.min <= benchReport.microsecondsPerQuantum.average, 'min <= average');
assert.ok(benchReport.microsecondsPerQuantum.average <= benchReport.microsecondsPerQuantum.max, 'average <= max');
assert.ok(Number.isFinite(benchReport.budgetMicroseconds) && benchReport.budgetMicroseconds > 0);
assert.ok(Number.isFinite(benchReport.realTimeRatio) && benchReport.realTimeRatio > 0);
assert.equal(benchReport.worklet, undefined, 'no AudioWorklet in Node, so no worklet sub-report');
console.log(`ok: bench ran 8 real quanta — average ${benchReport.microsecondsPerQuantum.average.toFixed(1)}us against a ${benchReport.budgetMicroseconds.toFixed(1)}us budget`);

// ── 4. a malformed chunk answers `error`, never a throw ─────────────────────────────
dispatch({ type: 'chunk', index: 0, total: 2, size: 4, data: 'not valid base64 !!' });
const errorReply = await waitForMessage((data) => data.type === 'error');
assert.equal(errorReply.stage, 'chunk');
assert.equal(typeof errorReply.message, 'string');
assert.ok(errorReply.message.length > 0);
console.log('ok: a malformed `chunk` message answers `error` rather than throwing');

console.log('probe-harness: all checks passed');
