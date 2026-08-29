// The wire protocol shared by the page and the AudioWorklet.
//
// This file is loaded twice: once by the page as an ordinary <script>, and once as part
// of the concatenated worklet bundle that `cargo xtask wasm` builds. A worklet cannot
// `import` or `fetch`, so sharing code with it means concatenation, and concatenation
// means this file must be a plain script that installs itself on `globalThis` rather
// than an ES module. That is why the protocol lives here and not inside `app.js`.
//
// ── Why a SharedArrayBuffer and not wasm linear memory ──────────────────────────────
//
// The obvious ring would live inside the wasm instance's own memory: the page writes,
// the worklet reads, nothing is copied. It does not work. Only the worklet instantiates
// the module, and its linear memory is visible to the page only if the module is built
// with *shared* memory — `-C target-feature=+atomics`, a rebuilt `std`, and a toolchain
// this project deliberately does not pin. So the cross-thread hop is a SharedArrayBuffer
// the page allocates and hands to the worklet once, at construction. The worklet drains
// it on the audio thread and pushes what it found into the Rust command ring, which is
// where the engine will read from in M1. Only the producer side moves.
//
// SharedArrayBuffer itself needs the document to be cross-origin isolated (COOP
// `same-origin` + COEP `require-corp`). When it is not, both directions fall back to
// `postMessage` — batched to at most one message per animation frame outbound, and one
// per 8 quanta inbound, never one per slider event.

