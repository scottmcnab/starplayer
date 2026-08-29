// AudioWorklet half of the web player. `xtask wasm` preserves A4's architecture by
// concatenating no-modules wasm-bindgen glue + ring.js + this file into one bundle.

'use strict';

const Ring = globalThis.StarPlayerRing;

class StarPlayerProcessor extends AudioWorkletProcessor {
    constructor(options) {
        super();
        const settings = options.processorOptions;
        const module = settings.wasmModule instanceof WebAssembly.Module
            ? settings.wasmModule
            : new WebAssembly.Module(settings.wasmBytes);
        const wasm = wasm_bindgen.initSync({ module });

        wasm_bindgen.init(sampleRate, settings.channelCount);
        this.memory = wasm.memory;
        this.renderQuantum = wasm_bindgen.render_quantum();
        this.channelCount = settings.channelCount;
        this.commandRing = settings.commandRing ? Ring.viewCommandRing(settings.commandRing) : null;
        this.telemetry = settings.telemetry ? Ring.viewTelemetry(settings.telemetry) : null;
        this.quantaSinceFallback = 0;
        this.reportedQuantum = false;
        this.faulted = null;
        this.initialMemoryBytes = this.memory.buffer.byteLength;
        this.stableMemoryBytes = this.initialMemoryBytes;
        this.bindViews();

        this.applyCommand = (opcode, argument, extra) => {
            wasm_bindgen.enqueue_command(opcode, argument, extra);
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

    bindViews() {
        const outputPointer = wasm_bindgen.output_ptr();
        const channelStride = wasm_bindgen.output_channel_stride();
        this.channelViews = [];
        this.wideChannelViews = [];
        for (let channel = 0; channel < this.channelCount; channel += 1) {
            const byteOffset = outputPointer + channel * channelStride * 4;
            this.channelViews.push(new Float32Array(this.memory.buffer, byteOffset, this.renderQuantum));
            this.wideChannelViews.push(new Float32Array(this.memory.buffer, byteOffset, channelStride));
        }
        this.telemetryWords = new Int32Array(
            this.memory.buffer,
            wasm_bindgen.telemetry_ptr(),
            wasm_bindgen.telemetry_len(),
        );
        this.stableMemoryBytes = this.memory.buffer.byteLength;
    }

    onMessage(message) {
        if (message.type === 'loadModule') {
            try {
                const generation = wasm_bindgen.load_module(new Uint8Array(message.bytes));
                // Activation may consume more of the pre-reserved heap. Rebind once here,
                // outside process; any growth after this point is a fatal RT-path defect.
                this.bindViews();
                this.port.postMessage({
                    type: 'moduleLoaded',
                    requestId: message.requestId,
                    generation,
                    memoryBytes: this.stableMemoryBytes,
                });
            } catch (error) {
                this.port.postMessage({
                    type: 'moduleError',
                    requestId: message.requestId,
                    reason: error && error.message ? error.message : String(error),
                });
            }
        } else if (message.type === 'commandBatch') {
            Ring.drainFallbackCommands(message.commands, this.applyCommand);
        } else if (message.type === 'collectGarbage') {
            const collected = wasm_bindgen.collect_garbage();
            this.port.postMessage({
                type: 'garbageCollected',
                collected,
                total: wasm_bindgen.retired_modules_collected(),
                pending: wasm_bindgen.pending_garbage(),
            });
        }
    }

    process(inputs, outputs) {
        const output = outputs[0];
        if (!output || output.length === 0) {
            return true;
        }
        const frames = output[0].length;

        if (!this.reportedQuantum) {
            this.reportedQuantum = true;
            this.port.postMessage({
                type: 'quantum',
                frames,
                matchesEngine: frames === this.renderQuantum,
                memoryBytes: this.memory.buffer.byteLength,
            });
        }

        if (this.memory.buffer.byteLength !== this.stableMemoryBytes || this.channelViews[0].length === 0) {
            if (this.faulted === null) {
                this.faulted = `wasm memory grew in process (${this.stableMemoryBytes} → ${this.memory.buffer.byteLength})`;
                this.port.postMessage({ type: 'fault', reason: this.faulted });
            }
            return true;
        }

        if (this.commandRing !== null) {
            Ring.drainCommands(this.commandRing, this.applyCommand);
        }

        wasm_bindgen.process(frames);
        if (this.memory.buffer.byteLength !== this.stableMemoryBytes) {
            this.faulted = `wasm memory grew in process (${this.stableMemoryBytes} → ${this.memory.buffer.byteLength})`;
            this.port.postMessage({ type: 'fault', reason: this.faulted });
            return true;
        }

        if (frames === this.renderQuantum) {
            for (let channel = 0; channel < output.length; channel += 1) {
                output[channel].set(this.channelViews[Math.min(channel, this.channelCount - 1)]);
            }
        } else {
            for (let channel = 0; channel < output.length; channel += 1) {
                output[channel].set(this.wideChannelViews[Math.min(channel, this.channelCount - 1)].subarray(0, frames));
            }
        }

        const dropped = wasm_bindgen.dropped_commands()
            + (this.commandRing === null ? 0 : Ring.commandOverflows(this.commandRing));
        const quanta = wasm_bindgen.quanta_rendered();
        if (this.telemetry !== null) {
            Ring.publishTelemetry(
                this.telemetry,
                this.telemetryWords,
                this.memory.buffer.byteLength,
                frames,
                dropped,
                quanta,
            );
        } else {
            this.quantaSinceFallback += 1;
            if (this.quantaSinceFallback >= Ring.TELEMETRY_FALLBACK_QUANTA) {
                this.quantaSinceFallback = 0;
                const snapshot = Ring.decodeWasmTelemetry(
                    this.telemetryWords,
                    this.memory.buffer.byteLength,
                    frames,
                    dropped,
                    quanta,
                );
                snapshot.type = 'telemetry';
                this.port.postMessage(snapshot);
            }
        }
        return true;
    }
}

registerProcessor('starplayer-player', StarPlayerProcessor);
