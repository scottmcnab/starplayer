// The development Web Receiver's logic (A4-N1). This is not N3: it renders no music the
// owner would want to hear. Its only job is to answer, honestly and without throwing, what
// the speaker's runtime can do — `WebAssembly`, `AudioContext`, `AudioWorklet`,
// `SharedArrayBuffer`, memory — and how fast the wasm host runs a bench module there.
//
// Loaded as an ES module (`<script type="module" src="receiver.js">`), so it shares the
// page's top-level scope with two plain `<script>` tags loaded ahead of it in
// `receiver.html`: the CAF receiver SDK (defines `cast`) and the wasm-bindgen no-modules
// glue for `starplayer-host-wasm` (defines `wasm_bindgen`). Both are declared with `let` at
// their own top level, which — per the ECMAScript global environment record — is visible
// to this module as a free variable, the same way it would be visible to another classic
// script; it is not a `window` property. `apps/starplayer-cast-probe/test/probe-harness.mjs`
// relies on exactly this: it `vm.runInThisContext`s the real glue, stubs `cast`, and then
// dynamically imports this file, all in one Node realm.
//
// Every code path here is wrapped so a throw becomes an `error` reply, never a silent
// failure: an unanswered probe looks exactly like a speaker that cannot run the page at
// all, which is the one outcome this task must not produce.

import { syntheticMod } from './synthetic-mod.mjs';
import { WORKLET_TEXT_DECODER_POLYFILL } from './worklet-prelude-source.mjs';

export const NAMESPACE = 'urn:x-cast:com.starplayer.probe';

const BENCH_SAMPLE_RATE = 44_100;
const DEFAULT_MIXER_MODE_WIRE = 0x0000_0202; // float path, linear interpolation, stereo
const HOST_WASM_GLUE_PATH = 'starplayer_host_wasm.js';
const HOST_WASM_MODULE_PATH = 'starplayer_host_wasm_bg.wasm';
const TONE_PROCESSOR_NAME = 'starplayer-probe-tone';
const BENCH_PROCESSOR_NAME = 'starplayer-probe-bench';
const TONE_SECONDS = 2;

// A hand-written, one-function WebAssembly module: `(func (export "test") (result i32)
// (i32.const 42))`. Deliberately independent of the real host wasm — this only answers
// "does `WebAssembly.instantiate` work at all, and does the result run?" — so a build
// that never reaches `cargo xtask cast-probe` still reports something.
const PROBE_WASM_BYTES = new Uint8Array([
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic, version 1
    0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f, // type section: () -> i32
    0x03, 0x02, 0x01, 0x00, // function section: one function, type 0
    0x07, 0x08, 0x01, 0x04, 0x74, 0x65, 0x73, 0x74, 0x00, 0x00, // export "test" func 0
    0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x2a, 0x0b, // code: i32.const 42; end
]);

class ProbeError extends Error {
    constructor(stage, message) {
        super(message);
        this.stage = stage;
    }
}

// ── report state ────────────────────────────────────────────────────────────────────

let transferState = null; // { total, chunkSize, chunks: Array<Uint8Array | null>, startedAt }
let uploadedModuleBytes = null;
let maxMessageBytesSeen = null;
let lastAudioWorkletProbe = null; // cached so `bench` knows whether a worklet is available
let hostWasmReadyPromise = null;
let compiledHostModulePromise = null;
let workletBenchModulePromise = null;

function now() {
    // `performance.now()` exists on the page, in Node (the harness), and in
    // `AudioWorkletGlobalScope` — this is the one clock every realm this file runs in
    // actually has.
    return typeof performance !== 'undefined' ? performance.now() : Date.now();
}

function setStatus(text) {
    if (typeof document === 'undefined') return;
    const pre = document.getElementById('status');
    if (pre !== null) pre.textContent = text;
}

// ── capability probes ───────────────────────────────────────────────────────────────

