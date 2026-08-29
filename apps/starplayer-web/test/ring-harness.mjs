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
const received = [];
assert.equal(Ring.drainCommands(commands, (...record) => received.push(record)), 3);
assert.deepEqual(received, [
    [Ring.OPCODE_PLAY, 0, 0],
    [Ring.OPCODE_SEEK_ORDER, 17, 0],
    [Ring.OPCODE_MASTER_VOLUME, 32768, 0],
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
sourceWords[18] = 48;
sourceWords[19] = 1;
sourceWords[20] = 65535;
sourceWords[22] = 8;
sourceWords[23] = 0x42;
sourceWords[24] = 50000;
sourceWords[25] = 1;

const telemetry = Ring.createTelemetry();
Ring.publishTelemetry(telemetry, sourceWords, 18_000_000, 128, 0, 99);
const snapshot = Ring.readTelemetry(telemetry);
assert.equal(snapshot.sequence, 42);
assert.equal(snapshot.order, 3);
assert.equal(snapshot.pattern, 7);
assert.equal(snapshot.row, 12);
assert.equal(snapshot.masterPeak, 40000);
assert.equal(snapshot.retiredCollected, 2);
assert.equal(snapshot.channels[0].note, 48);
assert.equal(snapshot.channels[0].effectCode, 8);
assert.equal(snapshot.memoryBytes, 18_000_000);
assert.equal(snapshot.quantumFrames, 128);

// An odd publication sequence means a write is in flight and must never be decoded.
Atomics.add(telemetry.words, 0, 1);
assert.equal(Ring.readTelemetry(telemetry), null);
Atomics.add(telemetry.words, 0, 1);
assert.equal(Ring.readTelemetry(telemetry).sequence, 42);

console.log('ring harness: typed commands and coherent telemetry passed');
