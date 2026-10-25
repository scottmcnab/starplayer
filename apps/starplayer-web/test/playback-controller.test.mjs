import test from 'node:test';
import assert from 'node:assert/strict';
import { createPlaybackController } from '../www/playback-controller.js';
import { createPlaylistPlayback } from '../www/playlist-playback.js';
import { createPlaylist } from '../www/playlist.js';

const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const settle = async () => { for (let index = 0; index < 20; index += 1) await Promise.resolve(); };
function setup(overrides = {}) {
    let callback;
    const delays = [];
    const clock = { setInterval(fn, milliseconds) { assert.equal(milliseconds, 100); callback = fn; return 1; }, clearInterval() { callback = null; }, setTimeout(fn) { delays.push(fn); } };
    let snapshot = null;
    let graph = 'graph';
    let generation = 0;
    let repeat = true;
    let flushed = 0;
    const ended = [];
    const activated = [];
    const controller = createPlaybackController({
        resolver: { resolve: async locator => locator.buffer },
        activate: async buffer => { activated.push(buffer); return { graph, generation: ++generation }; },
        readSnapshot: () => ({ graph, snapshot }), flushCommands: () => flushed++,
        repeat: () => repeat, onTrackEnded: id => ended.push(id), clock, ...overrides,
    });
    return { controller, clock, ended, activated, delays, get flushed() { return flushed; }, setRepeat(value) { repeat = value; },
        tick(playing, songFlags = 1, moduleGeneration = generation, observedGraph = graph) { graph = observedGraph; snapshot = { playing, songFlags, moduleGeneration, sequence: (snapshot?.sequence || 0) + 1 }; callback(); },
        get snapshot() { return snapshot; },
        setSnapshot(fields) { snapshot = { moduleGeneration: generation, ...fields }; callback(); },
    };
}
const locator = buffer => ({ kind: 'bytes', buffer });

test('slow automatic resolve cannot supersede a manual load; aborted errors are abandoned', async () => {
    const slow = deferred();
    const fixture = setup({ resolver: { resolve: () => slow.promise } });
    const pending = fixture.controller.play({ kind: 'library' }, { origin: 'playlist', entryId: 'first' });
    await fixture.controller.play(locator('manual'));
    slow.resolve('late');
    assert.equal((await pending).status, 'abandoned');
    assert.deepEqual(fixture.activated, ['manual']);
    const aborted = setup({ resolver: { resolve: async () => { throw new DOMException('cancelled', 'AbortError'); } } });
    assert.equal((await aborted.controller.play({ kind: 'library' })).status, 'abandoned');
});

for (const flags of [[1, 5], [3, 15, 7]]) test(`natural completion emits once through flags ${flags}`, async () => {
    const fixture = setup();
    await fixture.controller.play(locator('song'), { origin: 'playlist', entryId: 'entry' });
    for (const value of flags.slice(0, -1)) fixture.tick(true, value);
    fixture.tick(false, flags.at(-1));
    fixture.tick(false, flags.at(-1));
    fixture.tick(false, 3); // Host rewind clears END_REACHED.
    assert.deepEqual(fixture.ended, ['entry']);
    assert.ok(fixture.flushed >= 3);
});

test('the stop at the end counts even when it repeats the engine sequence', async () => {
    const fixture = setup();
    await fixture.controller.play(locator('song'), { origin: 'playlist', entryId: 'entry' });
    // The host packs `playing` and `engineFrame` live, so the engine sequence can stay put.
    fixture.tick(true, 1);
    const { sequence } = fixture.snapshot;
    fixture.setSnapshot({ playing: true, songFlags: 1, sequence, engineFrame: 128 });
    fixture.setSnapshot({ playing: true, songFlags: 1, sequence, engineFrame: 128 });
    fixture.setSnapshot({ playing: false, songFlags: 5, sequence, engineFrame: 256 });
    fixture.setSnapshot({ playing: false, songFlags: 5, sequence, engineFrame: 256 });
    assert.deepEqual(fixture.ended, ['entry']);
});

test('a seek needs a fresh playing edge but does not suppress completion', async () => {
    const fixture = setup();
    await fixture.controller.play(locator('song'));
    fixture.tick(true, 15);
    fixture.controller.userControl('seek');
    fixture.tick(false, 7); // A stale stopped snapshot straight after the seek is not an edge.
    assert.deepEqual(fixture.ended, []);
    fixture.tick(true, 3);
    fixture.tick(false, 7);
    assert.deepEqual(fixture.ended, [null]);
});

