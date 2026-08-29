// Main-thread half of the M0-A4 AudioWorklet spike.
//
// Plain script, no framework, no bundler. It does four things: compile the wasm and hand
// it to the worklet, own the command transport, read the telemetry transport, and report
// honestly which of the two transports it ended up on. In M1-task-B7 this file grows
// into the real player; the shape of the plumbing below is meant to survive that.

'use strict';

(function () {
    const Ring = globalThis.StarPlayerRing;

    const WASM_URL = 'starplayer_host_wasm_bg.wasm';
    const WORKLET_URL = 'starplayer-worklet.js';
    const PROCESSOR_NAME = 'starplayer-sine';
    const CHANNEL_COUNT = 2;

    /** How long the memory-growth watch runs before it reports a verdict. */
    const MEMORY_WATCH_SECONDS = 60;

    const elements = {
        start: document.getElementById('start'),
        stop: document.getElementById('stop'),
        frequency: document.getElementById('frequency'),
        frequencyReadout: document.getElementById('frequency-readout'),
        forceFallback: document.getElementById('force-fallback'),
        isolation: document.getElementById('isolation'),
        commandTransport: document.getElementById('command-transport'),
        telemetryTransport: document.getElementById('telemetry-transport'),
        quantum: document.getElementById('quantum'),
        peakBar: document.getElementById('peak-bar'),
        peakReadout: document.getElementById('peak-readout'),
        actualFrequency: document.getElementById('actual-frequency'),
        memory: document.getElementById('memory'),
        memoryVerdict: document.getElementById('memory-verdict'),
        quanta: document.getElementById('quanta'),
        dropped: document.getElementById('dropped'),
        backPressure: document.getElementById('back-pressure'),
        log: document.getElementById('log'),
    };

    const state = {
        context: null,
        node: null,
        commandRing: null,
        telemetry: null,
        commandTransport: 'not started',
        telemetryTransport: 'not started',
        // Only used on the postMessage command path: the newest slider value, posted at
        // most once per animation frame rather than once per `input` event.
        pendingFrequency: null,
        lastPostedFrequency: null,
        ringBackPressureEvents: 0,
        latest: null,
        initialMemoryBytes: null,
        startedAt: null,
        memoryVerdictReported: false,
    };

    function log(message) {
        const line = document.createElement('div');
        const stamp = state.startedAt === null ? '  --  ' : `${((performance.now() - state.startedAt) / 1000).toFixed(1)}s`;
        line.textContent = `[${stamp}] ${message}`;
        elements.log.prepend(line);
        while (elements.log.childElementCount > 60) {
            elements.log.lastElementChild.remove();
        }
    }

    function sharedMemoryAvailable() {
        return typeof SharedArrayBuffer === 'function' && globalThis.crossOriginIsolated === true;
    }

    function describeIsolation() {
        const isolated = globalThis.crossOriginIsolated === true;
        const constructorPresent = typeof SharedArrayBuffer === 'function';
        elements.isolation.textContent = isolated
            ? `cross-origin isolated (COOP/COEP present)${constructorPresent ? '' : ' but no SharedArrayBuffer constructor'}`
            : `NOT cross-origin isolated — SharedArrayBuffer unavailable${constructorPresent ? ' despite the constructor existing' : ''}`;
        elements.isolation.className = isolated ? 'value good' : 'value warn';
    }

    async function start() {
        elements.start.disabled = true;
        state.startedAt = performance.now();
        state.memoryVerdictReported = false;

        try {
            const useSharedMemory = sharedMemoryAvailable() && !elements.forceFallback.checked;
            if (!useSharedMemory && sharedMemoryAvailable()) {
                log('SharedArrayBuffer is available but the postMessage fallback was forced.');
            }

            // The page compiles; the worklet only links. Compiling here keeps the audio
            // thread free of the one genuinely expensive step, and `WebAssembly.Module`
            // is structured-cloneable, so it can be handed straight over.
            const response = await fetch(WASM_URL);
            if (!response.ok) {
                throw new Error(`fetching ${WASM_URL}: ${response.status} ${response.statusText}`);
            }
            const wasmBytes = await response.arrayBuffer();
            const wasmModule = await WebAssembly.compile(wasmBytes);
            log(`compiled ${wasmBytes.byteLength} bytes of wasm on the page's thread`);

            state.context = new AudioContext({ latencyHint: 'interactive' });
            await state.context.audioWorklet.addModule(WORKLET_URL);
            log(`worklet module loaded; AudioContext sample rate is ${state.context.sampleRate} Hz`);

            state.commandRing = useSharedMemory ? Ring.createCommandRing() : null;
            state.telemetry = useSharedMemory ? Ring.createTelemetry() : null;

            const processorOptions = {
                channelCount: CHANNEL_COUNT,
                wasmModule,
                commandRing: state.commandRing ? state.commandRing.buffer : null,
                telemetry: state.telemetry ? state.telemetry.buffer : null,
            };

            state.node = createNode(processorOptions, wasmBytes);
            state.node.port.onmessage = (event) => onWorkletMessage(event.data);
            state.node.onprocessorerror = () => log('FAULT: the processor threw and was torn down');
            state.node.connect(state.context.destination);

            await state.context.resume();

            state.commandTransport = state.commandRing ? 'SharedArrayBuffer ring' : 'postMessage (batched per animation frame)';
            state.telemetryTransport = state.telemetry ? 'SharedArrayBuffer block' : `postMessage (every ${Ring.TELEMETRY_FALLBACK_QUANTA} quanta)`;
            elements.commandTransport.textContent = state.commandTransport;
            elements.telemetryTransport.textContent = state.telemetryTransport;
            elements.stop.disabled = false;

            sendFrequency(Number(elements.frequency.value), true);
            log('audio running');
        } catch (error) {
            elements.start.disabled = false;
            log(`FAILED to start: ${error && error.message ? error.message : error}`);
            throw error;
        }
    }

    /**
     * Structured-cloning a `WebAssembly.Module` into `processorOptions` is the fast path
     * and works in current Chromium and Firefox. If a browser refuses, fall back to
     * shipping the bytes and compiling inside the worklet — slower, but it only happens
     * once, at construction, and never on the audio path.
     */
    function createNode(processorOptions, wasmBytes) {
        const nodeOptions = {
            numberOfInputs: 0,
            numberOfOutputs: 1,
            outputChannelCount: [CHANNEL_COUNT],
            processorOptions,
        };
        try {
            return new AudioWorkletNode(state.context, PROCESSOR_NAME, nodeOptions);
        } catch (error) {
            log(`passing a compiled WebAssembly.Module failed (${error.name}); sending raw bytes instead`);
            nodeOptions.processorOptions = Object.assign({}, processorOptions, {
                wasmModule: null,
                wasmBytes,
            });
            return new AudioWorkletNode(state.context, PROCESSOR_NAME, nodeOptions);
        }
    }

    function stop() {
        if (state.node !== null) {
            state.node.port.postMessage({ type: 'stop' });
            state.node.disconnect();
            state.node = null;
        }
        if (state.context !== null) {
            state.context.close();
            state.context = null;
        }
        elements.stop.disabled = true;
        elements.start.disabled = false;
        log('audio stopped');
    }

    function onWorkletMessage(message) {
        if (message.type === 'ready') {
            state.initialMemoryBytes = message.initialMemoryBytes;
            log(`worklet ready: engine quantum ${message.renderQuantum}, wasm memory ${message.initialMemoryBytes} bytes`);
        } else if (message.type === 'quantum') {
            elements.quantum.textContent = `${message.frames} frames${message.matchesEngine ? ' — matches RENDER_QUANTUM' : ' — DOES NOT match RENDER_QUANTUM'}`;
            elements.quantum.className = message.matchesEngine ? 'value good' : 'value warn';
            log(`AudioWorklet handed us ${message.frames} frames per process() call`);
        } else if (message.type === 'telemetry') {
            state.latest = message;
        } else if (message.type === 'fault') {
            log(`FAULT: ${message.reason}`);
        }
    }

    /**
     * Slider handler. Under `SharedArrayBuffer` this is a ring write and nothing else —
     * no message, no structured clone, no work on the audio thread's behalf.
     *
     * A full ring is not treated as a fault. The ring holds 64 records and the audio
     * thread drains all of them every 2.7 ms, so a human cannot fill it; a script that
     * fires a hundred `input` events inside one task can. `SetFrequency` is a
     * last-writer-wins parameter rather than a discrete event, so the honest response to
     * back-pressure is to keep the newest value and offer it again on the next animation
     * frame. The count is surfaced so that back-pressure stays visible rather than
     * silently smoothing over an audio thread that has actually stopped.
     */
    function sendFrequency(hertz, immediate) {
        elements.frequencyReadout.textContent = `${hertz.toFixed(1)} Hz`;
        if (state.node === null) {
            return;
        }
        if (state.commandRing !== null) {
            if (Ring.pushSetFrequency(state.commandRing, hertz)) {
                state.pendingFrequency = null;
            } else {
                state.pendingFrequency = hertz;
                state.ringBackPressureEvents += 1;
            }
            return;
        }
        state.pendingFrequency = hertz;
        if (immediate) {
            flushPendingFrequency();
        }
    }

    /**
     * Called once per animation frame. On the ring path it retries whatever back-pressure
     * left behind; on the fallback path it is the rate limiter that turns any number of
     * slider events into at most one message per frame.
     */
    function flushPendingFrequency() {
        if (state.node === null || state.pendingFrequency === null) {
            return;
        }
        if (state.commandRing !== null) {
            if (Ring.pushSetFrequency(state.commandRing, state.pendingFrequency)) {
                state.pendingFrequency = null;
            }
            return;
        }
        if (state.pendingFrequency === state.lastPostedFrequency) {
            state.pendingFrequency = null;
            return;
        }
        state.node.port.postMessage({ type: 'setFrequency', hertz: state.pendingFrequency });
        state.lastPostedFrequency = state.pendingFrequency;
        state.pendingFrequency = null;
    }

    function decibelsFromAmplitude(amplitude) {
        if (!(amplitude > 0)) {
            return -Infinity;
        }
        return 20 * Math.log10(amplitude);
    }

    function refresh() {
        requestAnimationFrame(refresh);
        flushPendingFrequency();

        const snapshot = state.telemetry !== null ? Ring.readTelemetry(state.telemetry) : state.latest;
        if (!snapshot) {
            return;
        }
        // A shared block that has never been published reads as all zeros, which is
        // indistinguishable from a real snapshot except for the sequence counter. Reading
        // it as real is how a zeroed `memoryBytes` once got recorded as "memory shrank".
        if (snapshot.sequence === 0) {
            return;
        }

        const decibels = decibelsFromAmplitude(snapshot.peak);
        const fraction = Math.max(0, Math.min(1, (decibels + 60) / 60));
        elements.peakBar.style.width = `${(fraction * 100).toFixed(1)}%`;
        elements.peakReadout.textContent = Number.isFinite(decibels)
            ? `${snapshot.peak.toFixed(4)} (${decibels.toFixed(1)} dBFS)`
            : 'silence';
        elements.actualFrequency.textContent = `${snapshot.frequency.toFixed(1)} Hz`;
        elements.quanta.textContent = `${snapshot.quantaRendered.toFixed(0)}`;
        elements.dropped.textContent = `${snapshot.droppedCommands}`;
        elements.dropped.className = snapshot.droppedCommands > 0 ? 'value warn' : 'value';
        elements.backPressure.textContent = `${state.ringBackPressureEvents}`;
        elements.backPressure.className = state.ringBackPressureEvents > 0 ? 'value warn' : 'value';
        elements.memory.textContent = `${snapshot.memoryBytes} bytes (${(snapshot.memoryBytes / 65536).toFixed(0)} pages)`;

        if (state.initialMemoryBytes !== null && snapshot.memoryBytes !== state.initialMemoryBytes) {
            elements.memoryVerdict.textContent = `GREW from ${state.initialMemoryBytes} to ${snapshot.memoryBytes}`;
            elements.memoryVerdict.className = 'value warn';
            state.memoryVerdictReported = true;
        } else if (state.startedAt !== null && !state.memoryVerdictReported) {
            const elapsedSeconds = (performance.now() - state.startedAt) / 1000;
            if (elapsedSeconds >= MEMORY_WATCH_SECONDS) {
                elements.memoryVerdict.textContent = `unchanged after ${MEMORY_WATCH_SECONDS}s at ${snapshot.memoryBytes} bytes`;
                elements.memoryVerdict.className = 'value good';
                state.memoryVerdictReported = true;
                log(`memory check passed: ${snapshot.memoryBytes} bytes at start and after ${MEMORY_WATCH_SECONDS}s`);
            } else {
                elements.memoryVerdict.textContent = `watching (${elapsedSeconds.toFixed(0)}/${MEMORY_WATCH_SECONDS}s)`;
            }
        }
    }

    elements.start.addEventListener('click', () => { start().catch(() => {}); });
    elements.stop.addEventListener('click', stop);
    elements.frequency.addEventListener('input', () => sendFrequency(Number(elements.frequency.value), false));

    describeIsolation();
    elements.frequencyReadout.textContent = `${Number(elements.frequency.value).toFixed(1)} Hz`;
    elements.commandTransport.textContent = state.commandTransport;
    elements.telemetryTransport.textContent = state.telemetryTransport;
    elements.forceFallback.disabled = !sharedMemoryAvailable();
    requestAnimationFrame(refresh);
    log('page loaded — press Start (browsers require a gesture before audio)');
})();
