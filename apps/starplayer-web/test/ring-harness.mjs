import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const source = await readFile(new URL('../www/ring.js', import.meta.url), 'utf8');
vm.runInThisContext(source, { filename: 'ring.js' });
const Ring = globalThis.StarPlayerRing;

const commands = Ring.createCommandRing();
assert.equal(Ring.pushCommand(commands, Ring.OPCODE_PLAY, 0, 0), true);
assert.equal(Ring.pushCommand(commands, Ring.OPCODE_SEEK_ORDER, 17, 0), true);
assert.equal(Ring.pushCommand(commands, Ring.OPCODE_MASTER_VOLUME, 32768, 0), true);
assert.equal(Ring.pushCommand(commands, Ring.OPCODE_SET_MIXER_MODE, 0x0000_0261, 0), true);
assert.equal(Ring.pushCommand(commands, Ring.OPCODE_SEEK_FRAME, 441000, 0), true);
assert.equal(Ring.pushCommand(commands, Ring.OPCODE_AT_END, Ring.AT_END_FADE_OUT, 480000), true);
const received = [];
assert.equal(Ring.drainCommands(commands, (...record) => received.push(record)), 6);
assert.deepEqual(received, [
    [Ring.OPCODE_PLAY, 0, 0],
    [Ring.OPCODE_SEEK_ORDER, 17, 0],
    [Ring.OPCODE_MASTER_VOLUME, 32768, 0],
    [Ring.OPCODE_SET_MIXER_MODE, 0x0000_0261, 0],
    [Ring.OPCODE_SEEK_FRAME, 441000, 0],
    [Ring.OPCODE_AT_END, Ring.AT_END_FADE_OUT, 480000],
]);

const sourceWords = new Int32Array(Ring.SNAPSHOT_WORDS);
sourceWords[0] = 42;
sourceWords[2] = 2;
sourceWords[3] = 1;
sourceWords[4] = 3;
sourceWords[5] = 7;
sourceWords[6] = 12;
sourceWords[8] = 6;
sourceWords[9] = 125;
sourceWords[16] = 40000;
sourceWords[17] = 2;
sourceWords[18] = 0x0000_0261;
sourceWords[19] = 441000;
sourceWords[20] = 8_820_000;
sourceWords[21] = Ring.SONG_FLAG_LENGTH_KNOWN | Ring.SONG_FLAG_LOOPS;
// The first channel's eight words start straight after the 22-word header.
sourceWords[22] = 48;
sourceWords[23] = 1;
sourceWords[24] = 65535;
sourceWords[26] = 8;
sourceWords[27] = 0x42;
sourceWords[28] = 50000;
sourceWords[29] = 1;

const telemetry = Ring.createTelemetry();
Ring.publishTelemetry(telemetry, sourceWords, 18_000_000, 128, 0, 99);
const snapshot = Ring.readTelemetry(telemetry);
assert.equal(snapshot.sequence, 42);
assert.equal(snapshot.order, 3);
assert.equal(snapshot.pattern, 7);
assert.equal(snapshot.row, 12);
assert.equal(snapshot.masterPeak, 40000);
assert.equal(snapshot.retiredCollected, 2);
assert.equal(snapshot.mixerModeWire, 0x0000_0261);
assert.equal(snapshot.songFrame, 441000);
assert.equal(snapshot.songLengthFrames, 8_820_000);
assert.equal(snapshot.songFlags & Ring.SONG_FLAG_LENGTH_KNOWN, Ring.SONG_FLAG_LENGTH_KNOWN);
assert.equal(snapshot.songFlags & Ring.SONG_FLAG_LOOPS, Ring.SONG_FLAG_LOOPS);
assert.equal(snapshot.songFlags & Ring.SONG_FLAG_END_REACHED, 0);
assert.equal(snapshot.songFlags & Ring.SONG_FLAG_FADING, 0);
assert.equal(snapshot.channels[0].note, 48);
assert.equal(snapshot.channels[0].effectCode, 8);
assert.equal(snapshot.memoryBytes, 18_000_000);
assert.equal(snapshot.quantumFrames, 128);

// An odd publication sequence means a write is in flight and must never be decoded.
Atomics.add(telemetry.words, 0, 1);
assert.equal(Ring.readTelemetry(telemetry), null);
Atomics.add(telemetry.words, 0, 1);
assert.equal(Ring.readTelemetry(telemetry).sequence, 42);

// ── the scope taps (architecture 9(b)) ──────────────────────────────────────────────

const scope = Ring.createScope();
assert.equal(Ring.readScope(scope), null, 'nothing published yet reads as absent, not as silence');

const scopeValues = new Int16Array(Ring.SCOPE_CHANNELS * Ring.SCOPE_WINDOW_BUCKETS);
const scopeIndices = new Int32Array(Ring.SCOPE_CHANNELS);
for (let channel = 0; channel < 4; channel += 1) {
    scopeIndices[channel] = 1024 + channel;
    for (let bucket = 0; bucket < Ring.SCOPE_WINDOW_BUCKETS; bucket += 1) {
        scopeValues[channel * Ring.SCOPE_WINDOW_BUCKETS + bucket] = (channel + 1) * 100 + bucket;
    }
}
// A fifth channel's worth of data that the publish must not carry: the worklet only
// publishes the channels the module actually uses.
scopeValues[4 * Ring.SCOPE_WINDOW_BUCKETS] = -31_000;

Ring.publishScope(scope, scopeValues, scopeIndices, 4, 4, 7);
const window = Ring.readScope(scope);
assert.equal(window.generation, 7);
assert.equal(window.channelCount, 4);
assert.equal(window.bucketFrames, 4);
assert.equal(window.windowBuckets, Ring.SCOPE_WINDOW_BUCKETS);
assert.equal(window.values[0], 100, 'channel 1, oldest bucket');
assert.equal(window.values[Ring.SCOPE_WINDOW_BUCKETS - 1], 100 + Ring.SCOPE_WINDOW_BUCKETS - 1, 'channel 1, newest bucket');
assert.equal(window.values[2 * Ring.SCOPE_WINDOW_BUCKETS], 300, 'channel 3 starts one window on');
assert.equal(window.values[4 * Ring.SCOPE_WINDOW_BUCKETS], 0, 'a channel the module does not use is never published');
assert.deepEqual([...window.indices.slice(0, 4)], [1024, 1025, 1026, 1027]);

// Tearing is the design: a reader that races a republish still gets a window, never null
// and never a throw. That is what makes the scope path free on the audio side.
Atomics.add(scope.words, 0, 1);
const torn = Ring.readScope(scope);
assert.notEqual(torn, null, 'a window read mid-publish is served rather than refused');
Atomics.add(scope.words, 0, 1);

Ring.publishScope(scope, scopeValues, scopeIndices, 0, 4, 8);
assert.equal(Ring.readScope(scope).channelCount, 0, 'a module with no channels publishes an empty window');

const fallback = Ring.decodeWorkletScope(scopeValues, scopeIndices, 4, 4, 9);
assert.equal(fallback.generation, 9);
assert.equal(fallback.values.length, 4 * Ring.SCOPE_WINDOW_BUCKETS, 'the fallback copies only the active channels');
assert.equal(fallback.indices.length, 4);
assert.equal(fallback.values[0], 100);
assert.notEqual(fallback.values.buffer, scopeValues.buffer, 'and it is a copy, so the message owns it');

console.log('ring harness: typed commands, coherent telemetry and lossy scope taps passed');