async function probeWasm() {
    if (typeof WebAssembly === 'undefined') {
        return { supported: false, instantiated: false, error: null, returnedValue: null };
    }
    try {
        const { instance } = await WebAssembly.instantiate(PROBE_WASM_BYTES);
        const returnedValue = instance.exports.test();
        return { supported: true, instantiated: true, error: null, returnedValue };
    } catch (error) {
        return { supported: true, instantiated: false, error: String((error && error.message) || error), returnedValue: null };
    }
}

function probeAudioContext() {
    const Ctor = typeof AudioContext !== 'undefined' ? AudioContext : (typeof webkitAudioContext !== 'undefined' ? webkitAudioContext : null);
    if (Ctor === null) {
        return { supported: false, sampleRate: null, state: null, baseLatency: null };
    }
    let context = null;
    try {
        context = new Ctor();
        const report = { supported: true, sampleRate: context.sampleRate, state: context.state, baseLatency: context.baseLatency ?? null };
        context.close().catch(() => {});
        return report;
    } catch (error) {
        if (context !== null) context.close().catch(() => {});
        return { supported: false, sampleRate: null, state: null, baseLatency: null };
    }
}

// The tone processor is registered from a `Blob` URL — `AudioWorkletGlobalScope` cannot
// `fetch()` or `importScripts()`, so this is the only script it will ever see, and it must
// be self-contained. It plays an audible 440 Hz tone for `TONE_SECONDS`, at a deliberately
// low gain, and posts back the moment its own `process()` is first called — proof the
// worklet's realtime callback actually ran, which is the fact a Cast audio device might
// not give us at all.
const TONE_PROCESSOR_SOURCE = `
'use strict';
class StarPlayerProbeToneProcessor extends AudioWorkletProcessor {
    constructor() {
        super();
        this.phase = 0;
        this.reported = false;
    }
    process(inputs, outputs) {
        const data = outputs[0][0];
        if (data !== undefined) {
            const step = (2 * Math.PI * 440) / sampleRate;
            for (let frame = 0; frame < data.length; frame += 1) {
                data[frame] = 0.15 * Math.sin(this.phase);
                this.phase += step;
            }
        }
        if (!this.reported) {
            this.reported = true;
            this.port.postMessage({ type: 'processed' });
        }
        return true;
    }
}
registerProcessor('${TONE_PROCESSOR_NAME}', StarPlayerProbeToneProcessor);
`;

async function probeAudioWorklet() {
    if (typeof AudioContext === 'undefined' || typeof AudioWorkletNode === 'undefined') {
        return { supported: false, registered: false, toneHeard: false, error: null };
    }
    let context = null;
    let blobUrl = null;
    try {
        context = new AudioContext();
        blobUrl = URL.createObjectURL(new Blob([TONE_PROCESSOR_SOURCE], { type: 'text/javascript' }));
        await context.audioWorklet.addModule(blobUrl);
        const node = new AudioWorkletNode(context, TONE_PROCESSOR_NAME, { numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [1] });
        const toneHeard = await new Promise((resolve) => {
            let settled = false;
            node.port.onmessage = (event) => {
                if (!settled && event.data && event.data.type === 'processed') {
                    settled = true;
                    resolve(true);
                }
            };
            node.connect(context.destination);
            setTimeout(() => {
                if (!settled) {
                    settled = true;
                    resolve(false);
                }
            }, TONE_SECONDS * 1000);
        });
        node.disconnect();
        await context.close().catch(() => {});
        return { supported: true, registered: true, toneHeard, error: null };
    } catch (error) {
        if (context !== null) context.close().catch(() => {});
        return { supported: true, registered: false, toneHeard: false, error: String((error && error.message) || error) };
    } finally {
        if (blobUrl !== null) URL.revokeObjectURL(blobUrl);
    }
}

