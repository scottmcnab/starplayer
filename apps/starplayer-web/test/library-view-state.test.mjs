import test from 'node:test';
import { createDiscoveryController } from '../www/library-discovery.js';
import assert from 'node:assert/strict';
import { appendDiscoveredRows, createDiscoveredLibrary, search, updateDiscoveredRows, attachComments } from '../www/library-index.js';
import { captureAnchor, restoreAnchor, formatOptions, unplayableCount, folderAddLabel, playlistSnapshot, selectedSnapshot, folderSnapshot, progressText, metadataHint, scanExplanation, createIncrementalViewState, issuePage, createIssuePageState } from '../www/library-view-state.js';

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
    assert.equal(metadataHint(status({ metadataIncomplete: false, metadataCounts: { hashesPending: 0 } })), '');
    assert.match(metadataHint(status({ metadataIncomplete: false })), /2 playback digests pending/);
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


function instrumentation() {
    const operations = {};
    return { operations, onOperation(name, amount = 1) { operations[name] = (operations[name] ?? 0) + amount; }, reset() { for (const name of Object.keys(operations)) delete operations[name]; } };
}
const sortOptions = column => ({ sort: { column, descending: false } });

test('100k rows and 10k issues: status ticks and irrelevant title batches do no global work', () => {
    const library = makeLibrary(Array.from({ length: 100000 }, (_, index) => record(`set${index % 100}/track${String(index).padStart(6, '0')}.mod`)));
    const metrics = instrumentation();
    const state = createIncrementalViewState(library, metrics);
    const before = state.full('', sortOptions('name'));
    assert.equal(metrics.operations.treeBuilds, 1);
    assert.equal(metrics.operations.fullSorts, 1);
    assert.equal(metrics.operations.rowEvaluations, 100000);
    const issues = Array.from({ length: 10000 }, (_, index) => ({ path: `bad${index}`, reason: 'bad' }));
    let reads = 0;
    const pages = createIssuePageState(metrics);
    const publication = { revision: 1, expanded: false, readIssues() { reads += 1; return issues; } };
    pages.update(publication);
    metrics.reset();
    for (let index = 0; index < 100; index += 1) { state.update([]); assert.equal(pages.update(publication), null); }
    assert.deepEqual(metrics.operations, {});
    assert.equal(reads, 0);
    assert.equal(state.order, before);
    const patches = library.rows.slice(0, 100).map(row => ({ locator: row, patch: { title: `Title ${row.id}` } }));
    updateDiscoveredRows(library, patches);
    const result = state.update(library.lastRowChanges);
    assert.equal(result.order, before);
    assert.equal(result.orderChanged, false);
    assert.equal(result.affectedTreePaths.size, 0);
    assert.equal(result.changedRowIds.size, 100);
    assert.deepEqual(metrics.operations, {});
    assert.deepEqual([...state.order], [...search(library, '', sortOptions('name'))]);
    const first = pages.update({ ...publication, expanded: true });
    assert.equal(first.items.length, 100); assert.equal(first.pages, 100);
    assert.equal(metrics.operations.issueMaterializations, 100);
    metrics.reset();
    for (let index = 0; index < 100; index += 1) pages.update({ ...publication, expanded: true });
    assert.deepEqual(metrics.operations, {}); assert.equal(reads, 1);
    const titleOptions = sortOptions('title');
    state.full('title:needle', titleOptions); metrics.reset();
    updateDiscoveredRows(library, library.rows.slice(0, 100).map(row => ({ locator: row, patch: { title: `needle ${String(99 - row.id).padStart(3, '0')}` } })));
    state.update(library.lastRowChanges);
    assert.equal(metrics.operations.rowEvaluations, 100); assert.equal(metrics.operations.fullSorts ?? 0, 0);
    assert.deepEqual([...state.order], [...search(library, 'title:needle', titleOptions)]);
    state.full('', titleOptions); metrics.reset();
    updateDiscoveredRows(library, library.rows.slice(0, 100).map(row => ({ locator: row, patch: { title: `changed ${row.id}` } })));
    state.update(library.lastRowChanges);
    assert.equal(metrics.operations.rowEvaluations ?? 0, 0); assert.equal(metrics.operations.mergeRows, 100000);
    assert.equal(metrics.operations.fullSorts ?? 0, 0); assert.equal(metrics.operations.treeBuilds ?? 0, 0);
    assert.ok(metrics.operations.comparisons < 102000);
    assert.deepEqual([...state.order], [...search(library, '', titleOptions)]);
    console.log('R6 100k/10k: 100 status ticks = 0 sorts/comparisons/tree builds/issue reads; 100 name-sort title patches = 0 row evaluations/comparisons/merges/tree contributions; open issue page = 100 materialized.');
});

