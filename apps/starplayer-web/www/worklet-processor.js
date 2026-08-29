// The AudioWorklet processor.
//
// `cargo xtask wasm` concatenates three things into `dist/starplayer-worklet.js`:
//
//   1. the `wasm-bindgen --target no-modules` glue, which defines `wasm_bindgen`,
//   2. `ring.js`, which defines `globalThis.StarPlayerRing`,
//   3. this file.
//
// Concatenation is the whole of the "worklet-scope massaging" the build does, and it is
// forced by the environment: `AudioWorklet.addModule()` takes exactly one URL, and
// inside `AudioWorkletGlobalScope` there is no `fetch`, no `importScripts` and no
// reliable dynamic `import`. So everything the processor needs has to arrive in that one
// file — except the wasm itself, which arrives as an already-compiled
// `WebAssembly.Module` in `processorOptions`.

'use strict';

const Ring = globalThis.StarPlayerRing;

class StarPlayerSineProcessor extends AudioWorkletProcessor {
    constructor(options) {
        super();

        const settings = options.processorOptions;

        // Instantiation is synchronous and happens here, in the constructor, so that the
        // very first `process()` call has nothing left to set up. The module was compiled
        // on the page's thread; `initSync` only has to link it.
        const module = settings.wasmModule instanceof WebAssembly.Module
            ? settings.wasmModule
            : new WebAssembly.Module(settings.wasmBytes);
        const wasm = wasm_bindgen.initSync({ module });

        // `init` allocates every buffer the audio path will touch *and* forces the heap
        // to whatever size it is going to need, so the views taken immediately below can
        // never be detached by a later `memory.grow`.
        wasm_bindgen.init(sampleRate, settings.channelCount);

        this.memory = wasm.memory;
        this.initialMemoryBytes = this.memory.buffer.byteLength;
        this.renderQuantum = wasm_bindgen.render_quantum();

        const outputPointer = wasm_bindgen.output_ptr();
        const channelStride = wasm_bindgen.output_channel_stride();
        this.wasmChannelCount = settings.channelCount;

        // One view per channel, sized to exactly one quantum and built once. The steady
        // state is `output[c].set(this.channelViews[c])` — no `subarray`, no object
        // churn, nothing for the collector to do on the audio thread.
        this.channelViews = [];
        this.wideChannelViews = [];
        for (let channel = 0; channel < settings.channelCount; channel += 1) {
            const byteOffset = outputPointer + channel * channelStride * 4;
            this.channelViews.push(new Float32Array(this.memory.buffer, byteOffset, this.renderQuantum));
            this.wideChannelViews.push(new Float32Array(this.memory.buffer, byteOffset, channelStride));
        }

        this.commandRing = settings.commandRing ? Ring.viewCommandRing(settings.commandRing) : null;
        this.telemetry = settings.telemetry ? Ring.viewTelemetry(settings.telemetry) : null;

        // Only used on the `postMessage` fallback: the latest frequency the page asked
        // for, coalesced, applied on the next quantum.
        this.pendingFrequency = null;
        this.quantaSinceTelemetryPost = 0;
        this.reportedQuantumFrames = 0;
        this.running = true;
        this.faulted = null;

        // Bound once so the hot path does not create a closure per quantum.
        this.applyCommand = (opcode, value) => {
            if (opcode === Ring.OPCODE_SET_FREQUENCY) {
                wasm_bindgen.push_set_frequency(value);
            }
        };

        this.port.onmessage = (event) => this.onMessage(event.data);
        this.port.postMessage({
            type: 'ready',
            sampleRate,
            renderQuantum: this.renderQuantum,
            initialMemoryBytes: this.initialMemoryBytes,
            commandTransport: this.commandRing ? 'SharedArrayBuffer' : 'postMessage',
            telemetryTransport: this.telemetry ? 'SharedArrayBuffer' : 'postMessage',
        });
    }

    onMessage(message) {
        if (message.type === 'setFrequency') {
            // The fallback path. The page has already coalesced everything the user did
            // since the last animation frame into this one value.
            this.pendingFrequency = message.hertz;
        } else if (message.type === 'stop') {
            this.running = false;
        }
    }

    process(inputs, outputs) {
        const output = outputs[0];
        if (!output || output.length === 0) {
            return this.running;
        }
        const frames = output[0].length;

        // Deliverable: confirm the render quantum is what the specification promises,
        // rather than assuming it. Reported once, from the audio thread, with the real
        // number the host handed us.
        if (this.reportedQuantumFrames !== frames) {
            this.reportedQuantumFrames = frames;
            this.port.postMessage({
                type: 'quantum',
                frames,
                matchesEngine: frames === this.renderQuantum,
                memoryBytes: this.memory.buffer.byteLength,
            });
        }

        // A detached view means linear memory grew under us — the exact failure this
        // design exists to prevent. Say so once and go silent rather than write garbage.
        if (this.channelViews[0].length === 0 && this.faulted === null) {
            this.faulted = 'wasm memory grew on the audio thread; views detached';
            this.port.postMessage({ type: 'fault', reason: this.faulted });
        }
        if (this.faulted !== null) {
            return this.running;
        }

        let commandsApplied = 0;
        if (this.commandRing !== null) {
            commandsApplied = Ring.drainCommands(this.commandRing, this.applyCommand);
        } else if (this.pendingFrequency !== null) {
            wasm_bindgen.push_set_frequency(this.pendingFrequency);
            this.pendingFrequency = null;
            commandsApplied = 1;
        }

        const peak = wasm_bindgen.process(frames);

        if (frames === this.renderQuantum) {
            for (let channel = 0; channel < output.length; channel += 1) {
                const source = this.channelViews[Math.min(channel, this.wasmChannelCount - 1)];
                output[channel].set(source);
            }
        } else {
            // Never taken in a conforming browser; here so an off-spec host degrades to
            // correct-but-slower instead of silent.
            for (let channel = 0; channel < output.length; channel += 1) {
                const source = this.wideChannelViews[Math.min(channel, this.wasmChannelCount - 1)];
                output[channel].set(source.subarray(0, frames));
            }
        }

        this.publishTelemetry(peak, commandsApplied);
        return this.running;
    }

    publishTelemetry(peak, commandsApplied) {
        const memoryBytes = this.memory.buffer.byteLength;
        if (this.telemetry !== null) {
            Ring.publishTelemetry(this.telemetry, {
                quantumFrames: this.reportedQuantumFrames,
                droppedCommands: wasm_bindgen.dropped_commands(),
                memoryBytes,
                peak,
                frequency: wasm_bindgen.current_frequency(),
                quantaRendered: wasm_bindgen.quanta_rendered(),
                commandsApplied,
            });
            return;
        }

        // Fallback: one message per `TELEMETRY_FALLBACK_QUANTA`, about 47 Hz at 48 kHz.
        // This allocates an object on the audio thread, which is precisely why it is the
        // fallback and not the default — see the Q1 note in the architecture document.
        this.quantaSinceTelemetryPost += 1;
        if (this.quantaSinceTelemetryPost < Ring.TELEMETRY_FALLBACK_QUANTA) {
            return;
        }
        this.quantaSinceTelemetryPost = 0;
        this.port.postMessage({
            type: 'telemetry',
            peak,
            frequency: wasm_bindgen.current_frequency(),
            quantaRendered: wasm_bindgen.quanta_rendered(),
            droppedCommands: wasm_bindgen.dropped_commands(),
            memoryBytes,
            commandsApplied,
        });
    }
}

registerProcessor('starplayer-sine', StarPlayerSineProcessor);