function buildReport(extra) {
    return probeWasm().then((wasm) => probeAudioWorklet().then((audioWorklet) => {
        lastAudioWorkletProbe = audioWorklet;
        return {
            type: 'report',
            userAgent: typeof navigator !== 'undefined' ? navigator.userAgent : null,
            wasm,
            audioContext: probeAudioContext(),
            audioWorklet,
            scriptProcessor: { supported: typeof AudioContext !== 'undefined' && typeof AudioContext.prototype.createScriptProcessor === 'function' },
            sharedArrayBuffer: { supported: typeof SharedArrayBuffer !== 'undefined' },
            crossOriginIsolated: typeof window !== 'undefined' ? window.crossOriginIsolated === true : false,
            memory: typeof performance !== 'undefined' && performance.memory
                ? { jsHeapSizeLimit: performance.memory.jsHeapSizeLimit, totalJSHeapSize: performance.memory.totalJSHeapSize, usedJSHeapSize: performance.memory.usedJSHeapSize }
                : null,
            hardwareConcurrency: typeof navigator !== 'undefined' ? (navigator.hardwareConcurrency ?? null) : null,
            serviceWorker: {
                supported: typeof navigator !== 'undefined' && 'serviceWorker' in navigator,
                controller: typeof navigator !== 'undefined' && navigator.serviceWorker ? navigator.serviceWorker.controller !== null : false,
            },
            maxMessageBytes: maxMessageBytesSeen,
            ...extra,
        };
    }));
}

// ── chunk transfer ──────────────────────────────────────────────────────────────────

function base64ToBytes(base64) {
    if (typeof atob === 'function') {
        const binary = atob(base64);
        const bytes = new Uint8Array(binary.length);
        for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
        return bytes;
    }
    // Node has no `atob`; the harness runs here, never a browser.
    return new Uint8Array(Buffer.from(base64, 'base64'));
}

async function handleChunk(message, reply) {
    const { index, total, size, data } = message;
    if (!Number.isInteger(total) || total <= 0 || !Number.isInteger(index) || index < 0 || index >= total || typeof data !== 'string') {
        throw new ProbeError('chunk', `malformed chunk message: index=${index}, total=${total}, data is ${typeof data}`);
    }
    let bytes;
    try {
        bytes = base64ToBytes(data);
    } catch (error) {
        throw new ProbeError('chunk', `chunk ${index} did not decode as base64: ${(error && error.message) || error}`);
    }
    if (typeof size === 'number' && bytes.length !== size) {
        throw new ProbeError('chunk', `chunk ${index} declared ${size} bytes but decoded to ${bytes.length}`);
    }
    if (bytes.length > (maxMessageBytesSeen ?? 0)) maxMessageBytesSeen = bytes.length;

    if (transferState === null || transferState.total !== total) {
        transferState = { total, chunkSize: bytes.length, chunks: new Array(total).fill(null), startedAt: now() };
    }
    transferState.chunks[index] = bytes;

    const elapsedMs = now() - transferState.startedAt;
    reply({ type: 'chunk-ack', index, received: transferState.chunks.filter((chunk) => chunk !== null).length, elapsedMs });

    if (index !== total - 1) return;

    const missing = transferState.chunks.findIndex((chunk) => chunk === null);
    if (missing !== -1) {
        const failedTransfer = transferState;
        transferState = null;
        throw new ProbeError('chunk', `transfer finished at the last index but chunk ${missing} of ${failedTransfer.total} never arrived`);
    }

    const totalBytes = transferState.chunks.reduce((sum, chunk) => sum + chunk.length, 0);
    const merged = new Uint8Array(totalBytes);
    let offset = 0;
    for (const chunk of transferState.chunks) {
        merged.set(chunk, offset);
        offset += chunk.length;
    }
    uploadedModuleBytes = merged;
    const totalElapsedMs = now() - transferState.startedAt;
    const chunkSize = transferState.chunkSize;
    transferState = null;

    const report = await buildReport({
        transfer: {
            bytes: totalBytes,
            elapsedMs: totalElapsedMs,
            bytesPerSecond: totalElapsedMs > 0 ? totalBytes / (totalElapsedMs / 1000) : totalBytes,
            chunkSize,
        },
    });
    reply(report);
}

// ── bench ────────────────────────────────────────────────────────────────────────────

