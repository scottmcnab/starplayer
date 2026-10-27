// Typed command and coherent telemetry protocol shared by the page and worklet bundle.
// Plain script by design: `xtask wasm` concatenates it into the A4 single-file worklet.

(function (scope) {
    'use strict';

    const COMMAND_HEADER_WORDS = 4;
    const COMMAND_RECORD_WORDS = 3;
    const COMMAND_CAPACITY = 64;
    const COMMAND_INDEX_WRAP = COMMAND_CAPACITY * 1024;
    const COMMAND_WRITE_INDEX = 0;
    const COMMAND_READ_INDEX = 1;
    const COMMAND_CAPACITY_INDEX = 2;
    const COMMAND_OVERFLOW_INDEX = 3;
    const COMMAND_RING_BYTES = (COMMAND_HEADER_WORDS + COMMAND_CAPACITY * COMMAND_RECORD_WORDS) * 4;

    const OPCODE_PLAY = 1;
    const OPCODE_STOP = 2;
    const OPCODE_SEEK_ORDER = 3;
    const OPCODE_SEEK_ROW = 4;
    const OPCODE_MASTER_VOLUME = 5;
    const OPCODE_MUTE_CHANNEL = 6;
    const OPCODE_SET_MIXER_MODE = 7;
    const OPCODE_SEEK_FRAME = 8;
    const OPCODE_AT_END = 9;
    // One live MIDI channel-voice message (task E6): Web MIDI, or the page's own
    // tracker-style keyboard. `argument` packs the three bytes a browser already hands
    // over framed — status in bits 0-7, data1 in 8-15, data2 in 16-23 — and `extra` is
    // unused. It rides this ring rather than a second one because it is an ordinary
    // command: bounded, drained between render quanta, and turned into one push onto the
    // engine's live-input queue. Installing that queue is a `midiInput` port message
    // instead, because building the instrument rack allocates.
    const OPCODE_MIDI_EVENT = 10;
    // Change one parameter of an already-installed insert effect (M7-H7). `argument`
    // packs the target (bits 0-7), the slot (bits 8-11) and the ParamId (bits 12-19);
    // `extra` is the new value in the parameter's own fixed unit — some are negative
    // (a compressor threshold in centi-dB), which crosses exactly as a positive one does,
    // since the ring and `enqueue_command` both carry raw 32-bit patterns. Rides this
    // ring for the same reason OPCODE_MIDI_EVENT does: bounded, decoded between render
    // quanta, one allocation-free `Player::set_insert_param` call.
    const OPCODE_INSERT_PARAM = 11;
    // Skip, or stop skipping, an already-installed insert effect. `argument` packs the
    // same target and slot as OPCODE_INSERT_PARAM; `extra` is 0 or 1.
    const OPCODE_INSERT_BYPASS = 12;
    // The reserved target byte naming the master bus, rather than a channel index.
    const INSERT_TARGET_MASTER = 0xFF;

    // Arguments for OPCODE_AT_END; `extra` carries the fade length in frames.
    const AT_END_FADE_OUT = 0;
    const AT_END_CONTINUE = 1;
    const AT_END_STOP = 2;

    /** Pack a channel-voice message into one wire `argument`. Bytes are masked to seven
     *  bits below the status byte, so a malformed message cannot smear into the next
     *  field. */
    function packMidiMessage(status, data1, data2) {
        return ((status & 0xFF) | ((data1 & 0x7F) << 8) | ((data2 & 0x7F) << 16)) >>> 0;
    }

    /** Pack an insert target and slot into the low 12 bits OPCODE_INSERT_BYPASS reads. */
    function packInsertSlotArgument(target, slot) {
        return ((target & 0xFF) | ((slot & 0xF) << 8)) >>> 0;
    }

    /** Pack an insert target, slot and ParamId into the `argument` OPCODE_INSERT_PARAM
     *  reads. */
    function packInsertParamArgument(target, slot, paramId) {
        return ((target & 0xFF) | ((slot & 0xF) << 8) | ((paramId & 0xFF) << 12)) >>> 0;
    }

    const INTERACTIVE_WIRE_VERSION = 2;
    const INTERACTIVE_RECORD_WORDS = 7;
    const INTERACTIVE_HEADER_WORDS = 5;
    const INTERACTIVE_RING_BYTES = (INTERACTIVE_HEADER_WORDS + COMMAND_CAPACITY * INTERACTIVE_RECORD_WORDS) * 4;
    const OPCODE_INTERACTIVE_PLAY = 13, OPCODE_PAUSE = 14, OPCODE_RESUME = 15, OPCODE_HOLD_STOP = 16;
    const OPCODE_ALL_SOUND_OFF = 17, OPCODE_STOP_PREVIEW = 18, OPCODE_LIVE_DESTINATION = 19;
    const OPCODE_PREVIEW = 20, OPCODE_RELEASE_PREVIEW = 21, OPCODE_NOTE_OVERLAP = 22, OPCODE_LET_NOTES_RING = 23, OPCODE_LIVE_MIDI = 24;
    const MAX_U64 = (1n << 64n) - 1n;
    function validWireWord(value) { return Number.isInteger(value) && value >= -2147483648 && value <= 0xffffffff; }
    function validIdentity(identity) {
        return !!identity && typeof identity.session === 'bigint' && typeof identity.build === 'bigint'
            && identity.session >= 0n && identity.session <= MAX_U64 && identity.build >= 0n && identity.build <= MAX_U64;
    }
    function sameIdentity(left, right) { return validIdentity(left) && validIdentity(right) && left.session === right.session && left.build === right.build; }
    function validInteractive(opcode, argument, extra, identity) {
        if (!validIdentity(identity) || !Number.isInteger(argument) || argument < 0 || argument > 0xffffffff || !Number.isInteger(extra) || extra < 0 || extra > 0xffffffff) return false;
        return validInteractiveArguments(opcode, argument, extra);
    }
    function validInteractiveArguments(opcode, argument, extra) {
        if ((opcode >= 13 && opcode <= 18) || opcode === 21) return argument === 0 && extra === 0;
        if (opcode === 19) return argument <= 65537 && extra === 0;
        if (opcode === 20) return argument <= 65535 && extra === 0;
        if (opcode === 22) return argument <= 1 && extra === 0;
        if (opcode === 23) return argument <= 65535 && extra <= 1;
        if (opcode === 24) return extra === 0 && (argument & 0xff808000) === 0 && (argument & 0xff) >= 0x80 && (argument & 0xff) < 0xf0;
        return false;
    }
    function viewCommandRing(buffer) {
        const words = new Int32Array(buffer);
        const version = buffer.byteLength === COMMAND_RING_BYTES ? 1
            : buffer.byteLength === INTERACTIVE_RING_BYTES && Atomics.load(words, 4) === INTERACTIVE_WIRE_VERSION ? INTERACTIVE_WIRE_VERSION : 0;
        return { buffer, words, version, headerWords: version === 2 ? INTERACTIVE_HEADER_WORDS : COMMAND_HEADER_WORDS, recordWords: version === 2 ? INTERACTIVE_RECORD_WORDS : COMMAND_RECORD_WORDS };
    }

    function createCommandRing(version = 1) {
        const buffer = new SharedArrayBuffer(version === 2 ? INTERACTIVE_RING_BYTES : COMMAND_RING_BYTES);
        if (version === 2) Atomics.store(new Int32Array(buffer), 4, version);
        const ring = viewCommandRing(buffer);
        Atomics.store(ring.words, COMMAND_CAPACITY_INDEX, COMMAND_CAPACITY);
        return ring;
    }

    function commandOccupancy(words) {
        const write = Atomics.load(words, COMMAND_WRITE_INDEX);
        const read = Atomics.load(words, COMMAND_READ_INDEX);
        return (write - read + COMMAND_INDEX_WRAP) % COMMAND_INDEX_WRAP;
    }

    function pushCommand(ring, opcode, argument, extra, identity = null) {
        if (!ring.version || (ring.version === 2 && (!validIdentity(identity) || !validWireWord(argument) || !validWireWord(extra) || !Number.isInteger(opcode) || opcode < 1 || opcode > 24)) || (opcode >= 13 && (ring.version !== 2 || !validInteractive(opcode, argument, extra, identity)))) return false;
        const words = ring.words;
        if (commandOccupancy(words) >= COMMAND_CAPACITY) {
            Atomics.add(words, COMMAND_OVERFLOW_INDEX, 1);
            return false;
        }
        const write = Atomics.load(words, COMMAND_WRITE_INDEX);
        const base = ring.headerWords + (write % COMMAND_CAPACITY) * ring.recordWords;
        words[base] = opcode;
        words[base + 1] = argument;
        words[base + 2] = extra;
        if (ring.version === 2) {
            const session = identity?.session ?? 0n, build = identity?.build ?? 0n;
            words[base + 3] = Number(session & 0xffffffffn); words[base + 4] = Number(session >> 32n);
            words[base + 5] = Number(build & 0xffffffffn); words[base + 6] = Number(build >> 32n);
        }
        Atomics.store(words, COMMAND_WRITE_INDEX, (write + 1) % COMMAND_INDEX_WRAP);
        return true;
    }

    /** Consumer side: bounded, allocation-free, and called once at each render quantum. */
    function drainCommands(ring, apply, limit = COMMAND_CAPACITY) {
        if (!ring.version) return 0;
        const words = ring.words;
        const write = Atomics.load(words, COMMAND_WRITE_INDEX);
        let read = Atomics.load(words, COMMAND_READ_INDEX);
        let applied = 0;
        while (read !== write && applied < Math.min(COMMAND_CAPACITY, limit)) {
            const base = ring.headerWords + (read % COMMAND_CAPACITY) * ring.recordWords;
            const opcode = words[base];
            if (ring.version === 2) {
                if ((opcode > 0 && opcode < 13) || validInteractiveArguments(opcode, words[base + 1] >>> 0, words[base + 2] >>> 0)) apply(opcode, words[base + 1] >>> 0, words[base + 2] >>> 0, words[base + 3] >>> 0, words[base + 4] >>> 0, words[base + 5] >>> 0, words[base + 6] >>> 0);
            } else if (opcode < 13) apply(opcode, words[base + 1], words[base + 2]);
            read = (read + 1) % COMMAND_INDEX_WRAP;
            applied += 1;
        }
        Atomics.store(words, COMMAND_READ_INDEX, read);
        return applied;
    }

    function commandOverflows(ring) {
        return Atomics.load(ring.words, COMMAND_OVERFLOW_INDEX);
    }

    // Fallback batches are flat triples so one animation frame produces one message,
    // regardless of how many controls changed during that frame.
    function drainFallbackCommands(records, apply, version = 1) {
        if (!Array.isArray(records)) return 0;
        if (records.length && typeof records[0] === 'object') {
            if (version !== INTERACTIVE_WIRE_VERSION) return 0;
            let applied = 0;
            for (const record of records.slice(0, COMMAND_CAPACITY)) {
                if (record && validWireWord(record.argument) && validWireWord(record.extra) && validIdentity(record.identity) && ((record.opcode > 0 && record.opcode < 13 && Number.isInteger(record.opcode)) || validInteractive(record.opcode, record.argument, record.extra, record.identity))) { apply(record.opcode, record.argument >>> 0, record.extra >>> 0, Number(record.identity.session & 0xffffffffn), Number(record.identity.session >> 32n), Number(record.identity.build & 0xffffffffn), Number(record.identity.build >> 32n)); applied += 1; }
            }
            return applied;
        }
        if (version === INTERACTIVE_WIRE_VERSION) return 0;
        let applied = 0;
        for (let index = 0; index + 2 < records.length && applied < COMMAND_CAPACITY; index += 3) {
            if (records[index] < 13) apply(records[index], records[index + 1], records[index + 2]);
            applied += 1;
        }
        return applied;
    }

    // Rust's packed Snapshot layout. The SAB adds a seqlock and diagnostics ahead of it.
    const SNAPSHOT_HEADER_WORDS = 22;

    // Bits of the song_flags word (index 21).
    const SONG_FLAG_LENGTH_KNOWN = 1;
    // Set only when the song comes round through its own flow — a Bxx/Cxx/Dxx jump back
    // into music already played. An order list that simply runs out is an *end*, and
    // leaves this clear (task D2).
    const SONG_FLAG_LOOPS = 2;
    const SONG_FLAG_END_REACHED = 4;
    const SONG_FLAG_FADING = 8;
    const SNAPSHOT_CHANNEL_WORDS = 8;
    const SNAPSHOT_CHANNELS = 64;
    const SNAPSHOT_LIVE_OFFSET = SNAPSHOT_HEADER_WORDS + SNAPSHOT_CHANNELS * SNAPSHOT_CHANNEL_WORDS;
    const SNAPSHOT_WORDS = SNAPSHOT_LIVE_OFFSET + 3;
    const TELEMETRY_SEQUENCE_INDEX = 0;
    const TELEMETRY_MEMORY_INDEX = 1;
    const TELEMETRY_QUANTUM_INDEX = 2;
    const TELEMETRY_DROPPED_INDEX = 3;
    const TELEMETRY_QUANTA_INDEX = 4;
    const TELEMETRY_SOURCE_OFFSET = 8;
    const TELEMETRY_WORDS = TELEMETRY_SOURCE_OFFSET + SNAPSHOT_WORDS;
    const TELEMETRY_BYTES = TELEMETRY_WORDS * 4;
    const TELEMETRY_FALLBACK_QUANTA = 8;

    function viewTelemetry(buffer) {
        return { buffer, words: new Int32Array(buffer) };
    }

    function createTelemetry() {
        return viewTelemetry(new SharedArrayBuffer(TELEMETRY_BYTES));
    }

    /** Worklet side. The odd/even seqlock makes the 2 KB scalar snapshot coherent. */
    function publishTelemetry(telemetry, sourceWords, memoryBytes, quantumFrames, droppedCommands, quantaRendered) {
        const words = telemetry.words;
        const previous = Atomics.load(words, TELEMETRY_SEQUENCE_INDEX);
        const writing = (previous & 1) === 0 ? previous + 1 : previous + 2;
        Atomics.store(words, TELEMETRY_SEQUENCE_INDEX, writing);
        words[TELEMETRY_MEMORY_INDEX] = memoryBytes;
        words[TELEMETRY_QUANTUM_INDEX] = quantumFrames;
        words[TELEMETRY_DROPPED_INDEX] = droppedCommands;
        words[TELEMETRY_QUANTA_INDEX] = quantaRendered;
        words.set(sourceWords, TELEMETRY_SOURCE_OFFSET);
        Atomics.store(words, TELEMETRY_SEQUENCE_INDEX, writing + 1);
    }

    function decodeSnapshot(source, diagnostics) {
        const channelCount = Math.max(0, Math.min(SNAPSHOT_CHANNELS, source[2]));
        const channels = [];
        for (let index = 0; index < channelCount; index += 1) {
            const base = SNAPSHOT_HEADER_WORDS + index * SNAPSHOT_CHANNEL_WORDS;
            channels.push({
                note: source[base] < 0 ? null : source[base],
                instrument: source[base + 1],
                volume: source[base + 2],
                pan: source[base + 3],
                effectCode: source[base + 4],
                effectParam: source[base + 5],
                vu: source[base + 6],
                active: (source[base + 7] & 1) !== 0,
                muted: (source[base + 7] & 2) !== 0,
            });
        }
        return {
            sequence: source[0],
            publishesDropped: source[1],
            channelCount,
            voicesActive: source[3],
            order: source[4],
            pattern: source[5],
            row: source[6],
            tick: source[7],
            speed: source[8],
            bpm: source[9],
            globalVolume: source[10],
            warnings: source[11],
            engineFrame: source[12],
            pendingGarbage: source[13],
            moduleGeneration: source[14],
            playing: (source[15] & 8) ? (source[15] & 1) !== 0 : source[15] !== 0,
            transport: (source[15] & 8) ? ((source[15] & 1) ? 'playing' : (source[15] & 2) ? 'paused' : 'stopped') : (source[15] ? 'playing' : 'stopped'),
            naturalEnd: (source[15] & 8) ? (source[15] & 4) !== 0 : undefined,
            live: { active: source[SNAPSHOT_LIVE_OFFSET] || 0, retiring: source[SNAPSHOT_LIVE_OFFSET + 1] || 0, peak: source[SNAPSHOT_LIVE_OFFSET + 2] || 0 },
            masterPeak: source[16],
            retiredCollected: source[17],
            mixerModeWire: source[18] >>> 0,
            songFrame: source[19],
            songLengthFrames: source[20],
            songFlags: source[21],
            channels,
            memoryBytes: diagnostics.memoryBytes,
            quantumFrames: diagnostics.quantumFrames,
            droppedCommands: diagnostics.droppedCommands,
            quantaRendered: diagnostics.quantaRendered,
        };
    }

    /** Page side. Retry if the worklet published while the copy was in flight. */
    function readTelemetry(telemetry) {
        const words = telemetry.words;
        for (let attempt = 0; attempt < 3; attempt += 1) {
            const before = Atomics.load(words, TELEMETRY_SEQUENCE_INDEX);
            if ((before & 1) !== 0 || before === 0) {
                continue;
            }
            const source = words.slice(TELEMETRY_SOURCE_OFFSET, TELEMETRY_SOURCE_OFFSET + SNAPSHOT_WORDS);
            const diagnostics = {
                memoryBytes: words[TELEMETRY_MEMORY_INDEX],
                quantumFrames: words[TELEMETRY_QUANTUM_INDEX],
                droppedCommands: words[TELEMETRY_DROPPED_INDEX],
                quantaRendered: words[TELEMETRY_QUANTA_INDEX],
            };
            const after = Atomics.load(words, TELEMETRY_SEQUENCE_INDEX);
            if (before === after && (after & 1) === 0) {
                return decodeSnapshot(source, diagnostics);
            }
        }
        return null;
    }

    /** Worklet fallback side; allocation is accepted only on the non-SAB path. */
    function decodeWasmTelemetry(sourceWords, memoryBytes, quantumFrames, droppedCommands, quantaRendered) {
        return decodeSnapshot(sourceWords, { memoryBytes, quantumFrames, droppedCommands, quantaRendered });
    }

    // ── scope taps: architecture §9(b), the lossy half of telemetry ─────────────────
    //
    // Deliberately *not* the coherent snapshot's protocol. A scope is a picture of a
    // waveform: if the worklet republishes while the page is drawing, the trace has a
    // seam a few milliseconds wide that nobody can see. So this block is its own
    // SharedArrayBuffer, the page reads the values straight out of it with no copy, and
    // the sequence word exists only so a reader can tell "nothing published yet" from
    // "silence". The engine already downsampled: 4 output frames per bucket, so a whole
    // render quantum is 32 buckets rather than 128 frames.
    const SCOPE_CHANNELS = 64;
    const SCOPE_WINDOW_BUCKETS = 256;
    const SCOPE_HEADER_WORDS = 8;
    const SCOPE_SEQUENCE_INDEX = 0;
    const SCOPE_GENERATION_INDEX = 1;
    const SCOPE_CHANNEL_COUNT_INDEX = 2;
    const SCOPE_BUCKET_FRAMES_INDEX = 3;
    const SCOPE_WINDOW_INDEX = 4;
    const SCOPE_INDEX_WORDS = SCOPE_HEADER_WORDS + SCOPE_CHANNELS;
    const SCOPE_VALUES_BYTE_OFFSET = SCOPE_INDEX_WORDS * 4;
    const SCOPE_BYTES = SCOPE_VALUES_BYTE_OFFSET + SCOPE_CHANNELS * SCOPE_WINDOW_BUCKETS * 2;

    function viewScope(buffer) {
        return {
            buffer,
            words: new Int32Array(buffer, 0, SCOPE_INDEX_WORDS),
            values: new Int16Array(buffer, SCOPE_VALUES_BYTE_OFFSET, SCOPE_CHANNELS * SCOPE_WINDOW_BUCKETS),
        };
    }

    function createScope() {
        return viewScope(new SharedArrayBuffer(SCOPE_BYTES));
    }

    /** Worklet side. One copy per *refresh*, not per quantum: the wasm block only moves
     *  every few quanta, and the generation word says when. */
    function publishScope(scope, sourceValues, sourceIndices, channelCount, bucketFrames, generation) {
        const words = scope.words;
        const channels = Math.max(0, Math.min(SCOPE_CHANNELS, channelCount));
        const previous = Atomics.load(words, SCOPE_SEQUENCE_INDEX);
        const writing = (previous & 1) === 0 ? previous + 1 : previous + 2;
        Atomics.store(words, SCOPE_SEQUENCE_INDEX, writing);
        words[SCOPE_GENERATION_INDEX] = generation;
        words[SCOPE_CHANNEL_COUNT_INDEX] = channels;
        words[SCOPE_BUCKET_FRAMES_INDEX] = bucketFrames;
        words[SCOPE_WINDOW_INDEX] = SCOPE_WINDOW_BUCKETS;
        if (channels > 0) {
            scope.values.set(sourceValues.subarray(0, channels * SCOPE_WINDOW_BUCKETS));
            words.set(sourceIndices.subarray(0, channels), SCOPE_HEADER_WORDS);
        }
        Atomics.store(words, SCOPE_SEQUENCE_INDEX, writing + 1);
    }

    /** Page side. Returns a *view*, not a copy — tearing is the design (see above). */
    function readScope(scope) {
        const words = scope.words;
        if (Atomics.load(words, SCOPE_SEQUENCE_INDEX) === 0) {
            return null;
        }
        return {
            generation: words[SCOPE_GENERATION_INDEX],
            channelCount: words[SCOPE_CHANNEL_COUNT_INDEX],
            bucketFrames: words[SCOPE_BUCKET_FRAMES_INDEX],
            windowBuckets: words[SCOPE_WINDOW_INDEX] || SCOPE_WINDOW_BUCKETS,
            values: scope.values,
            indices: words.subarray(SCOPE_HEADER_WORDS, SCOPE_INDEX_WORDS),
        };
    }

    /** Worklet fallback side: one copy of the active channels, riding the same batched
     *  telemetry message the snapshot already goes in. 32 channels is 16 KB — an eighth
     *  of what a whole-ring transfer would cost, and the message is posted once every
     *  TELEMETRY_FALLBACK_QUANTA quanta rather than every one. */
    function decodeWorkletScope(sourceValues, sourceIndices, channelCount, bucketFrames, generation) {
        const channels = Math.max(0, Math.min(SCOPE_CHANNELS, channelCount));
        return {
            generation,
            channelCount: channels,
            bucketFrames,
            windowBuckets: SCOPE_WINDOW_BUCKETS,
            values: sourceValues.slice(0, channels * SCOPE_WINDOW_BUCKETS),
            indices: sourceIndices.slice(0, channels),
        };
    }

    scope.StarPlayerRing = {
        INTERACTIVE_WIRE_VERSION, INTERACTIVE_RING_BYTES, validWireWord, validIdentity, sameIdentity, validInteractive,
        OPCODE_INTERACTIVE_PLAY, OPCODE_PAUSE, OPCODE_RESUME, OPCODE_HOLD_STOP, OPCODE_ALL_SOUND_OFF, OPCODE_STOP_PREVIEW, OPCODE_LIVE_DESTINATION, OPCODE_PREVIEW, OPCODE_RELEASE_PREVIEW, OPCODE_NOTE_OVERLAP, OPCODE_LET_NOTES_RING, OPCODE_LIVE_MIDI,
        COMMAND_CAPACITY,
        COMMAND_RING_BYTES,
        OPCODE_PLAY,
        OPCODE_STOP,
        OPCODE_SEEK_ORDER,
        OPCODE_SEEK_ROW,
        OPCODE_MASTER_VOLUME,
        OPCODE_MUTE_CHANNEL,
        OPCODE_SET_MIXER_MODE,
        OPCODE_SEEK_FRAME,
        OPCODE_AT_END,
        OPCODE_MIDI_EVENT,
        OPCODE_INSERT_PARAM,
        OPCODE_INSERT_BYPASS,
        INSERT_TARGET_MASTER,
        AT_END_FADE_OUT,
        AT_END_CONTINUE,
        AT_END_STOP,
        SONG_FLAG_LENGTH_KNOWN,
        SONG_FLAG_LOOPS,
        SONG_FLAG_END_REACHED,
        SONG_FLAG_FADING,
        SNAPSHOT_WORDS,
        TELEMETRY_BYTES,
        TELEMETRY_FALLBACK_QUANTA,
        SCOPE_BYTES,
        SCOPE_CHANNELS,
        SCOPE_WINDOW_BUCKETS,
        viewCommandRing,
        createCommandRing,
        packMidiMessage,
        packInsertSlotArgument,
        packInsertParamArgument,
        pushCommand,
        drainCommands,
        drainFallbackCommands,
        commandOccupancy,
        commandOverflows,
        viewTelemetry,
        createTelemetry,
        publishTelemetry,
        readTelemetry,
        decodeWasmTelemetry,
        viewScope,
        createScope,
        publishScope,
        readScope,
        decodeWorkletScope,
    };
})(globalThis);
