import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import { createPlaybackController } from '../www/playback-controller.js';

const app = await readFile(new URL('../www/app.js', import.meta.url), 'utf8');
const activationSource = app.slice(app.indexOf('async function activateLoadedModule('), app.indexOf('\nasync function playArchiveEntry('));
const rowPlaySource = app.slice(app.indexOf('export async function playLibraryRow('), app.indexOf('\nasync function loadBuffer(')).replace('export ', '');
const deferred = () => { let resolve, reject; const promise = new Promise((done, fail) => { resolve = done; reject = fail; }); return { promise, resolve, reject }; };
const settle = async () => { for (let index = 0; index < 12; index++) await Promise.resolve(); };
const identity = { sha256: 'folder', name: 'Folder' };
const locator = path => ({ kind: 'library', libraryIdentity: { ...identity }, sourcePath: path, containerMember: '', memberPath: '' });
function setup() {
    const accepted = [], events = [];
    const library = { identity, discovery: { acceptPlaybackMetadata: (...args) => accepted.push(args) } };
    const state = { library, moduleRevision: 1, node: {}, loadCount: 0 };
    let inspected = '';
    const Loader = {
        inspect_s3m(bytes) { inspected = `Title ${bytes[0]}`; events.push('inspect'); },
        module_metadata() { events.push('structured'); return JSON.stringify({ title: inspected, format: 'mod', channels: 4, instrumentNames: 'Samples', sampleNames: '', songMessage: '' }); },
    };
    const sandbox = {
        state, Loader, Uint8Array, playback: { controlRevision: 0 },
        loadEffectNames() { events.push('effects'); }, readMetadata: label => { events.push('display'); return { title: inspected, label, isMod: true, instruments: [] }; },
        showMessage() {}, loadOptionsFromControls: () => ({}), activateModuleOnNode: async (node, buffer) => {
            // The worklet path transfers the original; metadata must use retained bytes.
            structuredClone(buffer, { transfer: [buffer] });
            return { generation: 7 };
        },
        libraryView: { setPlaying() {} }, renderMetadata() {}, playlist: { entries: [] }, mediaSession: { metadata() {}, position() {} },
        scheduleMediaPosition() {}, backgroundOutput: { start() {} }, setControlsEnabled() {}, releaseHeldKeys() {}, syncLiveInput() {},
        queueCommand() {}, queueInteractive() {}, queueRepeatCommand() {}, Ring: {}, elements: { volume: { value: 100 }, overlapEnable: {}, inputDestination: {} }, describeEnhancementReduction: () => '',
        setTimeout() {}, requestGarbageCollection() {},
    };
    vm.createContext(sandbox);
    vm.runInContext(activationSource, sandbox);
    const context = { libraryContext: library, locator: locator('song.mod'), isCurrent: () => true, audio: Promise.resolve() };
    return { state, library, accepted, events, sandbox, Loader, context, activate: sandbox.activateLoadedModule };
}

test('real activation snapshots structured metadata before await and lends retained playback bytes only after commit', async () => {
    const fixture = setup();
    const audio = deferred();
    fixture.context.audio = audio.promise;
    const pending = fixture.activate(new Uint8Array([1, 2, 3]).buffer, 'song.mod', fixture.context);
    assert.deepEqual(fixture.events, ['inspect', 'effects', 'display', 'structured']);
    assert.equal(fixture.accepted.length, 0);
    fixture.Loader.inspect_s3m(new Uint8Array([9])); // Another synchronous inspection cannot overwrite the snapshot.
    fixture.sandbox.playlist.entries = [{ id: 'elsewhere', locator: locator('unrelated.mod') }];
    audio.resolve();
    assert.equal((await pending).generation, 7);
    assert.equal(fixture.accepted.length, 1);
    const [original, metadata, bytes] = fixture.accepted[0];
    assert.equal(original.sourcePath, 'song.mod');
    assert.equal(metadata.title, 'Title 1');
    assert.equal(typeof metadata, 'object');
    assert.equal(bytes, fixture.state.currentModuleBytes);
    assert.deepEqual([...new Uint8Array(bytes)], [1, 2, 3]);
});

for (const fail of ['missing', 'throws', 'bad-json']) test(`optional metadata ${fail} leaves normal activation intact`, async () => {
    const fixture = setup();
    if (fail === 'missing') delete fixture.Loader.module_metadata;
    if (fail === 'throws') fixture.Loader.module_metadata = () => { throw new Error('optional'); };
    if (fail === 'bad-json') fixture.Loader.module_metadata = () => '{';
    assert.equal((await fixture.activate(new Uint8Array([1]).buffer, 'song.mod', fixture.context)).generation, 7);
    assert.equal(fixture.state.metadata.title, 'Title 1');
    assert.equal(fixture.accepted.length, 0);
});

for (const stale of ['intent', 'revision', 'node', 'same-identity-context', 'different-identity-context', 'upload']) test(`activation cannot enrich ${stale}`, async () => {
    const fixture = setup();
    const activation = deferred();
    fixture.sandbox.activateModuleOnNode = () => activation.promise;
    const pending = fixture.activate(new Uint8Array([1]).buffer, 'song.mod', fixture.context);
    await settle();
    if (stale === 'intent') fixture.context.isCurrent = () => false;
    if (stale === 'revision') fixture.state.moduleRevision++;
    if (stale === 'node') fixture.state.node = {};
    if (stale.endsWith('identity-context')) fixture.state.library = { identity: { ...identity, ...(stale.startsWith('different') ? { sha256: 'other' } : {}) }, discovery: { acceptPlaybackMetadata() { assert.fail('replacement must not be enriched'); } } };
    if (stale === 'upload') fixture.context.locator = { kind: 'upload' };
    activation.resolve({ generation: 7 });
    const result = await pending;
    assert.equal(fixture.accepted.length, 0);
    if (['intent', 'revision', 'node'].includes(stale)) assert.equal(result, null);
    else assert.equal(result.generation, 7);
});