function summarize(durationsMicroseconds) {
    const sorted = [...durationsMicroseconds].sort((a, b) => a - b);
    const sum = sorted.reduce((total, value) => total + value, 0);
    const percentileIndex = Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95));
    return { min: sorted[0], average: sum / sorted.length, max: sorted[sorted.length - 1], p95: sorted[percentileIndex] };
}

async function ensureHostWasmLoaded() {
    if (hostWasmReadyPromise === null) {
        hostWasmReadyPromise = (async () => {
            if (typeof wasm_bindgen !== 'function') {
                throw new ProbeError('bench', `${HOST_WASM_GLUE_PATH} did not define the wasm_bindgen loader — is it loaded ahead of receiver.js in receiver.html?`);
            }
            await wasm_bindgen({ module_or_path: HOST_WASM_MODULE_PATH });
        })();
    }
    return hostWasmReadyPromise;
}

/** Compile the host wasm module once, for reuse by `runWorkletBench`'s own instance. */
async function ensureHostModuleCompiled() {
    if (compiledHostModulePromise === null) {
        compiledHostModulePromise = (async () => {
            const response = await fetch(HOST_WASM_MODULE_PATH);
            if (!response.ok) throw new ProbeError('bench', `could not fetch ${HOST_WASM_MODULE_PATH}: ${response.status} ${response.statusText}`);
            const bytes = await response.arrayBuffer();
            return WebAssembly.compile(bytes);
        })();
    }
    return compiledHostModulePromise;
}

async function runPageThreadBench(quanta, moduleBytes) {
    await ensureHostWasmLoaded();
    if (!wasm_bindgen.init(BENCH_SAMPLE_RATE, DEFAULT_MIXER_MODE_WIRE)) {
        throw new ProbeError('bench', 'the wasm host refused to initialize (no engine arm for the default mixer mode)');
    }
    wasm_bindgen.load_module_with_options(moduleBytes, false, 0);
    const renderQuantum = wasm_bindgen.render_quantum();
    const budgetMicroseconds = (renderQuantum / BENCH_SAMPLE_RATE) * 1_000_000;
    const durations = new Array(quanta);
    for (let index = 0; index < quanta; index += 1) {
        const started = now();
        wasm_bindgen.process(renderQuantum);
        durations[index] = (now() - started) * 1000;
    }
    return { frames: renderQuantum, budgetMicroseconds, microsecondsPerQuantum: summarize(durations) };
}