test('structural additions update only ancestors and one sorted merge, preserving existing tree objects', () => {
    const library = makeLibrary([record('demo/a.mod'), record('other/c.mod')]);
    const metrics = instrumentation(), state = createIncrementalViewState(library, metrics);
    state.full('', sortOptions('name'));
    const demo = state.nodes.get('demo'), other = state.nodes.get('other');
    metrics.reset();
    appendDiscoveredRows(library, [record('demo/disk.zip', { memberPath: 'b.mod', containerMember: 'inner.zip' }), record('demo/bad.mod', { playable: false })]);
    const result = state.update(library.lastRowChanges);
    assert.equal(state.nodes.get('demo'), demo); assert.equal(state.nodes.get('other'), other);
    assert.equal(state.tree.trackCount, 3); assert.equal(demo.trackCount, 2); assert.equal(other.trackCount, 1);
    assert.equal(state.nodes.get('demo/disk.zip').kind, 'zip');
    assert.equal(state.nodes.get('demo/disk.zip/inner.zip').kind, 'nested');
    assert.equal(result.affectedTreePaths.has('other'), false);
    assert.deepEqual([...state.order], [...search(library, '', sortOptions('name'))]);
    assert.equal(metrics.operations.fullSorts ?? 0, 0); assert.equal(metrics.operations.treeBuilds ?? 0, 0);
    assert.equal(metrics.operations.rowEvaluations, 2); assert.equal(metrics.operations.batchSorts, 1); assert.equal(metrics.operations.mergeRows, 2);
    // Structural mutation is a view contract; current metadata patches never alter playability.
    library.rows[0].playable = false;
    state.update([{ id: 0, fields: ['playable'], added: false }]);
    assert.equal(demo.trackCount, 1); assert.equal(state.tree.trackCount, 2);
    assert.deepEqual([...state.order], [...search(library, '', sortOptions('name'))]);
});

test('title predicates reevaluate only relevant rows, coalesce deltas and preserve locator tie order', () => {
    const library = makeLibrary([record('a.mod'), record('B.mod'), record('b.mod'), record('d.mod')]);
    const metrics = instrumentation(), state = createIncrementalViewState(library, metrics);
    state.full('title:match', sortOptions('title')); metrics.reset();
    updateDiscoveredRows(library, [{ locator: library.rows[2], patch: { title: 'match' } }, { locator: library.rows[1], patch: { title: 'match' } }]);
    const deltas = [...library.lastRowChanges, { id: 1, fields: ['title'], added: false }];
    state.update(deltas);
    assert.deepEqual([...state.order], [1, 2]); assert.equal(metrics.operations.rowEvaluations, 2);
    assert.equal(metrics.operations.fullSorts ?? 0, 0); assert.equal(metrics.operations.treeContributions ?? 0, 0);
    updateDiscoveredRows(library, [{ locator: library.rows[1], patch: { title: 'gone' } }, { locator: library.rows[3], patch: { title: 'match z' } }]);
    state.update(library.lastRowChanges);
    assert.deepEqual([...state.order], [...search(library, 'title:match', sortOptions('title'))]);
    state.full('', sortOptions('title')); metrics.reset();
    updateDiscoveredRows(library, [{ locator: library.rows[0], patch: { title: 'zzz' } }]);
    state.update(library.lastRowChanges);
    assert.equal(metrics.operations.rowEvaluations ?? 0, 0); assert.equal(metrics.operations.mergeRows, 4);
    assert.deepEqual([...state.order], [...search(library, '', sortOptions('title'))]);
});

test('all query fields, filter changes and synchronous sidecar replacement retain indexed semantics', () => {
    const library = makeLibrary([record('demo/a.mod', { sha256: 'a'.repeat(64) }), record('other/b.mod'), record('other/c.mod', { playable: false })]);
    const state = createIncrementalViewState(library);
    for (const query of ['new', 'title:new', '-title:new', 'format:xm', 'ch:>4', 'text:voice', 'tracker:test', 'path:demo', 'name:a']) {
        for (const options of [sortOptions('name'), { ...sortOptions('title'), showUnplayable: true, scopeFolder: 'demo', format: 'xm', includeComments: false }]) {
            state.full(query, options);
            updateDiscoveredRows(library, [{ locator: library.rows[0], patch: { title: 'new', format: 'xm', channels: 8, tracker: 'test', instrumentNames: 'voice' } }]);
            state.update(library.lastRowChanges);
            assert.deepEqual([...state.order], [...search(library, query, options)]);
            updateDiscoveredRows(library, [{ locator: library.rows[0], patch: { title: '', format: 'mod', channels: null, tracker: '', instrumentNames: '' } }]);
        }
    }
    attachComments(library, new Map([['a'.repeat(64), { instrumentNames: 'sidecar', sampleNames: '', songMessage: '' }]]));
    assert.deepEqual([...state.full('text:sidecar', sortOptions('name'))], [0]);
    attachComments(library, null);
    assert.deepEqual([...state.full('text:sidecar', sortOptions('name'))], []);
});

