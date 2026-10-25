import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createBackgroundOutput, backgroundPlaybackDefault } from '../www/background-output.js';

function setup(enabled = true) {
    const attributes = new Map();
    const audio = { paused: true, setAttribute(name, value) { attributes.set(name, value); }, play() { this.paused = false; return Promise.resolve(); }, pause() { this.paused = true; } };
    const messages = [];
    const blobs = [];
    const output = createBackgroundOutput({ enabled, createAudio: () => audio, createObjectURL: blob => { blobs.push(blob); return 'blob:silence'; }, report: message => messages.push(message) });
    return { output, audio, attributes, blobs, messages };
}
test('one looping inline element uses a ten-second silent PCM WAV blob', async () => {
    const { output, audio, attributes, blobs } = setup();
    assert.equal(audio.hidden, true);
    assert.equal(audio.autoplay, false);
    assert.equal(audio.loop, true);
    assert.equal(attributes.has('playsinline'), true);
    assert.equal(audio.src, 'blob:silence');
    assert.equal(blobs[0].type, 'audio/wav');
    const buffer = await blobs[0].arrayBuffer();
    const bytes = new Uint8Array(buffer);
    const header = new DataView(buffer);
    const text = (offset, size) => new TextDecoder().decode(bytes.subarray(offset, offset + size));
    assert.equal(text(0, 4), 'RIFF');
    assert.equal(header.getUint32(4, true), bytes.length - 8);
    assert.equal(text(8, 8), 'WAVEfmt ');
    assert.equal(header.getUint32(16, true), 16);
    assert.equal(header.getUint16(20, true), 1);
    assert.equal(header.getUint16(22, true), 1);
    assert.equal(header.getUint32(24, true), 8000);
    assert.equal(header.getUint32(28, true), 8000);
    assert.equal(header.getUint16(32, true), 1);
    assert.equal(header.getUint16(34, true), 8);
    assert.equal(text(36, 4), 'data');
    assert.equal(header.getUint32(40, true), 80000);
    assert.equal(bytes.length, 80044);
    assert.ok(header.getUint32(40, true) / header.getUint32(24, true) > 5);
    assert.ok(bytes.subarray(44).every(sample => sample === 0x80));
    output.start(); output.pause(); output.start();
    assert.equal(blobs.length, 1);
});
test('start is synchronous; pause and toggles preserve the same source', () => {
    const { output, audio } = setup(false);
    output.start();
    assert.equal(audio.paused, true);
    output.setEnabled(true, false);
    assert.equal(audio.paused, true);
    output.start();
    assert.equal(audio.paused, false);
    output.pause();
    assert.equal(audio.paused, true);
    output.setEnabled(true, true);
    assert.equal(audio.paused, false);
    output.setEnabled(false, true);
    assert.equal(audio.paused, true);
    assert.equal(audio.src, 'blob:silence');
});
test('play rejection reports a note and allows a retry', async () => {
    const { output, audio, messages } = setup();
    audio.play = () => Promise.reject(new Error('denied'));
    output.start();
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(messages, ['Background playback unavailable (Error).']);
    audio.play = () => { audio.paused = false; };
    output.start();
    assert.equal(audio.paused, false);
});
test('synchronous play rejection is also reported', () => {
    const { output, audio, messages } = setup();
    audio.play = () => { throw new Error('denied'); };
    output.start();
    assert.equal(messages.length, 1);
});
test('a late play rejection after Stop does not report an error', async () => {
    const { output, audio, messages } = setup();
    let rejectPlay;
    audio.play = () => new Promise((resolve, reject) => { rejectPlay = reject; });
    output.start(); output.pause();
    rejectPlay(new Error('interrupted'));
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(audio.paused, true);
    assert.equal(messages.length, 0);
});
test('keep-alive never uses streams or touches the audio graph', async () => {
    const source = await readFile(new URL('../www/background-output.js', import.meta.url), 'utf8');
    assert.doesNotMatch(source, /MediaStream|srcObject|AudioContext|\.connect\(|\.disconnect\(|\.destination|setSinkId/);
    const { output } = setup();
    const graph = new Proxy({}, { get() { assert.fail('keep-alive accessed the graph'); } });
    output.start(graph); output.setEnabled(true, true, graph); output.pause();
});
test('mobile default prefers client hints, with coarse pointer fallback', () => {
    assert.equal(backgroundPlaybackDefault({ userAgentData: { mobile: false } }, () => ({ matches: true })), false);
    assert.equal(backgroundPlaybackDefault({}, () => ({ matches: true })), true);
});