// The worklet-thread half of the bench. `AudioWorkletGlobalScope` cannot `fetch()` its own
// dependencies once running, so the script it loads is assembled here, on the page, from
// three pieces already on this page: the `TextDecoder` polyfill (the glue constructs one,
// unconditionally, the moment it is evaluated — this realm has none), the actual
// `starplayer_host_wasm.js` glue fetched fresh, and a small processor class appended after
// it. Concatenated into one script and loaded with one `addModule()`, all three share the
// worklet's top-level scope, exactly as `xtask wasm`'s `build_worklet_bundle` arranges for
// the real web player — this just does it at runtime instead of at build time, since the
// probe is not allowed to depend on that build step or its output directory.
//
// Everything the processor needs — the compiled `WebAssembly.Module`, the module bytes to
// load, the mixer mode and the quanta count — travels in `processorOptions`, at node
// construction, and the whole bench runs synchronously in the constructor. This is not a
// style choice: a compiled `WebAssembly.Module` structured-clones correctly into
// `AudioWorkletGlobalScope` as part of `AudioWorkletNodeOptions` (the same path the real
// web player's own worklet uses, in `apps/starplayer-web/www/worklet-processor.js`), but a
// `WebAssembly.Module` handed to `node.port.postMessage()` **after** construction — a
// second, independent structured clone through the same message port — silently never
// arrives in this build of Chromium; neither the sender nor the receiver observes an
// error, so the request just hangs forever. Confirmed by hand against a real
// `AudioWorkletNode` before shipping this: swap the two and the worklet's `onmessage`
// simply never fires.
//
// `performance` is also, confirmed by hand the same way, `undefined` inside
// `AudioWorkletGlobalScope` in this build of Chromium (151) — only `Date`, `currentTime`
// (the audio clock, which only advances once per real render quantum pulled from the
// graph, not while this constructor blocks synchronously, so it cannot time this loop at
// all) and `currentFrame` are there. `Date.now()` is millisecond-grained, far coarser than
// a single `process(128)` call, which the page-thread bench above typically measures in
// tens of microseconds — timing every call individually would read as zero almost every
// time. So this times **batches** of `BATCH_SIZE` calls and records the batch average
// against each call in the batch; `microsecondsPerQuantum` is therefore a coarser,
// batch-smoothed estimate on the worklet thread than the page-thread bench's per-call
// figure, and `receiver.js`'s doc comment on `runWorkletBench` says so.
const BENCH_PROCESSOR_SOURCE = `
'use strict';
class StarPlayerProbeBenchProcessor extends AudioWorkletProcessor {
    constructor(options) {
        super();
        const settings = options.processorOptions;
        const BATCH_SIZE = 32;
        const clock = typeof performance !== 'undefined'
            ? () => performance.now()
            : (typeof Date !== 'undefined' ? () => Date.now() : () => currentTime * 1000);
        try {
            wasm_bindgen.initSync({ module: settings.hostModule });
            if (!wasm_bindgen.init(sampleRate, settings.mixerModeWire)) {
                throw new Error('the worklet wasm host refused to initialize');
            }
            wasm_bindgen.load_module_with_options(settings.moduleBytes, false, 0);
            const renderQuantum = wasm_bindgen.render_quantum();
            const budgetMicroseconds = (renderQuantum / sampleRate) * 1_000_000;
            const durations = [];
            let underruns = 0;
            let remaining = settings.quanta;
            while (remaining > 0) {
                const batch = Math.min(BATCH_SIZE, remaining);
                const started = clock();
                for (let index = 0; index < batch; index += 1) wasm_bindgen.process(renderQuantum);
                const perCallMicroseconds = ((clock() - started) * 1000) / batch;
                for (let index = 0; index < batch; index += 1) durations.push(perCallMicroseconds);
                if (perCallMicroseconds > budgetMicroseconds) underruns += batch;
                remaining -= batch;
            }
            this.port.postMessage({ type: 'bench-report', quanta: settings.quanta, underruns, durations });
        } catch (error) {
            this.port.postMessage({ type: 'bench-error', message: String((error && error.message) || error) });
        }
    }
    process() {
        return true;
    }
}
registerProcessor('${BENCH_PROCESSOR_NAME}', StarPlayerProbeBenchProcessor);
`;

async function ensureWorkletBenchRegistered(context) {
    if (workletBenchModulePromise === null) {
        workletBenchModulePromise = (async () => {
            const response = await fetch(HOST_WASM_GLUE_PATH);
            if (!response.ok) throw new ProbeError('bench', `could not fetch ${HOST_WASM_GLUE_PATH}: ${response.status} ${response.statusText}`);
            const glueSource = await response.text();
            const bundle = `${WORKLET_TEXT_DECODER_POLYFILL}\n\n${glueSource}\n\n${BENCH_PROCESSOR_SOURCE}`;
            const blobUrl = URL.createObjectURL(new Blob([bundle], { type: 'text/javascript' }));
            try {
                await context.audioWorklet.addModule(blobUrl);
            } finally {
                URL.revokeObjectURL(blobUrl);
            }
        })();
    }
    return workletBenchModulePromise;
}

/**
 * `microsecondsPerQuantum` here is coarser than the page-thread bench's own figure:
 * `AudioWorkletGlobalScope` has no `performance.now()` (confirmed against a real
 * `AudioWorkletNode`, not assumed — see `BENCH_PROCESSOR_SOURCE`'s doc comment), only
 * millisecond-grained `Date.now()`, so the processor times batches of calls and reports
 * the batch average against every call in the batch rather than one real per-call sample.
 * Still enough to answer the question this exists for: whether the worklet thread clears
 * the render-quantum budget.
 */
