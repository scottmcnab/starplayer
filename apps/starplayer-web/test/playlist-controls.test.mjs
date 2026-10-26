import test from 'node:test';
import assert from 'node:assert/strict';
import { createPlaylist } from '../www/playlist.js';
import { createPlaylistControls } from '../www/playlist-controls.js';
import { createPlaylistPlayback } from '../www/playlist-playback.js';
import { createDiscoveredLibrary, appendDiscoveredRows } from '../www/library-index.js';

const identity = { sha256: 'folder', name: 'Folder' };
const row = sourcePath => ({ sourcePath, containerMember: '', memberPath: '', rawHex: '', size: 1, timestamp: '', playable: true, format: 'mod' });
const item = (sourcePath, libraryIdentity = identity, metadataState) => ({ locator: { kind: 'library', libraryIdentity, ...row(sourcePath) }, display: { name: sourcePath, title: 'Provisional', format: 'mod', ...(metadataState ? { metadataState } : {}) } });
const library = (phase = 'running', libraryIdentity = identity) => ({ mode: 'discovered', identity: libraryIdentity, source: { kind: 'directory', label: 'Folder' }, library: createDiscoveredLibrary({ name: 'Folder' }), discoveryStatus: { phase } });
async function setup() {
    const previous = globalThis.document;
    const elements = new Map();
    const element = name => {
        if (!elements.has(name)) elements.set(name, Object.assign(new EventTarget(), { textContent: '', hidden: false, disabled: false, value: '', checked: false, accept: '.mod' }));
        return elements.get(name);
    };
    globalThis.document = { getElementById: element };
    const playlist = createPlaylist();
    const saved = [];
    const store = { state: { entries: [], currentId: null, loop: false }, save: async (snapshot, options) => saved.push(structuredClone({ snapshot, options })), hasBlob: id => id === 'present' };
    const state = { library: library() };
    const controls = await createPlaylistControls({ playlist, state, loader: {}, openStore: async () => store });
    let renders = 0;
    controls.setRender(() => renders++);
    return { playlist, controls, state, saved, element: name => element(`playlist-${name}`), get renders() { return renders; }, restore() { globalThis.document = previous; } };
}

test('discovery status stays informational for explicit playback and advance, including complete missing rows', async () => {
    const fixture = await setup();
    try {
        const entries = fixture.playlist.add([item('before.mod'), item('never-listed.mod')]);
        const played = [];
        const controller = { playlistMode: true, cancel() {}, async play(locator) { played.push(locator.sourcePath); return { status: 'played' }; } };
        const navigation = createPlaylistPlayback({ playlist: fixture.playlist, controller, resolver: { prefetch() {} }, available: fixture.controls.canPlay });
        for (const phase of ['running', 'paused', 'failed', 'complete']) {
            fixture.state.library.discoveryStatus.phase = phase;
            assert.equal(fixture.controls.availability(entries[0]), phase === 'complete' ? 'not found in scan' : 'not yet discovered');
            assert.equal(fixture.controls.canPlay(entries[0]), true);
            await navigation.playEntry(entries[0]);
            await navigation.ended(entries[0].id);
            assert.equal(fixture.playlist.current.id, entries[1].id);
        }
        assert.deepEqual(played, Array.from({ length: 4 }, () => ['before.mod', 'never-listed.mod']).flat());
        assert.equal(navigation.failures.size, 0);
        navigation.dispose();
    } finally { fixture.restore(); }
});

test('disconnected/index-not-listed/missing-upload gates remain authoritative; membership arrival refreshes controls', async () => {
    const fixture = await setup();
    try {
        const [entry] = fixture.playlist.add([item('arrived.mod')]);
        fixture.state.library = { ...fixture.state.library, mode: 'indexed' };
        assert.equal(fixture.controls.canPlay(entry), false);
        assert.equal(fixture.controls.availability(entry), 'track not in library');
        const changedRowIds = appendDiscoveredRows(fixture.state.library.library, [row('arrived.mod')]);
        const saves = fixture.saved.length;
        fixture.controls.libraryUpdated(fixture.state.library, { changedRowIds, statusChanged: false, metadataUpdates: [] });
        assert.equal(fixture.controls.canPlay(entry), true);
        assert.equal(fixture.controls.availability(entry), '');
        assert.equal(fixture.renders, 1);
        assert.equal(fixture.saved.length, saves);
        fixture.state.library = library('running', { sha256: 'different', name: 'Same name' });
        assert.equal(fixture.controls.canPlay(entry), false);
        assert.match(fixture.controls.availability(entry), /library not connected/);
        for (const [blobId, allowed] of [['present', true], ['missing', false]]) {
            const upload = { locator: { kind: 'upload', blobId } };
            assert.equal(fixture.controls.canPlay(upload), allowed);
            assert.equal(fixture.controls.availability(upload), allowed ? '' : 'missing — re-add');
        }
    } finally { fixture.restore(); }
});

