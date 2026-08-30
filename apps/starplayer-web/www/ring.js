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
    const SNAPSHOT_HEADER_WORDS = 19;
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
        SNAPSHOT_WORDS,
        TELEMETRY_BYTES,
        TELEMETRY_FALLBACK_QUANTA,
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
    };
})(globalThis);
