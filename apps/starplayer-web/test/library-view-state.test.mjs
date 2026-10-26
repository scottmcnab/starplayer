import test from 'node:test';
import assert from 'node:assert/strict';
import { appendDiscoveredRows, createDiscoveredLibrary, search, updateDiscoveredRows } from '../www/library-index.js';
import { captureAnchor, restoreAnchor, formatOptions, unplayableCount, folderAddLabel, playlistSnapshot, selectedSnapshot, folderSnapshot, progressText, metadataHint } from '../www/library-view-state.js';

const identity = { name: 'Folder', sha256: 'a'.repeat(64) };
const record = (sourcePath, extra = {}) => ({ sourcePath, format: 'mod', playable: true, ...extra });
const makeLibrary = records => {
    const library = createDiscoveredLibrary({ name: 'Folder' });
    appendDiscoveredRows(library, records);
    return library;
};
const status = extra => ({ phase: 'running', tracksFound: 0, filesExamined: 0, archivesPending: 0, issueCount: 0, metadataPhase: 'idle', runtimeError: '', metadataIncomplete: true,
    metadataCounts: { unknown: 2, ready: 0, error: 0, hashesPending: 2 }, sidecarCounts: { joined: 0, unmatched: 0, awaitingHashes: 0 }, ...extra });

test('anchor follows the same stable id and pixel offset when rows append or move', () => {
    const previous = Int32Array.of(1, 2, 3, 4, 5, 6);
    const anchor = captureAnchor(previous, 69);
    assert.deepEqual(anchor, { id: 3, index: 2, offset: 5 });
    assert.equal(restoreAnchor(anchor, previous, Int32Array.of(0, 1, 2, 3, 4, 5, 6), 32), 101);
    assert.equal(restoreAnchor(anchor, previous, Int32Array.of(3, 1, 2, 4, 5, 6), 32), 5);
    assert.equal(restoreAnchor(anchor, previous, Int32Array.of(1, 2, 3, 4, 5, 6, 7), 32), 69);
});

test('removed anchor picks nearest survivor in previous order and clamps after shrinking', () => {
    const previous = Int32Array.of(1, 2, 3, 4, 5, 6);
    const anchor = captureAnchor(previous, 69);
    assert.equal(restoreAnchor(anchor, previous, Int32Array.of(1, 2, 4, 5, 6), 32), 69);
    assert.equal(restoreAnchor(anchor, previous, Int32Array.of(1, 2, 5, 6), 32), 37);
    assert.equal(restoreAnchor(anchor, previous, Int32Array.of(1, 6), 320), 0);
    assert.equal(restoreAnchor(anchor, previous, Int32Array.of(), 32), 0);
    assert.equal(restoreAnchor(anchor, previous, Int32Array.of(9, 10, 11, 12), 32), 69);
    assert.equal(restoreAnchor(captureAnchor([], 0), [], Int32Array.of(9), 32), 0);
    assert.equal(restoreAnchor(captureAnchor(previous, 160), previous, Int32Array.of(6, 1, 2), 64), 0);
});

test('metadata updates refresh sort, format options and first comment search without replacing stable ids', () => {
    const library = makeLibrary([record('B.MOD'), record('A.MOD')]);
    const before = search(library, '', { sort: { column: 'title', descending: false } });
    const anchor = captureAnchor(before, 3);
    assert.deepEqual([...before], [1, 0]);
    assert.equal(library.hasComments, false);
    updateDiscoveredRows(library, [{ locator: library.rows[1], patch: { title: 'Zulu', format: 'xm', metadataState: 'ready', instrumentNames: 'First arrival' } }]);
    const after = search(library, '', { sort: { column: 'title', descending: false } });
    assert.deepEqual([...after], [0, 1]);
    assert.equal(library.rows[1].sourcePath, 'A.MOD');
    assert.equal(library.hasComments, true);
    assert.deepEqual([...search(library, 'text:arrival')], [1]);
    assert.deepEqual([...search(library, '', { format: 'xm' })], [1]);
    assert.equal(restoreAnchor(anchor, before, after, 16), 35);
    assert.deepEqual(formatOptions(library), [{ value: 'mod', label: 'MOD' }, { value: 'xm', label: 'XM' }]);
    updateDiscoveredRows(library, [{ locator: library.rows[0], patch: { format: 'xm' } }]);
    assert.deepEqual(formatOptions(library), [{ value: 'xm', label: 'XM' }]);
    assert.deepEqual(formatOptions(library, 'mod'), [{ value: 'mod', label: 'MOD' }, { value: 'xm', label: 'XM' }]);
});