test('bounded issue pages defer readers until open/revision/page and keep append-only page positions', () => {
    const metrics = instrumentation(), state = createIssuePageState(metrics);
    let issues = Array.from({ length: 10001 }, (_, index) => ({ path: String(index), reason: 'bad' }));
    let reads = 0;
    const input = { revision: 1, expanded: false, readIssues() { reads += 1; return issues; } };
    assert.equal(state.update(input), null); assert.equal(reads, 0);
    const last = state.update({ ...input, expanded: true, page: 100 });
    assert.equal(last.items.length, 1); assert.equal(last.items[0].path, '10000');
    assert.equal(state.update({ ...input, expanded: true }), null);
    issues = [...issues, { path: '10001', reason: 'new' }];
    const appended = state.update({ ...input, revision: 2, expanded: true });
    assert.equal(appended.page, 100); assert.equal(appended.items.length, 2);
    assert.equal(metrics.operations.issueMaterializations, 3);
    assert.equal(issuePage(issues, -1).page, 0); assert.equal(issuePage([], 5).pages, 1);
    const replacement = state.update({ ...input, revision: 3, expanded: true, readIssues: () => [] });
    assert.equal(replacement.page, 0); assert.deepEqual(replacement.items, []);
});

test('scan guidance distinguishes excluded, row retry, obsolete digests and current refresh', () => {
    assert.match(scanExplanation(status({ metadataCounts: { scanActionable: 0, excluded: 5 } })), /Excluded tracks cannot be inspected/);
    assert.match(scanExplanation(status({ metadataCounts: { scanActionable: 0, error: 3 } })), /Retry metadata/);
    assert.match(scanExplanation(status({ metadataCounts: { scanActionable: 2 } })), /eligible tracks/);
    const excluded = metadataHint(status({ metadataIncomplete: false, metadataCounts: { excluded: 5, scanActionable: 0 }, sidecarCounts: { excludedHashes: 5 } }));
    assert.match(excluded, /5 excluded tracks cannot be scanned/); assert.match(excluded, /5 excluded hashes cannot be scanned/);
    const failures = metadataHint(status({ metadataCounts: { error: 3, scanActionable: 0 }, sidecarCounts: { failedHashes: 3 } }));
    assert.match(failures, /3 have metadata errors/); assert.match(failures, /3 failed hashes need row Retry/);
    const obsolete = metadataHint(status({ metadataIncomplete: false, metadataCounts: { ready: 1, hashesPending: 1, refreshPending: 0 } }));
    assert.match(obsolete, /superseded playback work/); assert.doesNotMatch(obsolete, /searches are incomplete|current-version refresh/);
    const refresh = metadataHint(status({ metadataCounts: { ready: 1, hashesPending: 2, refreshPending: 1 } }));
    assert.match(refresh, /1 current-version refreshes pending/); assert.match(refresh, /previously verified metadata remains available/);
});


test('changed effective sort key at the same position reuses order and avoids anchor remapping', () => {
    const library = makeLibrary([record('a.mod', { title: 'alpha' }), record('b.mod', { title: 'zulu' })]);
    const metrics = instrumentation(), state = createIncrementalViewState(library, metrics);
    const previous = state.full('', sortOptions('title'));
    metrics.reset();
    updateDiscoveredRows(library, [{ locator: library.rows[0], patch: { title: 'bravo' } }]);
    const result = state.update(library.lastRowChanges);
    assert.equal(result.orderChanged, false); assert.equal(result.order, previous);
    assert.equal(metrics.operations.mergeRows, 2); assert.equal(metrics.operations.fullSorts ?? 0, 0);
    metrics.reset();
    updateDiscoveredRows(library, [{ locator: library.rows[0], patch: { title: 'Bravo' } }]);
    const caseOnly = state.update(library.lastRowChanges);
    assert.equal(caseOnly.orderChanged, false); assert.equal(caseOnly.order, previous);
    assert.deepEqual(metrics.operations, {});
});


