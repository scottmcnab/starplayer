import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

const sandbox = { SharedArrayBuffer, Int32Array, Atomics };
vm.createContext(sandbox);
vm.runInContext(await readFile(new URL('../www/ring.js', import.meta.url), 'utf8'), sandbox);
const Ring = sandbox.StarPlayerRing;
const identity = { session: (1n << 61n) + 17n, build: (1n << 63n) + 29n };

test('version2 shared records and fallback carry all identity bits atomically with the action', () => {
    const ring = Ring.createCommandRing(2);
    assert.equal(Ring.pushCommand(ring, Ring.OPCODE_LET_NOTES_RING, 300, 1, identity), true);
    const shared = [];
    assert.equal(Ring.drainCommands(ring, (...record) => shared.push(record)), 1);
    const fallback = [];
    Ring.drainFallbackCommands([{ opcode: Ring.OPCODE_LET_NOTES_RING, argument: 300, extra: 1, identity }], (...record) => fallback.push(record), 2);
    assert.deepEqual(shared, fallback);
    const record = shared[0];
    assert.equal(BigInt(record[3]) | (BigInt(record[4]) << 32n), identity.session);
    assert.equal(BigInt(record[5]) | (BigInt(record[6]) << 32n), identity.build);
});

test('unnegotiated, malformed, squeezed or Number identities are rejected; legacy opcodes survive', () => {
    const old = Ring.createCommandRing();
    assert.equal(Ring.pushCommand(old, Ring.OPCODE_PREVIEW, 0, 0, identity), false);
    assert.equal(Ring.pushCommand(old, Ring.OPCODE_PLAY, 0, 0), true);
    const ring = Ring.createCommandRing(2);
    for (const invalid of [null, { session: Number(identity.session), build: 1 }, { session: -1n, build: 1n }, { session: 1n, build: 1n << 64n }]) assert.equal(Ring.pushCommand(ring, Ring.OPCODE_PREVIEW, 0, 0, invalid), false);
    assert.equal(Ring.pushCommand(ring, Ring.OPCODE_NOTE_OVERLAP, 2, 0, identity), false);
    assert.equal(Ring.pushCommand(ring, Ring.OPCODE_LET_NOTES_RING, 65536, 1, identity), false);
    assert.equal(Ring.pushCommand(ring, Ring.OPCODE_LIVE_DESTINATION, 0, 1, identity), false);
    assert.equal(Ring.drainFallbackCommands([{ opcode: Ring.OPCODE_PREVIEW, argument: 0, extra: 0, identity }], () => assert.fail(), 1), 0);
    assert.equal(Ring.drainFallbackCommands([Ring.OPCODE_PREVIEW, 0, 0], () => assert.fail()), 1);
    const corrupt = Ring.createCommandRing(2);
    Atomics.store(corrupt.words, 4, 99);
    assert.equal(Ring.viewCommandRing(corrupt.buffer).version, 0);
    assert.equal(Ring.drainCommands(Ring.viewCommandRing(corrupt.buffer), () => assert.fail()), 0);
});

test('ring saturation preserves preceding records and does not partially install an identity', () => {
    const ring = Ring.createCommandRing(2);
    for (let index = 0; index < Ring.COMMAND_CAPACITY; index++) assert.equal(Ring.pushCommand(ring, Ring.OPCODE_PREVIEW, index, 0, identity), true);
    assert.equal(Ring.pushCommand(ring, Ring.OPCODE_PREVIEW, 999, 0, { ...identity, build: identity.build + 1n }), false);
    const records = [];
    Ring.drainCommands(ring, (...record) => records.push(record));
    assert.equal(records.length, 64);
    assert.equal(records[63][1], 63);
    assert.equal(BigInt(records[63][5]) | (BigInt(records[63][6]) << 32n), identity.build);
});

test('transport word15 distinguishes paused/stopped/natural end and live summary retains64 tracker lanes', () => {
    const source = new Int32Array(Ring.SNAPSHOT_WORDS);
    source[2] = 64; source[15] = 10;
    source[22 + 64 * 8] = 4; source[23 + 64 * 8] = 1; source[24 + 64 * 8] = 123;
    const paused = Ring.decodeWasmTelemetry(source, 0, 128, 0, 1);
    assert.equal(paused.playing, false); assert.equal(paused.transport, 'paused'); assert.equal(paused.naturalEnd, false);
    assert.equal(paused.channels.length, 64); assert.equal(paused.live.active, 4); assert.equal(paused.live.peak, 123);
    source[15] = 12;
    const ended = Ring.decodeWasmTelemetry(source, 0, 128, 0, 2);
    assert.equal(ended.transport, 'stopped'); assert.equal(ended.naturalEnd, true);
});

