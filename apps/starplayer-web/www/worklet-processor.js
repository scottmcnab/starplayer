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

        const ringVersion = settings.commandRing ? Ring.viewCommandRing(settings.commandRing).version : settings.interactiveVersion;
        this.interactiveVersion = ringVersion === Ring.INTERACTIVE_WIRE_VERSION && settings.interactiveVersion === Ring.INTERACTIVE_WIRE_VERSION && this.wasm.interactive_wire_version?.() === Ring.INTERACTIVE_WIRE_VERSION ? Ring.INTERACTIVE_WIRE_VERSION : 0;
        let initialized = this.interactiveVersion
            ? this.wasm.init_interactive(sampleRate, settings.mixerMode, settings.seedSession ?? 0n, settings.seedBuild ?? 0n)
            : this.wasm.init(sampleRate, settings.mixerMode);
        if (!initialized && this.interactiveVersion) { this.interactiveVersion = 0; initialized = this.wasm.init(sampleRate, settings.mixerMode); }
        if (!initialized) {
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

        this.pendingAcks = [];
        this.nextBarrierLow = 1; this.nextBarrierHigh = 0;
        this.controlAck = { type: 'interactiveApplied', requestedIdentity: new Uint32Array(4), words: new Uint32Array(10), commands: new Uint32Array(Ring.COMMAND_CAPACITY * 3), count: 0, accepted: false };
        this.pendingControlSerial = 0;
        this.pendingControlFailed = false;
        this.fallbackBatches = [];
        this.deferredLoads = new Map();
        this.pendingReplay = null;
        this.controlFailed = false;
        this.pendingMixerMode = null;
        // Every other opcode is staged for the render path. A mode change is not: whichever
        // drain sees it — process(), or a message task — it is only retained as a scalar,
        // because rebuilding a typed engine allocates. `applyPendingMixerMode` runs it, and
        // is only ever called from a message task.
        this.applyCommand = (opcode, argument, extra, sessionLow, sessionHigh, buildLow, buildHigh) => {
            if (this.interactiveVersion) {
                if (opcode === Ring.OPCODE_SET_MIXER_MODE || this.controlAck.count >= Ring.COMMAND_CAPACITY || !this.wasm.enqueue_interactive_words(opcode, argument, extra, sessionLow, sessionHigh, buildLow, buildHigh)) this.controlFailed = true;
                if (this.controlAck.count === 0) { this.controlAck.requestedIdentity[0] = sessionLow; this.controlAck.requestedIdentity[1] = sessionHigh; this.controlAck.requestedIdentity[2] = buildLow; this.controlAck.requestedIdentity[3] = buildHigh; }
                const index = this.controlAck.count++ * 3;
                this.controlAck.commands[index] = opcode; this.controlAck.commands[index + 1] = argument; this.controlAck.commands[index + 2] = extra;
                return;
            }
            if (opcode === Ring.OPCODE_SET_MIXER_MODE) {
                this.pendingMixerMode = argument >>> 0;
            } else {
                this.wasm.enqueue_command(opcode, argument, extra);
            }
        };
        this.port.onmessage = (event) => this.onMessage(event.data);
        this.port.postMessage({
            type: 'ready',
            interactiveVersion: this.interactiveVersion,
            commandRingVersion: this.commandRing?.version ?? this.interactiveVersion,
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

    identity() { return { session: this.wasm.current_session(), build: this.wasm.current_build() }; }

    allocateBarrier() {
        if (this.nextBarrierLow === 0 && this.nextBarrierHigh === 0) return false;
        this.barrierLow = this.nextBarrierLow; this.barrierHigh = this.nextBarrierHigh;
        this.nextBarrierLow = (this.nextBarrierLow + 1) >>> 0;
        if (this.nextBarrierLow === 0) this.nextBarrierHigh = (this.nextBarrierHigh + 1) >>> 0;
        return true;
    }

    barrierReached(status, low, high) { return status[1] > high || (status[1] === high && status[0] >= low); }

    queueApplied(message) {
        if (this.pendingAcks.length >= Ring.COMMAND_CAPACITY || !this.allocateBarrier() || !this.wasm.queue_applied_barrier_words(this.barrierLow, this.barrierHigh)) {
            this.port.postMessage({ ...message, accepted: false, type: message.type === 'moduleLoaded' ? 'moduleError' : message.type === 'stateReady' ? 'stateError' : message.type === 'insertsApplied' ? 'insertsError' : message.type, reason: 'Audio command queue is full.' });
            return;
        }
        message.serialLow = this.barrierLow; message.serialHigh = this.barrierHigh;
        message.serial = BigInt(this.barrierLow) | (BigInt(this.barrierHigh) << 32n);
        message.rejectionsLow = this.interactiveStatus[8]; message.rejectionsHigh = this.interactiveStatus[9];
        message.identityWords = [Number(message.identity.session & 0xffffffffn), Number(message.identity.session >> 32n), Number(message.identity.build & 0xffffffffn), Number(message.identity.build >> 32n)];
        this.pendingAcks.push(message);
    }

    flushInteractive() {
        if (!this.interactiveVersion || this.pendingControlSerial) return;
        const rejected = this.wasm.dropped_commands();
        this.wasm.flush_commands();
        if (this.controlAck.count || this.controlFailed) {
            this.pendingControlFailed = this.controlFailed || this.wasm.dropped_commands() !== rejected;
            this.controlFailed = false;
            if (!this.allocateBarrier() || !this.wasm.queue_applied_barrier_words(this.barrierLow, this.barrierHigh)) {
                this.controlAck.accepted = false; this.controlAck.words.set(this.interactiveStatus);
                this.port.postMessage(this.controlAck); this.controlAck.count = 0; return;
            }
            this.pendingControlSerial = true;
            this.pendingControlLow = this.barrierLow; this.pendingControlHigh = this.barrierHigh;
            this.pendingControlRejectionsLow = this.interactiveStatus[8]; this.pendingControlRejectionsHigh = this.interactiveStatus[9];
        }
    }

    publishApplied() {
        if (!this.interactiveVersion) return;
        const status = this.interactiveStatus;
        if (this.pendingControlSerial && this.barrierReached(status, this.pendingControlLow, this.pendingControlHigh)) {
            const expected = this.controlAck.requestedIdentity;
            this.controlAck.accepted = status[7] !== 0 && status[8] === this.pendingControlRejectionsLow && status[9] === this.pendingControlRejectionsHigh && !this.pendingControlFailed && status[2] === expected[0] && status[3] === expected[1] && status[4] === expected[2] && status[5] === expected[3];
            this.controlAck.words.set(status);
            this.controlAck.words.set(this.controlAck.requestedIdentity, 2);
            this.port.postMessage(this.controlAck);
            this.controlAck.count = 0;
            this.pendingControlSerial = 0;
        }
        while (this.pendingAcks.length && this.barrierReached(status, this.pendingAcks[0].serialLow, this.pendingAcks[0].serialHigh)) {
            const message = this.pendingAcks.shift();
            const expected = message.identityWords;
            const accepted = status[7] !== 0 && status[8] === message.rejectionsLow && status[9] === message.rejectionsHigh && status[2] === expected[0] && status[3] === expected[1] && status[4] === expected[2] && status[5] === expected[3] && !message.failed;
            message.accepted = accepted;
            message.capabilities = status[6];
            if (!accepted) { message.type = message.type === 'moduleLoaded' ? 'moduleError' : message.type === 'insertsApplied' ? 'insertsError' : message.type; message.reason = 'Audio rejected a command, prepared load or state replay.'; }
            this.port.postMessage(message);
        }
    }

    stageLoadTransport(opcode, identity) {
        if (![Ring.OPCODE_INTERACTIVE_PLAY, Ring.OPCODE_PAUSE, Ring.OPCODE_RESUME, Ring.OPCODE_HOLD_STOP].includes(opcode)) return false;
        if (!this.wasm.enqueue_interactive_words(opcode, 0, 0, Number(identity.session & 0xffffffffn), Number(identity.session >> 32n), Number(identity.build & 0xffffffffn), Number(identity.build >> 32n))) return false;
        const rejected = this.wasm.dropped_commands();
        this.wasm.flush_commands();
        return this.wasm.dropped_commands() === rejected;
    }

    startReplay(settings, loaded) {
        const identity = loaded.identity;
        const actions = [{ opcode: Ring.OPCODE_MASTER_VOLUME, argument: settings.volume, extra: 0 }];
        for (const channel of settings.mutes) actions.push({ opcode: Ring.OPCODE_MUTE_CHANNEL, argument: channel, extra: 1 });
        for (const insert of settings.inserts) {
            actions.push({ install: insert });
            for (const [parameter, value] of Object.entries(insert.params)) actions.push({ opcode: Ring.OPCODE_INSERT_PARAM, argument: Ring.packInsertParamArgument(insert.target, insert.slot, Number(parameter)), extra: value >>> 0 });
            if (insert.bypassed) actions.push({ opcode: Ring.OPCODE_INSERT_BYPASS, argument: Ring.packInsertSlotArgument(insert.target, insert.slot), extra: 1 });
        }
        this.pendingReplay = { actions, loaded, index: 0, rejected: this.wasm.dropped_commands(), identityWords: [Number(identity.session & 0xffffffffn), Number(identity.session >> 32n), Number(identity.build & 0xffffffffn), Number(identity.build >> 32n)], resumeMessage: { type: 'loadReplayPending', requestId: loaded.requestId }, resumeSent: false, waitUntil: 0 };
        this.continueReplay();
    }

    continueReplay() {
        const replay = this.pendingReplay;
        if (!replay) return;
        try {
            this.wasm.collect_garbage();
            if (this.wasm.dropped_commands() !== replay.rejected) throw new Error('Audio rejected part of the settings replay.');
            // Leave room for prepared Load/transport commands and the final barrier.
            // A large64-lane mute/insert replay spans whole quanta while input stays off.
            if (this.wasm.pending_control_commands() < 32) {
                const words = replay.identityWords;
                const end = Math.min(replay.index + 16, replay.actions.length);
                for (; replay.index < end; replay.index += 1) {
                    const action = replay.actions[replay.index];
                    if (action.install) this.wasm.install_insert(action.install.target, action.install.slot, action.install.wire);
                    else if (!this.wasm.enqueue_interactive_words(action.opcode, action.argument, action.extra, words[0], words[1], words[2], words[3])) throw new Error('Settings staging queue is full.');
                }
                this.wasm.flush_commands();
            }
            this.bindViews();
            if (replay.index === replay.actions.length) {
                replay.loaded.failed ||= this.wasm.dropped_commands() !== replay.rejected;
                replay.loaded.memoryBytes = this.stableMemoryBytes;
                this.pendingReplay = null;
                this.queueApplied(replay.loaded);
            } else {
                replay.waitUntil = this.wasm.quanta_rendered() + 1;
                replay.resumeSent = false;
            }
        } catch (error) {
            this.bindViews();
            this.pendingReplay = null;
            this.port.postMessage({ type: 'moduleError', requestId: replay.loaded.requestId, identity: replay.loaded.identity, reason: String(error) });
        }
    }

    applyPendingMixerMode() {
        if (this.pendingMixerMode === null) return;
        const requested = this.pendingMixerMode;
        this.pendingMixerMode = null;
        try {
            if (this.interactiveVersion) throw new Error('Version2 mixer changes require the replacement-message path.');
            const active = this.wasm.set_mixer_mode(requested);
            // Rebuilding typed engine storage may grow wasm memory. Rebase only here,
            // outside process(), exactly as module activation does.
            this.bindViews();
            if (this.interactiveVersion) this.queueApplied({ type: 'mixerModeApplied', requested, active, identity: this.identity(), memoryBytes: this.stableMemoryBytes });
            else this.port.postMessage({ type: 'mixerModeApplied', requested, active, memoryBytes: this.stableMemoryBytes });
        } catch (error) {
            this.port.postMessage({
                type: 'mixerModeError',
                requested,
                reason: error && error.message ? error.message : String(error),
            });
        }
    }

    bindViews() {
        if (this.interactiveVersion) this.interactiveStatus = new Uint32Array(this.memory.buffer, this.wasm.interactive_status_ptr(), 10);
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
        if (message.type === 'continueReplay') { if (this.pendingReplay?.loaded.requestId === message.requestId) this.continueReplay(); return; }
        if (message.type === 'continueLoad') { const load = this.deferredLoads.get(message.requestId); if (load) { this.deferredLoads.delete(message.requestId); this.onMessage(load); } return; }
        if (message.type === 'loadTransport') {
            if (!this.interactiveVersion || !Ring.validIdentity(message.identity) || ![Ring.OPCODE_INTERACTIVE_PLAY, Ring.OPCODE_PAUSE, Ring.OPCODE_RESUME, Ring.OPCODE_HOLD_STOP].includes(message.opcode) || message.argument !== 0 || message.extra !== 0) return;
            const deferred = this.deferredLoads.get(message.requestId);
            if (deferred && Ring.sameIdentity(deferred.identity, message.identity)) { deferred.transportOpcode = message.opcode; return; }
            const loaded = this.pendingReplay?.loaded.requestId === message.requestId ? this.pendingReplay.loaded : this.pendingAcks.find(pending => pending.type === 'moduleLoaded' && pending.requestId === message.requestId);
            if (!loaded || !Ring.sameIdentity(loaded.identity, message.identity) || !Ring.sameIdentity(message.identity, this.identity())) return;
            loaded.transportOpcode = message.opcode;
            if (!this.stageLoadTransport(message.opcode, message.identity)) loaded.failed = true;
            return;
        }
        if (message.type === 'loadModule') {
            try {
                if (this.interactiveVersion && !Ring.validIdentity(message.identity)) throw new Error('Missing full playback identity.');
                if (this.commandRing && !this.pendingControlSerial) Ring.drainCommands(this.commandRing, this.applyCommand, this.interactiveVersion ? 63 : Ring.COMMAND_CAPACITY);
                this.flushInteractive();
                if (this.interactiveVersion && (this.pendingControlSerial || this.fallbackBatches.length || this.pendingReplay)) {
                    if (this.deferredLoads.size >= Ring.COMMAND_CAPACITY) throw new Error('Too many prepared loads.');
                    this.deferredLoads.set(message.requestId, message);
                    this.port.postMessage({ type: 'loadDeferred', requestId: message.requestId }); return;
                }
                const generation = this.interactiveVersion ? this.wasm.load_module_identified(
                    new Uint8Array(message.bytes), message.headphoneFriendlyModPanning === true,
                    message.enhancementFlags >>> 0 || 0, Math.trunc(Number(message.speedAdjust)) || 0,
                    message.identity.session, message.identity.build, message.replacement ?? new BigUint64Array(),
                ) : this.wasm.load_module_with_options(
                    new Uint8Array(message.bytes),
                    message.headphoneFriendlyModPanning === true,
                    message.enhancementFlags >>> 0 || 0,
                    // The song is scanned during this call, so the speed bias has to
                    // arrive with the module: the timeline must measure the song at the
                    // speed it will play at. The wasm side clamps it.
                    Math.trunc(Number(message.speedAdjust)) || 0,
                );
                // Activation may consume more of the pre-reserved heap. Rebind once here,
                // outside process; any growth after this point is a fatal RT-path defect.
                this.bindViews();
                const loaded = {
                    type: 'moduleLoaded',
                    identity: message.identity,
                    requestId: message.requestId,
                    generation,
                    memoryBytes: this.stableMemoryBytes,
                    // M10-W4: what the load actually did, which can be narrower than what
                    // was asked for — see `build_enhancement`'s frame-budget fallback.
                    appliedEnhancementFlags: this.wasm.applied_enhancement_flags(),
                    appliedEnhancementFactor: this.wasm.applied_enhancement_factor(),
                };
                if (this.interactiveVersion && message.transportOpcode !== undefined && !this.stageLoadTransport(message.transportOpcode, message.identity)) loaded.failed = true;
                if (this.interactiveVersion && message.hostSettings) this.startReplay(message.hostSettings, loaded);
                else if (this.interactiveVersion) this.queueApplied(loaded);
                else this.port.postMessage(loaded);
            } catch (error) {
                this.port.postMessage({
                    type: 'moduleError',
                    requestId: message.requestId,
                    reason: error && error.message ? error.message : String(error),
                });
            }
        } else if (message.type === 'commandBatch') {
            if (this.interactiveVersion && message.version !== Ring.INTERACTIVE_WIRE_VERSION) return;
            if (this.interactiveVersion) {
                if (this.fallbackBatches.length >= Ring.COMMAND_CAPACITY) {
                    this.port.postMessage({ type: 'interactiveOverflow', identity: this.identity() }); return;
                }
                const records = [];
                Ring.drainFallbackCommands(message.commands, (...record) => records.push(record), message.version);
                for (let index = 0; index < records.length; index += 63) this.fallbackBatches.push(records.slice(index, index + 63));
                if (!this.pendingControlSerial && this.fallbackBatches.length) {
                    const batch = this.fallbackBatches.shift();
                    for (const record of batch) this.applyCommand(...record);
                    this.flushInteractive();
                }
            } else { Ring.drainFallbackCommands(message.commands, this.applyCommand); }
            this.applyPendingMixerMode();
        } else if (message.type === 'flushCommands') {
            if (this.commandRing !== null && !this.pendingControlSerial) {
                Ring.drainCommands(this.commandRing, this.applyCommand, this.interactiveVersion ? 63 : Ring.COMMAND_CAPACITY);
            }
            this.flushInteractive();
            this.applyPendingMixerMode();
        } else if (message.type === 'midiInput') {
            if (this.interactiveVersion) { this.port.postMessage({ type: 'midiInputError', reason: 'Version2 live routing requires an identified destination command.' }); return; }
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
            // Preparation allocates only here. Parameters already queued ahead of this
            // message must reach Rust before the install/remove; never defer a mutation
            // while accepting later parameter commands for that slot.
            try {
                if (this.interactiveVersion) {
                    const status = this.interactiveStatus;
                    const appliedIdentity = { session: BigInt(status[2]) | (BigInt(status[3]) << 32n), build: BigInt(status[4]) | (BigInt(status[5]) << 32n) };
                    if (message.version !== Ring.INTERACTIVE_WIRE_VERSION || !Ring.sameIdentity(message.identity, this.identity()) || !Ring.sameIdentity(message.identity, appliedIdentity) || !status[7]) throw new Error('Stale or unready insert preparation.');
                    const precedingCommands = this.commandRing ? Ring.commandOccupancy(this.commandRing.words) : 0;
                    const hasCommandRoom = this.wasm.pending_control_commands() + precedingCommands + 2 <= Ring.COMMAND_CAPACITY;
                    if (this.pendingReplay || this.fallbackBatches.length || this.pendingControlSerial || this.pendingAcks.length >= Ring.COMMAND_CAPACITY || !hasCommandRoom || (this.nextBarrierHigh === 0xffffffff && this.nextBarrierLow >= 0xfffffffe)) throw new Error('Audio controls are busy; retry the insert change.');
                    if (!['install', 'remove'].includes(message.action) || !Number.isInteger(message.target) || !(message.target >= 0 && message.target < 64 || message.target === Ring.INSERT_TARGET_MASTER) || !Number.isInteger(message.slot) || message.slot < 0 || message.slot >= 4 || (message.action === 'install' && (!Number.isInteger(message.kind) || message.kind < 0 || !JSON.parse(this.wasm.effects_json()).some(effect => effect.wire === message.kind)))) throw new Error('Invalid insert preparation fields.');
                    if (this.commandRing) Ring.drainCommands(this.commandRing, this.applyCommand, 62);
                    this.flushInteractive();
                }
                if (message.action === 'install') this.wasm.install_insert(message.target, message.slot, message.kind);
                else if (message.action === 'remove') this.wasm.remove_insert(message.target, message.slot);
                this.bindViews();
                const applied = { type: 'insertsApplied', requestId: message.requestId, identity: message.identity, action: message.action, target: message.target, slot: message.slot, kind: message.kind, memoryBytes: this.stableMemoryBytes };
                if (this.interactiveVersion) this.queueApplied(applied);
                else this.port.postMessage(applied);
            } catch (error) {
                this.port.postMessage({ type: 'insertsError', requestId: message.requestId, identity: message.identity, action: message.action, target: message.target, slot: message.slot, reason: error && error.message ? error.message : String(error) });
            }
        } else if (message.type === 'captureState') {
            if (this.pendingControlSerial || this.fallbackBatches.length) { this.port.postMessage({ type: 'stateBusy', requestId: message.requestId }); return; }
            if (!this.interactiveVersion || !Ring.sameIdentity(message.identity, this.identity())) {
                this.port.postMessage({ type: 'stateError', requestId: message.requestId, reason: 'Stale renderer state request.' }); return;
            }
            if (this.commandRing) Ring.drainCommands(this.commandRing, this.applyCommand, this.interactiveVersion ? 63 : Ring.COMMAND_CAPACITY);
            this.flushInteractive();
            this.queueApplied({ type: 'stateReady', requestId: message.requestId, identity: message.identity });
        } else if (message.type === 'readState') {
            try {
                if (!Ring.sameIdentity(message.identity, this.identity()) || this.wasm.applied_serial() < message.serial || !this.wasm.applied_accepted()) throw new Error('Stale or incomplete renderer state.');
                const replacement = this.wasm.replacement_state();
                this.bindViews();
                this.port.postMessage({ type: 'stateCaptured', requestId: message.requestId, identity: message.identity, replacement });
            } catch (error) { this.port.postMessage({ type: 'stateError', requestId: message.requestId, reason: String(error) }); }
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

        if (!this.pendingControlSerial) {
            if (this.commandRing !== null) Ring.drainCommands(this.commandRing, this.applyCommand, this.interactiveVersion ? 63 : Ring.COMMAND_CAPACITY);
            else if (this.fallbackBatches.length) {
                const batch = this.fallbackBatches.shift();
                for (let index = 0; index < batch.length; index += 1) this.applyCommand(...batch[index]);
            }
        }

        this.flushInteractive();
        this.wasm.process(frames);
        this.publishApplied();
        const replay = this.pendingReplay;
        if (replay && !replay.resumeSent && this.wasm.quanta_rendered() >= replay.waitUntil) { replay.resumeSent = true; this.port.postMessage(replay.resumeMessage); }
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