test('real discovery publications keep row, status and issue revisions coherent through playback and replacement', async t => {
    let releaseWalk, resolveDigest;
    const gate = new Promise(resolve => { releaseWalk = resolve; });
    const digest = new Promise(resolve => { resolveDigest = resolve; });
    const source = { label: 'folder', async *enumerate() {
        yield { kind: 'file', path: 'a.mod' };
        yield { kind: 'issue', path: 'denied', reason: 'unreadable' };
        await gate;
        yield { kind: 'file', path: 'late.mod' };
    } };
    const context = { source, library: createDiscoveredLibrary({ name: 'folder' }) };
    const metrics = instrumentation(), issues = createIssuePageState(metrics), timers = new Map(), publications = [];
    let active = context, state = createIncrementalViewState(context.library, metrics), nextTimer = 0;
    state.full('', sortOptions('title'));
    const controller = createDiscoveryController({ context, generation: 5, digest: () => digest,
        runtimeFactory: () => ({ pauseCatalog() {}, resumeCatalog() {}, dispose() {}, cancelMetadata() {}, enqueue() { throw new Error('loose discovery reads no bytes'); } }),
        schedule(callback) { timers.set(++nextTimer, callback); return nextTimer; }, unschedule(id) { timers.delete(id); },
        onUpdate(next, change) {
            if (next !== active) return;
            state.update(change.rowChanges);
            const status = next.discoveryStatus;
            assert.equal(change.libraryRevision, next.library.revision);
            assert.equal(change.issuesRevision, status.issuesRevision);
            assert.equal(status.tracksFound, next.library.rows.length);
            assert.equal(status.issueCount, status.issues.length);
            assert.equal(status.metadataCounts.unknown + status.metadataCounts.ready + status.metadataCounts.error + status.metadataCounts.excluded, status.tracksFound);
            assert.deepEqual([...state.order], [...search(next.library, '', sortOptions('title'))]);
            issues.update({ revision: change.issuesRevision, expanded: true, readIssues: () => status.issues });
            publications.push({ change, status });
        } });
    t.after(() => { controller.dispose(); releaseWalk(); });
    const settle = async () => { for (let index = 0; index < 30; index += 1) await Promise.resolve(); };
    const publish = () => { for (const [id, callback] of [...timers]) { timers.delete(id); callback(); } };
    controller.start(); await settle(); publish();
    assert.equal(publications.length, 1); assert.equal(state.tree.trackCount, 1);
    assert.equal(publications[0].change.rowChanges[0].added, true); assert.equal(publications[0].change.issuesRevision, 1);
    metrics.reset();
    controller.acceptPlaybackMetadata(context.library.rows[0], { title: 'verified', format: 'mod', channels: 4 }, Uint8Array.of(1));
    publish();
    assert.equal(publications.at(-1).change.rowChanges.length, 0);
    assert.equal(publications.at(-1).status.metadataCounts.refreshPending, 1);
    assert.deepEqual(metrics.operations, {});
    resolveDigest(new Uint8Array(32).fill(10).buffer); await settle(); publish();
    assert.equal(context.library.rows[0].title, 'verified'); assert.equal(context.library.rows[0].sha256, '0a'.repeat(32));
    assert.equal(publications.at(-1).status.metadataCounts.ready, 1);
    assert.equal(publications.at(-1).status.metadataCounts.refreshPending, 0);
    assert.equal(metrics.operations.treeBuilds ?? 0, 0); assert.equal(metrics.operations.issueMaterializations ?? 0, 0);
    active = { library: makeLibrary([record('replacement.mod')]) };
    state = createIncrementalViewState(active.library, metrics);
    const replacementOrder = state.full('', sortOptions('title'));
    const count = publications.length;
    controller.pause(); publish();
    assert.equal(publications.length, count, 'obsolete accepted context does not publish into replacement');
    controller.resume();
    const staleCallbacks = [...timers.values()];
    controller.dispose(); releaseWalk(); await settle();
    for (const callback of staleCallbacks) callback();
    assert.equal(publications.length, count); assert.equal(state.order, replacementOrder);
    assert.deepEqual([...state.order], [0]); assert.equal(active.library.rows[0].sourcePath, 'replacement.mod');
});


test('folder structural changes retire zero-reference branches while preserving unaffected nodes', () => {
    const library = makeLibrary([record('old/deep/a.mod'), record('stable/b.mod')]);
    const metrics = instrumentation(), state = createIncrementalViewState(library, metrics);
    state.full('', sortOptions('name'));
    const stable = state.nodes.get('stable');
    metrics.reset();
    library.rows[0].folder = 'new/deep'; library.sortKeys[0].folder = 'new/deep';
    const result = state.update([{ id: 0, fields: ['folder'], added: false }]);
    assert.equal(state.nodes.has('old'), false); assert.equal(state.nodes.has('old/deep'), false);
    assert.equal(state.tree.children.has('old'), false); assert.equal(state.tree.children.has('new'), true);
    assert.equal(state.nodes.get('new/deep').trackCount, 1); assert.equal(state.tree.trackCount, 2);
    assert.equal(state.nodes.get('stable'), stable); assert.equal(result.affectedTreePaths.has('stable'), false);
    assert.deepEqual([...state.order], [...search(library, '', sortOptions('name'))]);
    assert.equal(metrics.operations.treeBuilds ?? 0, 0); assert.equal(metrics.operations.treeContributions, 2);
});