for (const action of ['stop']) test(`${action} during fade suppresses completion until Play`, async () => {
    const fixture = setup();
    await fixture.controller.play(locator('song'));
    fixture.tick(true, 15);
    fixture.controller.userControl(action);
    fixture.tick(false, 7);
    assert.deepEqual(fixture.ended, []);
    fixture.controller.userControl('play');
    fixture.tick(false, 7); // A stale stopped snapshot after Play is not an edge.
    fixture.tick(true, 3);
    fixture.tick(false, 7);
    assert.deepEqual(fixture.ended, [null]);
});

test('replacement and stale generation/graph snapshots cannot end the new track', async () => {
    const fixture = setup();
    await fixture.controller.play(locator('one'));
    fixture.tick(true);
    await fixture.controller.play(locator('two'));
    fixture.tick(false, 5, 1);
    fixture.tick(true, 1, 2);
    fixture.tick(false, 5, 1);
    fixture.tick(false, 5, 2, 'old graph');
    assert.deepEqual(fixture.ended, []);
    fixture.controller.rebind('rebuilt', 1);
    fixture.tick(false, 5, 1, 'rebuilt');
    fixture.tick(true, 1, 1);
    fixture.tick(false, 5, 1);
    assert.deepEqual(fixture.ended, [null]);
});

test('playlist at-end policy preserves Repeat preference, including rebuild', async () => {
    const fixture = setup();
    assert.equal(fixture.controller.effectiveAtEnd(), true);
    await fixture.controller.play(locator('song'), { origin: 'playlist' });
    assert.equal(fixture.controller.effectiveAtEnd(), false);
    fixture.controller.rebind('rebuilt', 1);
    assert.equal(fixture.controller.effectiveAtEnd(), false);
    await fixture.controller.play(locator('manual'));
    assert.equal(fixture.controller.effectiveAtEnd(), true);
    fixture.setRepeat(false);
    assert.equal(fixture.controller.effectiveAtEnd(), false);
});

test('superseded activation is abandoned and cannot become current', async () => {
    const waiting = deferred();
    const fixture = setup({ activate: async (buffer, label, origin, context) => {
        if (buffer === 'slow') await waiting.promise;
        if (!context.isCurrent()) return null;
        return { graph: 'graph', generation: buffer === 'slow' ? 1 : 2 };
    } });
    const pending = fixture.controller.play(locator('slow'));
    await settle();
    await fixture.controller.play(locator('fast'));
    waiting.resolve();
    assert.equal((await pending).status, 'abandoned');
    assert.equal(fixture.controller.current.generation, 2);
});

function playlistFixture(resolve) {
    let navigation;
    const fixture = setup({ resolver: { resolve }, onIntent: ({ origin }) => { if (origin !== 'playlist') navigation.cancel(); }, onTrackEnded: id => navigation.ended(id) });
    const playlist = createPlaylist();
    const entries = playlist.add(['one', 'two', 'three', 'four'].map(name => ({ locator: { kind: 'library', sourcePath: name }, display: { name } })));
    const errors = [];
    const prefetched = [];
    navigation = createPlaylistPlayback({ playlist, controller: fixture.controller, resolver: { prefetch: next => prefetched.push(next.sourcePath) }, report: message => errors.push(message), clock: fixture.clock });
    return { ...fixture, playlist, entries, errors, prefetched, navigation };
}

test('three consecutive real failures stop; aborted loads do not count', async () => {
    const fixture = playlistFixture(async () => { throw new Error('broken'); });
    const pending = fixture.navigation.playEntry(fixture.entries[0]);
    await settle();
    fixture.delays.shift()();
    await settle();
    fixture.delays.shift()();
    await pending;
    assert.equal(fixture.navigation.failures.size, 3);
    assert.match(fixture.errors.at(-1), /three consecutive failures/);
    const aborted = playlistFixture(async () => { throw new DOMException('cancelled', 'AbortError'); });
    await aborted.navigation.playEntry(aborted.entries[0]);
    assert.equal(aborted.navigation.failures.size, 0);
    assert.deepEqual(aborted.errors, []);
});