async function runWorkletBench(quanta, moduleBytes) {
    const hostModule = await ensureHostModuleCompiled();
    const context = new AudioContext();
    try {
        await ensureWorkletBenchRegistered(context);
        const result = await new Promise((resolve, reject) => {
            const node = new AudioWorkletNode(context, BENCH_PROCESSOR_NAME, {
                numberOfInputs: 0,
                numberOfOutputs: 1,
                outputChannelCount: [1],
                processorOptions: { hostModule, moduleBytes, mixerModeWire: DEFAULT_MIXER_MODE_WIRE, quanta },
            });
            node.port.onmessage = (event) => {
                if (event.data.type === 'bench-report') resolve(event.data);
                else if (event.data.type === 'bench-error') reject(new ProbeError('bench', event.data.message));
            };
            node.connect(context.destination);
        });
        return { quanta: result.quanta, underruns: result.underruns, microsecondsPerQuantum: summarize(result.durations) };
    } finally {
        await context.close().catch(() => {});
    }
}

async function runBench(quanta) {
    const moduleBytes = uploadedModuleBytes ?? syntheticMod();
    const pageThread = await runPageThreadBench(quanta, moduleBytes);
    const report = {
        type: 'bench-report',
        quanta,
        frames: pageThread.frames,
        microsecondsPerQuantum: pageThread.microsecondsPerQuantum,
        budgetMicroseconds: pageThread.budgetMicroseconds,
        realTimeRatio: pageThread.microsecondsPerQuantum.average / pageThread.budgetMicroseconds,
        module: uploadedModuleBytes !== null ? 'uploaded' : 'synthetic',
    };
    if (lastAudioWorkletProbe !== null && lastAudioWorkletProbe.registered && typeof AudioContext !== 'undefined') {
        try {
            report.worklet = await runWorkletBench(quanta, moduleBytes);
        } catch (error) {
            report.worklet = { error: String((error && error.message) || error) };
        }
    }
    return report;
}

// ── dispatch ─────────────────────────────────────────────────────────────────────────

async function dispatch(rawMessage, reply) {
    // The Cast custom-message channel is documented to carry either a string or a plain
    // object depending on SDK version and what the sender handed it; accept both rather
    // than betting on one.
    let message = rawMessage;
    if (typeof message === 'string') {
        try {
            message = JSON.parse(message);
        } catch (error) {
            throw new ProbeError('dispatch', `message was a string but not valid JSON: ${(error && error.message) || error}`);
        }
    }
    if (message === null || typeof message !== 'object' || typeof message.type !== 'string') {
        throw new ProbeError('dispatch', 'message has no string `type` field');
    }
    switch (message.type) {
        case 'probe': {
            reply(await buildReport({}));
            return;
        }
        case 'chunk': {
            await handleChunk(message, reply);
            return;
        }
        case 'bench': {
            const quanta = Number.isInteger(message.quanta) && message.quanta > 0 ? message.quanta : 512;
            reply(await runBench(quanta));
            return;
        }
        default:
            throw new ProbeError('dispatch', `unknown message type \`${message.type}\``);
    }
}

function wireListener(context) {
    context.addCustomMessageListener(NAMESPACE, (event) => {
        const senderId = event.senderId;
        const reply = (payload) => {
            setStatus(JSON.stringify(payload, null, 2));
            context.sendCustomMessage(NAMESPACE, senderId, payload);
        };
        Promise.resolve()
            .then(() => dispatch(event.data, reply))
            .catch((error) => {
                reply({ type: 'error', stage: error instanceof ProbeError ? error.stage : 'dispatch', message: String((error && error.message) || error) });
            });
    });
}

export function init() {
    const context = cast.framework.CastReceiverContext.getInstance();
    wireListener(context);
    context.start({ disableIdleTimeout: true });
    return context;
}

if (typeof document !== 'undefined') {
    try {
        init();
        setStatus('StarPlayer cast probe: waiting for a sender…');
    } catch (error) {
        setStatus(`StarPlayer cast probe failed to start: ${(error && error.message) || error}`);
    }
}