(function (scope) {
    'use strict';

    // ── command ring: page → worklet ────────────────────────────────────────────────

    /** Header words, then `CAPACITY` records of `RECORD_WORDS` words each. */
    const COMMAND_HEADER_WORDS = 4;
    const COMMAND_RECORD_WORDS = 2;

    /** Must match `COMMAND_RING_CAPACITY` in `crates/starplayer-host-wasm/src/command.rs`. */
    const COMMAND_CAPACITY = 64;

    const COMMAND_WRITE_INDEX = 0;
    const COMMAND_READ_INDEX = 1;
    const COMMAND_CAPACITY_INDEX = 2;
    const COMMAND_OVERFLOW_INDEX = 3;

    /**
     * Indices are monotonic so that "empty" and "full" are distinguishable without
     * wasting a slot, and are wrapped well short of `Int32Array`'s range so the
     * subtraction below can never see a negative from overflow.
     */
    const COMMAND_INDEX_WRAP = COMMAND_CAPACITY * 1024;

    const OPCODE_SET_FREQUENCY = 1;

    const COMMAND_RING_BYTES = (COMMAND_HEADER_WORDS + COMMAND_CAPACITY * COMMAND_RECORD_WORDS) * 4;

    /**
     * Wraps a buffer in the two views the ring needs. The same words are read as `i32`
     * for the opcode and as `f32` for the payload, which is why both views exist over
     * one buffer rather than one view and a `DataView`.
     */
    function viewCommandRing(buffer) {
        return {
            buffer,
            words: new Int32Array(buffer),
            floats: new Float32Array(buffer),
        };
    }

    function createCommandRing() {
        const buffer = new SharedArrayBuffer(COMMAND_RING_BYTES);
        const ring = viewCommandRing(buffer);
        Atomics.store(ring.words, COMMAND_CAPACITY_INDEX, COMMAND_CAPACITY);
        return ring;
    }

    function commandOccupancy(words) {
        const write = Atomics.load(words, COMMAND_WRITE_INDEX);
        const read = Atomics.load(words, COMMAND_READ_INDEX);
        return (write - read + COMMAND_INDEX_WRAP) % COMMAND_INDEX_WRAP;
    }

    /**
     * Producer side, called on the page's thread. Returns false without blocking if the
     * ring is full — a slider cannot outrun 64 slots in one 2.7 ms quantum, so a full
     * ring means the audio thread has stopped, and the page says so.
     */
    function pushSetFrequency(ring, hertz) {
        const words = ring.words;
        if (commandOccupancy(words) >= COMMAND_CAPACITY) {
            Atomics.add(words, COMMAND_OVERFLOW_INDEX, 1);
            return false;
        }
        const write = Atomics.load(words, COMMAND_WRITE_INDEX);
        const base = COMMAND_HEADER_WORDS + (write % COMMAND_CAPACITY) * COMMAND_RECORD_WORDS;
        words[base] = OPCODE_SET_FREQUENCY;
        ring.floats[base + 1] = hertz;
        // Sequentially consistent, so the record above is published before the index that
        // makes it visible to the consumer.
        Atomics.store(words, COMMAND_WRITE_INDEX, (write + 1) % COMMAND_INDEX_WRAP);
        return true;
    }

    /**
     * Consumer side, called on the audio thread. Applies every queued record through
     * `apply(opcode, value)` and returns how many it applied. Bounded by the capacity,
     * allocates nothing.
     */
    function drainCommands(ring, apply) {
        const words = ring.words;
        const write = Atomics.load(words, COMMAND_WRITE_INDEX);
        let read = Atomics.load(words, COMMAND_READ_INDEX);
        let applied = 0;
        while (read !== write && applied <= COMMAND_CAPACITY) {
            const base = COMMAND_HEADER_WORDS + (read % COMMAND_CAPACITY) * COMMAND_RECORD_WORDS;
            apply(words[base], ring.floats[base + 1]);
            read = (read + 1) % COMMAND_INDEX_WRAP;
            applied += 1;
        }
        Atomics.store(words, COMMAND_READ_INDEX, read);
        return applied;
    }

    function commandOverflows(ring) {
        return Atomics.load(ring.words, COMMAND_OVERFLOW_INDEX);
    }

    // ── telemetry block: worklet → page ─────────────────────────────────────────────
    //
    // Architecture §9 calls peak levels a *lossy audio tap*: the reader may see a stale
    // or torn value and it does not matter. A publication counter is still written last
    // so the page can tell "no update yet" from "updated to zero", and so a future scope
    // buffer can be added behind the same fence.

    const TELEMETRY_SEQUENCE_INDEX = 0;
    const TELEMETRY_QUANTUM_FRAMES_INDEX = 1;
    const TELEMETRY_DROPPED_COMMANDS_INDEX = 2;
    const TELEMETRY_MEMORY_BYTES_INDEX = 3;
    const TELEMETRY_PEAK_INDEX = 4;
    const TELEMETRY_FREQUENCY_INDEX = 5;
    const TELEMETRY_QUANTA_RENDERED_INDEX = 6;
    const TELEMETRY_COMMANDS_APPLIED_INDEX = 7;
    const TELEMETRY_WORDS = 8;

    const TELEMETRY_BYTES = TELEMETRY_WORDS * 4;

    function viewTelemetry(buffer) {
        return {
            buffer,
            words: new Int32Array(buffer),
            floats: new Float32Array(buffer),
        };
    }

    function createTelemetry() {
        return viewTelemetry(new SharedArrayBuffer(TELEMETRY_BYTES));
    }

    /** Audio-thread side. No allocation: every field is a store into the shared block. */
    function publishTelemetry(telemetry, snapshot) {
        const words = telemetry.words;
        const floats = telemetry.floats;
        words[TELEMETRY_QUANTUM_FRAMES_INDEX] = snapshot.quantumFrames;
        words[TELEMETRY_DROPPED_COMMANDS_INDEX] = snapshot.droppedCommands;
        words[TELEMETRY_MEMORY_BYTES_INDEX] = snapshot.memoryBytes;
        floats[TELEMETRY_PEAK_INDEX] = snapshot.peak;
        floats[TELEMETRY_FREQUENCY_INDEX] = snapshot.frequency;
        floats[TELEMETRY_QUANTA_RENDERED_INDEX] = snapshot.quantaRendered;
        floats[TELEMETRY_COMMANDS_APPLIED_INDEX] = snapshot.commandsApplied;
        Atomics.add(words, TELEMETRY_SEQUENCE_INDEX, 1);
    }

    /** Page side, called from an animation frame. */
    function readTelemetry(telemetry) {
        const words = telemetry.words;
        const floats = telemetry.floats;
        return {
            sequence: Atomics.load(words, TELEMETRY_SEQUENCE_INDEX),
            quantumFrames: words[TELEMETRY_QUANTUM_FRAMES_INDEX],
            droppedCommands: words[TELEMETRY_DROPPED_COMMANDS_INDEX],
            memoryBytes: words[TELEMETRY_MEMORY_BYTES_INDEX],
            peak: floats[TELEMETRY_PEAK_INDEX],
            frequency: floats[TELEMETRY_FREQUENCY_INDEX],
            quantaRendered: floats[TELEMETRY_QUANTA_RENDERED_INDEX],
            commandsApplied: floats[TELEMETRY_COMMANDS_APPLIED_INDEX],
        };
    }

    /** Quanta between `postMessage` telemetry posts when there is no shared memory. */
    const TELEMETRY_FALLBACK_QUANTA = 8;

    scope.StarPlayerRing = {
        OPCODE_SET_FREQUENCY,
        COMMAND_CAPACITY,
        COMMAND_RING_BYTES,
        TELEMETRY_BYTES,
        TELEMETRY_FALLBACK_QUANTA,
        createCommandRing,
        viewCommandRing,
        pushSetFrequency,
        drainCommands,
        commandOccupancy,
        commandOverflows,
        createTelemetry,
        viewTelemetry,
        publishTelemetry,
        readTelemetry,
    };
})(globalThis);