test('navigation warms next entry and removal invalidates pending advance without Stop', async () => {
    const slow = deferred();
    const fixture = playlistFixture(async entry => entry.sourcePath === 'two' ? slow.promise : entry.sourcePath);
    await fixture.navigation.playEntry(fixture.entries[0]);
    assert.deepEqual(fixture.prefetched, ['two']);
    const pending = fixture.navigation.ended(fixture.entries[0].id);
    fixture.playlist.remove([fixture.entries[0].id]);
    slow.resolve('two');
    await pending;
    assert.deepEqual(fixture.activated, ['one']);
    assert.equal(fixture.playlist.current, null);
});

test('manual load cancels a failure skip delay', async () => {
    const fixture = playlistFixture(async () => { throw new Error('broken'); });
    const pending = fixture.navigation.playEntry(fixture.entries[0]);
    await settle();
    await fixture.controller.play(locator('manual'));
    fixture.delays.shift()();
    await pending;
    assert.equal(fixture.navigation.failures.size, 1);
    assert.deepEqual(fixture.activated, ['manual']);
});

test('navigation follows reordered entries, wraps only when enabled, leaves last current', async () => {
    let loop = false;
    const playlist = createPlaylist();
    const entries = playlist.add(['one', 'two', 'three'].map(name => ({ locator: locator(name), display: { name } })));
    const fixture = setup();
    const navigation = createPlaylistPlayback({ playlist, controller: fixture.controller, resolver: { prefetch() {} }, loop: () => loop, clock: fixture.clock });
    await navigation.playEntry(entries[0]);
    playlist.move(entries[2].id, 1);
    await navigation.ended(entries[0].id);
    assert.equal(playlist.current.id, entries[2].id);
    await navigation.next();
    assert.equal(playlist.current.id, entries[1].id);
    await navigation.ended(entries[1].id);
    assert.equal(playlist.current.id, entries[1].id);
    assert.equal(fixture.activated.length, 3);
    loop = true;
    await navigation.ended(entries[1].id);
    assert.equal(playlist.current.id, entries[0].id);
    await navigation.previous();
    assert.equal(playlist.current.id, entries[1].id);
});

test('activation failures are skipped and successful playback resets the failure streak', async () => {
    const fixture = playlistFixture(async entry => entry.sourcePath);
    // Use a separate controller to make the failure originate in activation.
    let attempts = 0;
    const controller = setup({ activate: async () => { if (++attempts < 3) throw new Error('activation failed'); return { graph: 'graph', generation: attempts }; } }).controller;
    const errors = [];
    const navigation = createPlaylistPlayback({ playlist: fixture.playlist, controller, resolver: { prefetch() {} }, report: message => errors.push(message), clock: fixture.clock });
    const pending = navigation.playEntry(fixture.entries[0]);
    await settle();
    fixture.delays.shift()();
    await settle();
    fixture.delays.shift()();
    await pending;
    assert.equal(fixture.playlist.current.id, fixture.entries[2].id);
    assert.equal(errors.length, 2);
    assert.equal(navigation.failures.size, 2);
});

test('W7 completion subscriptions remain stable when the first listener starts another track', async () => {
    let stopped = 0;
    const fixture = setup({ stop: () => stopped++ });
    const notifications = [];
    fixture.controller.onTrackEnded(() => { fixture.controller.play(locator('next')); });
    const unsubscribe = fixture.controller.onTrackEnded(id => notifications.push(id));
    await fixture.controller.play(locator('first'), { entryId: 'first' });
    fixture.tick(true);
    fixture.tick(false, 5);
    assert.deepEqual(notifications, ['first']);
    unsubscribe();
    fixture.controller.stop();
    assert.equal(stopped, 1);
});

test('unavailable entries are skipped without failure budget; looping all unavailable terminates', async () => {
    const fixture = setup();
    const playlist = createPlaylist();
    const entries = playlist.add(Array.from({ length: 6 }, (_, index) => ({ locator: locator(String(index)), display: { name: String(index) } })));
    let available = false;
    const navigation = createPlaylistPlayback({ playlist, controller: fixture.controller, resolver: { prefetch() {} }, loop: () => true, available: entry => available && entry.id === entries[5].id });
    await navigation.playEntry(entries[0]);
    assert.equal(playlist.current, null);
    assert.equal(navigation.failures.size, 0);
    available = true;
    await navigation.playEntry(entries[0]);
    assert.equal(playlist.current.id, entries[5].id);
    assert.equal(navigation.failures.size, 0);
});


