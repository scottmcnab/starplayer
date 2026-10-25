import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createBackgroundOutput, backgroundPlaybackDefault } from '../www/background-output.js';

function setup(enabled = true) {
    const calls = [];
    const audio = { paused: true, srcObject: null, sinkId: '', setAttribute() {}, play() { this.paused = false; return Promise.resolve(); }, pause() { this.paused = true; }, async setSinkId(value) { this.sinkId = value; calls.push('audio sink'); } };
    const context = () => ({ destination: {}, sinkId: '', createMediaStreamDestination() { calls.push('stream'); return { stream: {} }; }, async setSinkId(value) { this.sinkId = value; calls.push('context sink'); } });
    const node = { connections: [], connect(destination) { this.connections.push(destination); calls.push('connect'); }, disconnect() { this.connections = []; calls.push('disconnect'); } };
    const messages = [];
    return { output: createBackgroundOutput({ enabled, createAudio: () => audio, report: message => messages.push(message) }), audio, context, node, calls, messages };
}
test('routing toggles disconnect first, caches streams per context, and preserves one element', () => {
    const { output, audio, context, node, calls } = setup();
    const first = context();
    output.start(first);
    assert.equal(audio.paused, false);
    output.connectOutput(node, first);
    const stream = audio.srcObject;
    assert.equal(node.connections.length, 1);
    assert.notEqual(node.connections[0], first.destination);
    output.setEnabled(false, node, first, true);
    assert.deepEqual(calls.filter(call => call === 'disconnect' || call === 'connect').slice(-2), ['disconnect', 'connect']);
    assert.deepEqual(node.connections, [first.destination]);
    assert.equal(audio.srcObject, null);
    assert.equal(audio.paused, true);
    output.setEnabled(true, node, first, true);
    assert.equal(audio.srcObject, stream);
    output.connectOutput(node, context());
    assert.notEqual(audio.srcObject, stream);
    assert.equal(calls.filter(call => call === 'stream').length, 2);
});
test('play rejection falls back to exactly one direct connection', async () => {
    const { output, audio, context, node, messages } = setup();
    const graph = context();
    audio.play = () => Promise.reject(new Error('denied'));
    output.start(graph);
    output.connectOutput(node, graph);
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(node.connections, [graph.destination]);
    assert.equal(output.active, false);
    assert.equal(audio.srcObject, null);
    assert.equal(messages.length, 1);
});
test('sink choice follows the route and survives toggles; stopped toggles do not play', async () => {
    const { output, audio, context, node } = setup();
    const graph = context();
    output.connectOutput(node, graph);
    await output.setSinkId('speakers');
    assert.equal(audio.sinkId, 'speakers');
    output.setEnabled(false, node, graph, false);
    assert.equal(graph.sinkId, 'speakers');
    output.setEnabled(true, node, graph, false);
    assert.equal(audio.paused, true);
    assert.equal(output.sinkId, 'speakers');
    audio.setSinkId = undefined;
    assert.equal(output.sinkSupported, false);
});
test('mobile default prefers client hints, with coarse pointer fallback', () => {
    assert.equal(backgroundPlaybackDefault({ userAgentData: { mobile: false } }, () => ({ matches: true })), false);
    assert.equal(backgroundPlaybackDefault({}, () => ({ matches: true })), true);
});
test('a late play rejection after Stop does not disable background routing', async () => {
    const { output, audio, context, node, messages } = setup();
    const graph = context();
    let rejectPlay;
    audio.play = () => new Promise((resolve, reject) => { rejectPlay = reject; });
    output.connectOutput(node, graph);
    output.start(graph);
    output.pause();
    rejectPlay(new Error('interrupted'));
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(output.active, true);
    assert.equal(audio.paused, true);
    assert.equal(messages.length, 0);
});