test('v2 legacy-numbered controls keep their complete identity on both transports', () => {
    for (const opcode of [Ring.OPCODE_SEEK_ROW, Ring.OPCODE_MUTE_CHANNEL, Ring.OPCODE_INSERT_PARAM, Ring.OPCODE_SET_MIXER_MODE]) {
        const ring = Ring.createCommandRing(2), received = [];
        assert.equal(Ring.pushCommand(ring, opcode, 3, 1, identity), true);
        Ring.drainCommands(ring, (...record) => received.push(record));
        const fallback = [];
        Ring.drainFallbackCommands([{ opcode, argument: 3, extra: 1, identity }], (...record) => fallback.push(record), 2);
        assert.deepEqual(received, fallback);
        assert.equal(BigInt(received[0][5]) | (BigInt(received[0][6]) << 32n), identity.build);
    }
});

test('barrier queue failure retains the rejected request identity instead of the installed identity', async () => {
    const worklet = await readFile(new URL('../www/worklet-processor.js', import.meta.url), 'utf8');
    const method = worklet.slice(worklet.indexOf('    flushInteractive() {'), worklet.indexOf('    publishApplied() {'));
    const flush = vm.runInNewContext(`({ ${method} }).flushInteractive`);
    const requested = new Uint32Array([11, 12, 13, 14]), installed = new Uint32Array([1, 2, 91, 92, 93, 94, 3, 1, 0, 0]);
    let acknowledgement;
    const receiver = {
        interactiveVersion: 2, pendingControlSerial: false, controlFailed: false, interactiveStatus: installed,
        controlAck: { count: 1, requestedIdentity: requested, words: new Uint32Array(10) },
        wasm: { dropped_commands: () => 0, flush_interactive_batch: () => false, queue_applied_barrier_words: () => false },
        allocateBarrier: () => true, port: { postMessage: message => { acknowledgement = structuredClone(message); } },
    };
    flush.call(receiver);
    assert.equal(acknowledgement.accepted, false);
    assert.deepEqual([...acknowledgement.words.slice(2, 6)], [...requested]);
    assert.notDeepEqual([...acknowledgement.words.slice(2, 6)], [...installed.slice(2, 6)]);
});


test('NO08 malformed fallback members remain explicit invalid records without scalar coercion', () => {
    for (const record of [null, 1n, { opcode: 99, argument: 0, extra: 0, identity }, { opcode: 14, argument: 1n, extra: 0, identity }, { opcode: 14, argument: 0, extra: 1n, identity }, { opcode: 14, argument: NaN, extra: 0, identity }, { opcode: 14, argument: 0, extra: 0, identity: { session: 1, build: identity.build } }]) {
        const forwarded = [];
        assert.equal(Ring.drainFallbackCommands([record], (...words) => forwarded.push(words), 2), 1);
        assert.equal(forwarded.length, 1); assert.equal(forwarded[0][0], 0);
    }
});

for (const opcode of [14.5, '14', 14n]) test(`NO08A1 malformed opcode ${typeof opcode}:${String(opcode)} is an explicit invalid member in every fallback group order`, () => {
    const malformed = { opcode, argument: 0, extra: 0, identity };
    const valid = { opcode: Ring.OPCODE_NOTE_OVERLAP, argument: 1, extra: 0, identity };
    const actual = [], expected = [];
    for (const records of [[malformed], [malformed, valid], [valid, malformed]]) {
        const forwarded = [];
        assert.doesNotThrow(() => Ring.drainFallbackCommands(records, (...words) => forwarded.push(words), 2));
        assert.equal(forwarded.length, records.length);
        actual.push(forwarded.map(words => words[0]));
        expected.push(records.map(record => record === malformed ? 0 : Ring.OPCODE_NOTE_OVERLAP));
        console.log('NO08A1 direct fallback', `${typeof opcode}:${String(opcode)}`, forwarded.map(words => `${typeof words[0]}:${String(words[0])}`).join(','));
        for (const words of forwarded) assert.deepEqual(words.slice(3), [Number(identity.session & 0xffffffffn), Number(identity.session >> 32n), Number(identity.build & 0xffffffffn), Number(identity.build >> 32n)]);
    }
    assert.deepEqual(actual, expected);
});