test('Play after completion re-arms exactly one notification without a new activation', async () => {
    const fixture = setup();
    await fixture.controller.play(locator('song'), { origin: 'playlist', entryId: 'entry' });
    fixture.tick(true);
    fixture.tick(false, 5);
    fixture.controller.userControl('play');
    fixture.tick(false, 5);
    fixture.tick(true);
    fixture.tick(false, 5);
    fixture.tick(false, 5);
    assert.deepEqual(fixture.ended, ['entry', 'entry']);
    assert.equal(fixture.activated.length, 1);
});

test('transport changes invalidate captured graph restoration and preserve Stop through rebind', async () => {
    const fixture = setup();
    await fixture.controller.play(locator('song'));
    const revision = fixture.controller.controlRevision;
    fixture.controller.stop();
    fixture.controller.rebind('rebuilt', 1);
    assert.notEqual(fixture.controller.controlRevision, revision);
    assert.equal(fixture.controller.stopped, true);
    fixture.controller.userControl('seek');
    assert.equal(fixture.controller.stopped, true);
    fixture.controller.userControl('play');
    assert.equal(fixture.controller.stopped, false);
});


test('a large disconnected playlist traverses once without copying entries per skip', async () => {
    const playlist = createPlaylist();
    const entries = playlist.add(Array.from({ length: 37674 }, (_, index) => ({ locator: locator(index), display: { name: String(index) } })));
    let reads = 0;
    let checked = 0;
    const observed = new Proxy(playlist, { get(target, key) { if (key === 'entries') reads += 1; return target[key]; } });
    const fixture = setup();
    const navigation = createPlaylistPlayback({ playlist: observed, controller: fixture.controller, resolver: {}, loop: () => true, available: () => { checked += 1; return false; } });
    await navigation.playEntry(entries[0]);
    assert.equal(checked, entries.length + 1);
    assert.ok(reads < 10, `playlist copied ${reads} times`);
    assert.equal(fixture.activated.length, 0);
    navigation.dispose();
});

test('loading covers only the newest intent, through success, failure and cancellation', async () => {
    const slow = deferred();
    const fixture = setup({ resolver: { resolve: () => slow.promise } });
    const superseded = fixture.controller.play({ kind: 'library' }, { origin: 'playlist', entryId: 'first' });
    assert.equal(fixture.controller.loading, true);
    const manual = deferred();
    const replacement = fixture.controller.play({ kind: 'bytes', read: () => manual.promise });
    slow.resolve('late');
    assert.equal((await superseded).status, 'abandoned');
    assert.equal(fixture.controller.loading, true, 'the abandoned advance must not end the newer load');
    manual.resolve('manual');
    assert.equal((await replacement).status, 'played');
    assert.equal(fixture.controller.loading, false);
    const failing = setup({ resolver: { resolve: async () => { throw new Error('missing'); } } });
    assert.equal((await failing.controller.play({ kind: 'library' })).status, 'failed');
    assert.equal(failing.controller.loading, false);
    const hanging = setup({ resolver: { resolve: () => new Promise(() => {}) } });
    hanging.controller.play({ kind: 'library' });
    assert.equal(hanging.controller.loading, true);
    hanging.controller.cancel();
    assert.equal(hanging.controller.loading, false, 'a cancel ends loading even while the old fetch hangs');
});

test('playlist pending covers an advance until it plays, is cancelled or runs out of entries', async () => {
    const playlist = createPlaylist();
    playlist.add([{ locator: locator('one'), display: { name: 'one' } }, { locator: locator('two'), display: { name: 'two' } }]);
    const slow = deferred();
    const fixture = setup({ resolver: { resolve: () => slow.promise } });
    const navigation = createPlaylistPlayback({ playlist, controller: fixture.controller, resolver: { prefetch() {} }, clock: fixture.clock });
    assert.equal(navigation.pending, null);
    const advance = navigation.playEntry({ ...playlist.entries[0], locator: { kind: 'library' } });
    assert.equal(navigation.pending, playlist.entries[0].id);
    slow.resolve('one');
    await advance;
    assert.equal(navigation.pending, null);
    await navigation.playEntry(null);
    assert.equal(navigation.pending, null, 'an advance past the last entry leaves nothing pending');
    navigation.playEntry({ ...playlist.entries[1], locator: { kind: 'library' } });
    navigation.cancel();
    assert.equal(navigation.pending, null);
});
