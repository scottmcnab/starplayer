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
        // The bundle turns wasm-bindgen's no-modules IIFE into a factory. Nodes overlap
        // while a graph rebuild is prepared, so each processor must own a distinct Rust
        // HOST and WebAssembly.Memory even though they share one compiled module.
        this.wasm = createStarPlayerWasmBindings();
        const wasm = this.wasm.initSync({ module });

        if (!this.wasm.init(sampleRate, settings.mixerMode)) {
            throw new Error('the requested mixer mode has no web engine arm');
        }
        this.memory = wasm.memory;
        this.renderQuantum = this.wasm.render_quantum();
        this.channelCount = settings.channelCount;
        this.commandRing = settings.commandRing ? Ring.viewCommandRing(settings.commandRing) : null;
        this.telemetry = settings.telemetry ? Ring.viewTelemetry(settings.telemetry) : null;
        this.scope = settings.scope ? Ring.viewScope(settings.scope) : null;
        this.scopeBucketFrames = this.wasm.scope_bucket_frames();
        // The wasm scope block is refreshed every few quanta, not every one, so this is
        // what keeps the publish (or the fallback copy) to one per refresh.
        this.publishedScopeGeneration = -1;
        this.quantaSinceFallback = 0;
        this.reportedQuantum = false;
        this.faulted = null;
        this.initialMemoryBytes = this.memory.buffer.byteLength;
        this.stableMemoryBytes = this.initialMemoryBytes;
        this.bindViews();

        this.pendingMixerMode = null;
        // Every other opcode is staged for the render path. A mode change is not: whichever
        // drain sees it — process(), or a message task — it is only retained as a scalar,
        // because rebuilding a typed engine allocates. `applyPendingMixerMode` runs it, and
        // is only ever called from a message task.
        this.applyCommand = (opcode, argument, extra) => {
            if (opcode === Ring.OPCODE_SET_MIXER_MODE) {
                this.pendingMixerMode = argument >>> 0;
            } else {
                this.wasm.enqueue_command(opcode, argument, extra);
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
            scopeTransport: this.scope ? 'SharedArrayBuffer' : 'postMessage',
            scopeBucketFrames: this.scopeBucketFrames,
            scopeWindowBuckets: this.wasm.scope_window_buckets(),
            // M7-H7: every effect this build can install, its parameters, their units,
            // ranges and defaults, as JSON — read straight from the wasm host's own
            // `InsertDescriptor`s so the page's Effects panel can never drift from what
            // `install_insert` and `OPCODE_INSERT_PARAM` actually accept. Needs no loaded
            // module and no prior state, so it rides the very first message.
            effects: this.wasm.effects_json(),
            // M10-W4: every load-time sample enhancer with a checkbox bit, read straight
            // from `starplayer::enhance::CATALOGUE` so the load panel's checkboxes can
            // never drift from what `loadModule`'s `enhancementFlags` actually accepts.
            enhancements: this.wasm.enhancements_json(),
        });
    }

    applyPendingMixerMode() {
        if (this.pendingMixerMode === null) return;
        const requested = this.pendingMixerMode;
        this.pendingMixerMode = null;
        try {
            const active = this.wasm.set_mixer_mode(requested);
            // Rebuilding typed engine storage may grow wasm memory. Rebase only here,
            // outside process(), exactly as module activation does.
            this.bindViews();
            this.port.postMessage({ type: 'mixerModeApplied', requested, active, memoryBytes: this.stableMemoryBytes });
        } catch (error) {
            this.port.postMessage({
                type: 'mixerModeError',
                requested,
                reason: error && error.message ? error.message : String(error),
            });
        }
    }

    bindViews() {
        const outputPointer = this.wasm.output_ptr();
        const channelStride = this.wasm.output_channel_stride();
        this.channelViews = [];
        this.wideChannelViews = [];
        for (let channel = 0; channel < this.channelCount; channel += 1) {
            const byteOffset = outputPointer + channel * channelStride * 4;
            this.channelViews.push(new Float32Array(this.memory.buffer, byteOffset, this.renderQuantum));
            this.wideChannelViews.push(new Float32Array(this.memory.buffer, byteOffset, channelStride));
        }
        this.telemetryWords = new Int32Array(
            this.memory.buffer,
            this.wasm.telemetry_ptr(),
            this.wasm.telemetry_len(),
        );
        // The scope taps (architecture §9(b)). Two more views over blocks the Rust host
        // allocated at construction; like every view here they are rebound outside
        // process(), because module activation may have grown wasm memory.
        this.scopeValues = new Int16Array(this.memory.buffer, this.wasm.scope_ptr(), this.wasm.scope_len());
        this.scopeIndices = new Int32Array(this.memory.buffer, this.wasm.scope_index_ptr(), this.wasm.scope_index_len());
        this.stableMemoryBytes = this.memory.buffer.byteLength;
    }

    onMessage(message) {
        if (message.type === 'loadModule') {
            try {
                const generation = this.wasm.load_module_with_options(
                    new Uint8Array(message.bytes),
                    message.headphoneFriendlyModPanning === true,
                    message.enhancementFlags >>> 0 || 0,
                );
                // Activation may consume more of the pre-reserved heap. Rebind once here,
                // outside process; any growth after this point is a fatal RT-path defect.
                this.bindViews();
                this.port.postMessage({
                    type: 'moduleLoaded',
                    requestId: message.requestId,
                    generation,
                    memoryBytes: this.stableMemoryBytes,
                    // M10-W4: what the load actually did, which can be narrower than what
                    // was asked for — see `build_enhancement`'s frame-budget fallback.
                    appliedEnhancementFlags: this.wasm.applied_enhancement_flags(),
                    appliedEnhancementFactor: this.wasm.applied_enhancement_factor(),
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
            this.applyPendingMixerMode();
        } else if (message.type === 'flushCommands') {
            if (this.commandRing !== null) {
                Ring.drainCommands(this.commandRing, this.applyCommand);
            }
            this.applyPendingMixerMode();
        } else if (message.type === 'midiInput') {
            // Installing jam mode builds an instrument rack, queue and two-source mux, so
            // it happens here — in a message task — exactly as a mixer-mode rebuild does.
            // The events themselves are opcode 10 on the ordinary command ring.
            try {
                const active = this.wasm.set_midi_input(message.enabled === true);
                // The rack is allocated out of the pre-reserved heap; rebind here, outside
                // process(), for the same reason module activation does.
                this.bindViews();
                this.port.postMessage({
                    type: 'midiInputApplied',
                    active,
                    leadFrames: this.wasm.event_lead_frames(),
                    leadMillis: this.wasm.event_lead_millis(),
                    memoryBytes: this.stableMemoryBytes,
                });
            } catch (error) {
                this.port.postMessage({
                    type: 'midiInputError',
                    requested: message.enabled === true,
                    reason: error && error.message ? error.message : String(error),
                });
            }
        } else if (message.type === 'inserts') {
            // Building an effect allocates its delay lines (M7-H1), so install and remove
            // both happen here — in a message task — exactly as `midiInput` and a
            // mixer-mode rebuild do. A parameter change or a bypass flip is not this
            // message at all: those are OPCODE_INSERT_PARAM / OPCODE_INSERT_BYPASS on the
            // ordinary command ring, applied in `process()` like every other opcode.
            try {
                if (message.action === 'install') {
                    this.wasm.install_insert(message.target, message.slot, message.kind);
                } else if (message.action === 'remove') {
                    this.wasm.remove_insert(message.target, message.slot);
                }
                // Installing may consume more of the pre-reserved heap; rebind here,
                // outside process(), for the same reason module activation does.
                this.bindViews();
                this.port.postMessage({
                    type: 'insertsApplied',
                    requestId: message.requestId,
                    action: message.action,
                    target: message.target,
                    slot: message.slot,
                    kind: message.kind,
                    memoryBytes: this.stableMemoryBytes,
                });
            } catch (error) {
                this.port.postMessage({
                    type: 'insertsError',
                    requestId: message.requestId,
                    action: message.action,
                    target: message.target,
                    slot: message.slot,
                    reason: error && error.message ? error.message : String(error),
                });
            }
        } else if (message.type === 'collectGarbage') {
            const collected = this.wasm.collect_garbage();
            this.port.postMessage({
                type: 'garbageCollected',
                collected,
                total: this.wasm.retired_modules_collected(),
                pending: this.wasm.pending_garbage(),
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

        this.wasm.process(frames);
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

        const dropped = this.wasm.dropped_commands()
            + (this.commandRing === null ? 0 : Ring.commandOverflows(this.commandRing));
        const quanta = this.wasm.quanta_rendered();
        const scopeGeneration = this.wasm.scope_generation();
        if (this.telemetry !== null) {
            Ring.publishTelemetry(
                this.telemetry,
                this.telemetryWords,
                this.memory.buffer.byteLength,
                frames,
                dropped,
                quanta,
            );
            // Once per refresh of the wasm block, not once per quantum: the taps are
            // downsampled in the engine and the block only moves every few quanta.
            if (this.scope !== null && scopeGeneration !== this.publishedScopeGeneration) {
                this.publishedScopeGeneration = scopeGeneration;
                Ring.publishScope(
                    this.scope,
                    this.scopeValues,
                    this.scopeIndices,
                    this.wasm.scope_channels(),
                    this.scopeBucketFrames,
                    scopeGeneration,
                );
            }
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
                // The scope rides the snapshot's own batched message rather than a
                // second post, so "no postMessage per interaction" still holds. This is
                // the one copy the fallback path accepts, and it is the same copy the
                // snapshot object above already is.
                this.publishedScopeGeneration = scopeGeneration;
                snapshot.scope = Ring.decodeWorkletScope(
                    this.scopeValues,
                    this.scopeIndices,
                    this.wasm.scope_channels(),
                    this.scopeBucketFrames,
                    scopeGeneration,
                );
                this.port.postMessage(snapshot);
            }
        }
        return true;
    }
}

registerProcessor('starplayer-player', StarPlayerProcessor);
