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

    // Arguments for OPCODE_AT_END; `extra` carries the fade length in frames.
    const AT_END_FADE_OUT = 0;
    const AT_END_CONTINUE = 1;
    const AT_END_STOP = 2;

    function viewCommandRing(buffer) {
        return { buffer, words: new Int32Array(buffer) };
    }

    function createCommandRing() {
        const ring = viewCommandRing(new SharedArrayBuffer(COMMAND_RING_BYTES));
        Atomics.store(ring.words, COMMAND_CAPACITY_INDEX, COMMAND_CAPACITY);
        return ring;
    }

    function commandOccupancy(words) {
        const write = Atomics.load(words, COMMAND_WRITE_INDEX);
        const read = Atomics.load(words, COMMAND_READ_INDEX);
        return (write - read + COMMAND_INDEX_WRAP) % COMMAND_INDEX_WRAP;
    }

    function pushCommand(ring, opcode, argument, extra) {
        const words = ring.words;
        if (commandOccupancy(words) >= COMMAND_CAPACITY) {
            Atomics.add(words, COMMAND_OVERFLOW_INDEX, 1);
            return false;
        }
        const write = Atomics.load(words, COMMAND_WRITE_INDEX);
        const base = COMMAND_HEADER_WORDS + (write % COMMAND_CAPACITY) * COMMAND_RECORD_WORDS;
        words[base] = opcode;
        words[base + 1] = argument;
        words[base + 2] = extra;
        Atomics.store(words, COMMAND_WRITE_INDEX, (write + 1) % COMMAND_INDEX_WRAP);
        return true;
    }

    /** Consumer side: bounded, allocation-free, and called once at each render quantum. */
    function drainCommands(ring, apply) {
        const words = ring.words;
        const write = Atomics.load(words, COMMAND_WRITE_INDEX);
        let read = Atomics.load(words, COMMAND_READ_INDEX);
        let applied = 0;
        while (read !== write && applied < COMMAND_CAPACITY) {
            const base = COMMAND_HEADER_WORDS + (read % COMMAND_CAPACITY) * COMMAND_RECORD_WORDS;
            apply(words[base], words[base + 1], words[base + 2]);
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
    function drainFallbackCommands(records, apply) {
        let applied = 0;
        for (let index = 0; index + 2 < records.length && applied < COMMAND_CAPACITY; index += 3) {
            apply(records[index], records[index + 1], records[index + 2]);
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
    const SNAPSHOT_WORDS = SNAPSHOT_HEADER_WORDS + SNAPSHOT_CHANNELS * SNAPSHOT_CHANNEL_WORDS;
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
            playing: source[15] !== 0,
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