test('rebind remains pending through running/paused/failed and applies one complete matching set', async () => {
    const fixture = await setup();
    try {
        const old = { sha256: 'old', name: 'Old selection' };
        const entries = fixture.playlist.add([item('first.mod', old), item('second.mod', old), item('missing.mod', old)]);
        appendDiscoveredRows(fixture.state.library.library, [row('first.mod')]);
        for (const phase of ['running', 'paused', 'failed']) {
            fixture.state.library.discoveryStatus.phase = phase;
            fixture.controls.libraryUpdated(fixture.state.library, { statusChanged: true });
            assert.equal(fixture.element('rebind').hidden, false);
            assert.equal(fixture.element('rebind').disabled, true);
            assert.match(fixture.element('rebind').title, new RegExp(phase));
            fixture.element('rebind').dispatchEvent(new Event('click'));
            assert.ok(fixture.playlist.entries.every(entry => entry.locator.libraryIdentity.sha256 === old.sha256));
        }
        appendDiscoveredRows(fixture.state.library.library, [row('second.mod')]);
        fixture.state.library.discoveryStatus.phase = 'complete';
        fixture.controls.libraryUpdated(fixture.state.library, { statusChanged: true });
        assert.equal(fixture.element('rebind').disabled, false);
        assert.match(fixture.element('rebind').textContent, /Rebind 2 entries/);
        fixture.element('rebind').dispatchEvent(new Event('click'));
        assert.deepEqual(fixture.playlist.entries.map(entry => entry.id), entries.map(entry => entry.id));
        assert.deepEqual(fixture.playlist.entries.map(entry => entry.locator.libraryIdentity.sha256), ['folder', 'folder', 'old']);
    } finally { fixture.restore(); }
});

test('partial before-row metadata enriches every matching legacy/unknown entry and persists once without no-op writes', async () => {
    const fixture = await setup();
    try {
        const entries = fixture.playlist.add([item('song.mod'), item('song.mod', identity, 'unknown'), item('song.mod', { sha256: 'other', name: 'Other' }), { locator: { kind: 'upload', blobId: 'song.mod' }, display: { name: 'song.mod' } }]);
        fixture.playlist.setCurrent(entries[1].id);
        const saves = fixture.saved.length;
        const change = { changedRowIds: [], statusChanged: true, metadataUpdates: [
            { locator: row('song.mod'), patch: { title: 'Actual', format: 's3m', metadataState: 'ready' } },
            { locator: row('song.mod'), patch: { sha256: 'hash' } },
        ] };
        fixture.controls.libraryUpdated(fixture.state.library, change);
        assert.equal(fixture.saved.length, saves + 1);
        assert.deepEqual(fixture.playlist.entries.slice(0, 2).map(entry => [entry.display.title, entry.display.format, entry.display.metadataState]), [['Actual', 's3m', 'ready'], ['Actual', 's3m', 'ready']]);
        assert.equal(fixture.playlist.entries[2].display.title, 'Provisional');
        assert.equal(fixture.playlist.current.id, entries[1].id);
        assert.deepEqual(fixture.saved.at(-1).snapshot.entries, fixture.playlist.entries);
        fixture.controls.libraryUpdated(fixture.state.library, change);
        fixture.controls.libraryUpdated(fixture.state.library, { metadataUpdates: [{ locator: row('song.mod'), patch: { sha256: 'other hash' } }] });
        assert.equal(fixture.saved.length, saves + 1);
        const stale = fixture.state.library;
        fixture.state.library = library(); // Same identity, new generation/context.
        fixture.controls.libraryUpdated(stale, { metadataUpdates: [{ locator: row('song.mod'), patch: { title: 'Stale' } }] });
        assert.equal(fixture.playlist.entries[0].display.title, 'Actual');
        assert.equal(fixture.saved.length, saves + 1);
        fixture.controls.libraryUpdated(fixture.state.library, { metadataUpdates: [{ locator: row('song.mod'), patch: { metadataState: 'error' } }] });
        assert.equal(fixture.playlist.entries[0].display.title, 'Actual');
        assert.equal(fixture.playlist.entries[0].display.format, 's3m');
        assert.equal(fixture.playlist.entries[0].display.metadataState, 'error');
    } finally { fixture.restore(); }
});

test('row arrivals already enriched before discovery update legacy playlist snapshots', async () => {
    const fixture = await setup();
    try {
        fixture.playlist.add([item('late.mod')]);
        const changedRowIds = appendDiscoveredRows(fixture.state.library.library, [{ ...row('late.mod'), metadataState: 'ready', title: '', format: 'xm' }]);
        fixture.controls.libraryUpdated(fixture.state.library, { changedRowIds, metadataUpdates: [] });
        assert.equal(fixture.playlist.entries[0].display.title, '');
        assert.equal(fixture.playlist.entries[0].display.format, 'xm');
        assert.equal(fixture.playlist.entries[0].display.metadataState, 'ready');
        const count = fixture.saved.length;
        fixture.controls.libraryUpdated(fixture.state.library, { changedRowIds, metadataUpdates: [] });
        assert.equal(fixture.saved.length, count);
    } finally { fixture.restore(); }
});