test('failed activation never enriches; optional enrichment failure never fails playback', async () => {
    const fixture = setup();
    fixture.sandbox.activateModuleOnNode = async () => { throw new Error('worklet failed'); };
    await assert.rejects(fixture.activate(new Uint8Array([1]).buffer, 'song.mod', fixture.context), /worklet failed/);
    assert.equal(fixture.accepted.length, 0);
    fixture.sandbox.activateModuleOnNode = async () => ({ generation: 8 });
    fixture.library.discovery.acceptPlaybackMetadata = () => { throw new Error('optional hash unavailable'); };
    assert.equal((await fixture.activate(new Uint8Array([1]).buffer, 'song.mod', fixture.context)).generation, 8);
});

for (const origin of ['library', 'playlist']) test(`controller captures ${origin} locator and source context before resolution`, async () => {
    const fixture = setup();
    const waiting = deferred();
    const original = locator('original.mod');
    const controller = createPlaybackController({ resolver: { resolve: () => waiting.promise },
        captureContext: () => ({ libraryContext: fixture.state.library }), activate: (bytes, label, origin, context) => fixture.activate(bytes, label, context),
        readSnapshot: () => null, clock: { setInterval() {}, clearInterval() {} } });
    const pending = controller.play(original, { origin, entryId: 'original' });
    original.sourcePath = 'later-selection.mod';
    original.libraryIdentity.sha256 = 'changed';
    waiting.resolve(new Uint8Array([1]).buffer);
    assert.equal((await pending).status, 'played');
    assert.equal(fixture.accepted.length, 1);
    assert.equal(fixture.accepted[0][0].sourcePath, 'original.mod');
    assert.equal(fixture.accepted[0][0].libraryIdentity.sha256, 'folder');
    controller.dispose();
});

test('same-identity source replacement while resolving cannot enrich its new generation', async () => {
    const fixture = setup();
    const waiting = deferred();
    const controller = createPlaybackController({ resolver: { resolve: () => waiting.promise },
        captureContext: () => ({ libraryContext: fixture.state.library }), activate: (bytes, label, origin, context) => fixture.activate(bytes, label, context),
        readSnapshot: () => null, clock: { setInterval() {}, clearInterval() {} } });
    const pending = controller.play(locator('song.mod'), { origin: 'playlist' });
    fixture.state.library = { identity, discovery: { acceptPlaybackMetadata() { assert.fail('replacement context'); } } };
    waiting.resolve(new Uint8Array([1]).buffer);
    assert.equal((await pending).status, 'played');
    assert.equal(fixture.accepted.length, 0);
    controller.dispose();
});

test('library labels omit empty titles/separators and never mark a replacement view playing', async () => {
    const fixture = setup();
    let label;
    const played = [];
    const waiting = deferred();
    fixture.sandbox.playback = { play: (locator, options) => { label = options.label; return waiting.promise; } };
    fixture.sandbox.libraryLocator = (row, identity) => ({ ...row, libraryIdentity: identity });
    fixture.sandbox.reportLoadFailure = () => assert.fail('unexpected failure');
    fixture.sandbox.libraryView.setPlaying = id => played.push(id);
    vm.runInContext(rowPlaySource, fixture.sandbox);
    const pending = fixture.sandbox.playLibraryRow({ id: 4, name: 'song.mod', title: '', folder: '', playable: true });
    assert.equal(label, 'song.mod');
    fixture.state.library = { identity };
    waiting.resolve({ status: 'played' });
    await pending;
    assert.deepEqual(played, []);
});


test('Stop/cancel during an installed pending load cannot issue automatic Play or commit metadata', async () => {
    const fixture = setup();
    const activation = deferred();
    const sent = [];
    fixture.sandbox.queueCommand = (...command) => sent.push(command);
    fixture.sandbox.queueInteractive = (...command) => sent.push(command);
    fixture.sandbox.activateModuleOnNode = () => activation.promise;
    let current = true;
    fixture.context.isCurrent = () => current;
    const pending = fixture.activate(new Uint8Array([1]).buffer, 'cancelled.mod', fixture.context);
    await settle();
    current = false; fixture.sandbox.playback.controlRevision += 1;
    activation.resolve({ generation: 9 });
    assert.equal(await pending, null);
    assert.equal(sent.length, 0);
    assert.equal(fixture.state.loadCount, 0);
});

test('Pause during a pending fresh activation wins over its default automatic Play', async () => {
    const fixture = setup();
    const activation = deferred(); const sent = [];
    fixture.sandbox.Ring.OPCODE_INTERACTIVE_PLAY = 13;
    fixture.sandbox.Ring.OPCODE_RESUME = 15;
    fixture.sandbox.queueCommand = (...command) => sent.push(command);
    fixture.sandbox.queueInteractive = (...command) => sent.push(command);
    fixture.sandbox.activateModuleOnNode = () => activation.promise;
    const pending = fixture.activate(new Uint8Array([1]).buffer, 'paused.mod', fixture.context);
    await settle();
    fixture.sandbox.playback.controlRevision += 1;
    fixture.state.lastTransportOpcode = 14;
    activation.resolve({ generation: 10 });
    assert.equal((await pending).generation, 10);
    assert.equal(sent.some(command => command[0] === 13 || command[0] === 15), false);
});