test('selected snapshots use only playable visible results and carry verified metadata state', () => {
    const library = makeLibrary([record('a.mod'), record('b.mod', { metadataState: 'error', error: 'Bad metadata' }), record('c.mod', { playable: false, error: 'Encrypted' }), record('d.mod', { metadataState: 'ready', title: 'Known' })]);
    const selected = new Set([0, 1, 2, 3]);
    const snapshot = selectedSnapshot(library, Int32Array.of(1, 2, 3), selected, identity);
    assert.deepEqual(snapshot.map(entry => entry.display.metadataState), ['error', 'ready']);
    assert.deepEqual(snapshot.map(entry => entry.display.name), ['b.mod', 'd.mod']);
    assert.equal(snapshot[0].locator.libraryIdentity.sha256, identity.sha256);
    assert.deepEqual([...selected], [0, 1, 2, 3]);
    assert.equal(playlistSnapshot(library, [0], identity)[0].display.metadataState, 'unknown');
    updateDiscoveredRows(library, [{ locator: library.rows[3], patch: { title: 'Later' } }]);
    assert.equal(snapshot[1].display.title, 'Known');
});

test('folder snapshot includes descendants at click time and never later arrivals or unrelated paths', () => {
    const library = makeLibrary([record('demo/a.mod'), record('demo/deep/b.mod'), record('demonstration/c.mod'), record('demo/broken.mod', { playable: false })]);
    const snapshot = folderSnapshot(library, 'demo', identity);
    appendDiscoveredRows(library, [record('demo/later.mod')]);
    assert.deepEqual(snapshot.map(entry => entry.display.name), ['a.mod', 'b.mod']);
    assert.equal(folderSnapshot(library, 'demo', identity).length, 3);
    assert.equal(folderSnapshot(library, '', identity).length, 4);
    for (const phase of ['running', 'paused', 'failed']) assert.equal(folderAddLabel('', { phase }), 'Add discovered tracks in All folders');
    assert.equal(folderAddLabel('demo', { phase: 'complete' }), 'Add folder demo');
    assert.equal(folderAddLabel('', undefined), 'Add folder All folders');
});

test('progress distinguishes empty complete, partial, waiting and suspended runtime failure', () => {
    assert.match(progressText(status()), /Discovering tracks/);
    assert.match(progressText(status({ phase: 'complete' })), /no supported tracks found/);
    assert.match(progressText(status({ phase: 'complete', tracksFound: 3 })), /Discovery complete\. 3 tracks/);
    assert.match(progressText(status({ phase: 'paused', metadataPhase: 'waiting' })), /paused — partial results.*waiting for discovery/);
    assert.match(progressText(status({ phase: 'failed' })), /failed — partial results/);
    assert.match(progressText(status({ phase: 'complete', metadataPhase: 'suspended', runtimeError: 'Worker crashed' })), /suspended.*Background worker error: Worker crashed.*Retry discovery/);
    assert.match(progressText(status({ metadataPhase: 'running' })), /Scanning metadata/);
    assert.match(progressText(status({ metadataPhase: 'cancelled' })), /scan cancelled/);
    assert.match(metadataHint(status()), /searches are incomplete: 2 tracks await metadata/);
    assert.match(metadataHint(status({ sidecarCounts: { joined: 2, unmatched: 3, awaitingHashes: 4 } })), /2 joined, 3 currently unmatched, 4 awaiting metadata\/hashes/);
    assert.match(metadataHint(status({ metadataCounts: { unknown: 0, ready: 2, error: 0, hashesPending: 0 } })), /still need checksums for comments matching/);
    assert.equal(metadataHint(status({ metadataIncomplete: false })), '');
});


test('metadata sorting ties use exact locator spelling rather than arrival ids', () => {
    const paths = ['demo/a.mod', 'demo/A.mod'];
    const first = makeLibrary(paths.map(path => record(path)));
    const second = makeLibrary([...paths].reverse().map(path => record(path)));
    const sortedPaths = library => [...search(library, '', { sort: { column: 'title', descending: false } })].map(id => library.rows[id].sourcePath);
    assert.deepEqual(sortedPaths(first), ['demo/A.mod', 'demo/a.mod']);
    assert.deepEqual(sortedPaths(first), sortedPaths(second));
});


test('unplayable count includes structural failures and excludes metadata errors', () => {
    const library = makeLibrary([record('bad.zip', { memberPath: 'bad.mod', playable: false, error: 'Encrypted' }), record('failed.mod', { metadataState: 'error', error: 'Inspection failed' })]);
    assert.equal(unplayableCount(library), 1);
    appendDiscoveredRows(library, [record('other.mod'), record('unsupported.mod', { playable: false })]);
    assert.equal(unplayableCount(library), 2);
    updateDiscoveredRows(library, [{ locator: library.rows[1], patch: { metadataState: 'ready' } }]);
    assert.equal(unplayableCount(library), 2);
    assert.equal(unplayableCount(makeLibrary([])), 0);
});
