import test from 'node:test';
import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { createDiscoveredLibrary, appendDiscoveredRows, updateDiscoveredRows, locatorKey, parseLibraryCsv, attachComments, search, buildTree, createRowPredicate, createRowComparator } from '../www/library-index.js';

const hash = digit => digit.repeat(64);
const record = (sourcePath, fields = {}) => ({ sourcePath, format: 'mod', size: null, timestamp: '', playable: true, ...fields });
const update = (sourcePath, patch, locator = {}) => ({ locator: { sourcePath, ...locator }, patch });
const found = (library, query, options) => [...search(library, query, options)];

test('empty libraries and append defaults preserve the CSV shape without guessing metadata', () => {
    const library = createDiscoveredLibrary({ name: 'Folder' });
    assert.equal(library.identityName, 'Folder');
    assert.equal(library.revision, 0);
    assert.deepEqual(found(library, ''), []);
    assert.deepEqual(buildTree(library), { path: '', label: '', kind: 'dir', children: [], trackCount: 0 });
    assert.deepEqual(attachComments(library, null), { joined: 0, unmatched: 0 });
    assert.equal(library.revision, 0);
    assert.deepEqual(appendDiscoveredRows(library, [record('music/Track.MTM', { format: '' })]), [0]);
    assert.deepEqual(library.rows[0], {
        id: 0, sourcePath: 'music/Track.MTM', sourceUrl: '', containerMember: '', memberPath: '', rawHex: '', name: 'Track.MTM', folder: 'music',
        format: 'mtm', playable: true, channels: null, size: null, timestamp: '', sha256: '', line: null, metadataState: 'unknown',
        title: '', tracker: '', instrumentNames: '', sampleNames: '', songMessage: '', error: '',
    });
    assert.equal(library.locatorIds.get(locatorKey({ sourcePath: 'music/Track.MTM' })), 0);
    assert.equal(library.revision, 1);
    assert.equal(library.hasComments, false);
    assert.deepEqual(library.commentsFor(0), { instrumentNames: '', sampleNames: '', songMessage: '' });
    assert.deepEqual(appendDiscoveredRows(library, []), []);
    assert.deepEqual(updateDiscoveredRows(library, []), []);
    assert.equal(library.revision, 1);
});

test('append validates types, preserves archive locators, and deduplicates case-sensitive tuples', () => {
    const library = createDiscoveredLibrary();
    const first = record('packs/a.zip', { containerMember: 'disk\\inner.zip', memberPath: 'SONGS\\A.XM', rawHex: 'abCD', format: ' XM ', size: 12,
        title: 12, channels: '4', tracker: {}, instrumentNames: [], sampleNames: false, songMessage: null, sha256: {}, timestamp: 0 });
    const second = record('packs/a.zip', { containerMember: 'disk\\inner.zip', memberPath: 'SONGS\\a.xm', format: 'XM', channels: 8 });
    assert.deepEqual(appendDiscoveredRows(library, [first, { ...first, title: 'duplicate', rawHex: 'different' }, second,
        record('other.mod', { channels: 0, size: -1 }), record('invalid.mod', { playable: 'true', channels: Infinity, size: NaN }),
        record('../bad.mod'), record('a//bad.mod'), record('bad.zip', { memberPath: 42 }), null]), [0, 1, 2, 3]);
    assert.equal(library.revision, 1);
    assert.equal(library.locatorIds.size, 4);
    assert.equal(library.issues.length, 4);
    assert.deepEqual(library.issues[0], { path: '../bad.mod', reason: 'invalid relative path: ../bad.mod' });
    assert.equal(library.rows[0].rawHex, 'abCD');
    assert.equal(library.rows[0].folder, 'packs/a.zip/disk/inner.zip/SONGS');
    assert.equal(library.rows[0].title, '');
    assert.equal(library.rows[0].tracker, '');
    assert.equal(library.rows[0].sha256, '');
    assert.equal(library.rows[0].timestamp, '');
    assert.equal(library.rows[0].channels, null);
    assert.equal(library.rows[1].channels, 8);
    assert.equal(library.rows[2].size, null);
    assert.equal(library.rows[3].playable, false);
    assert.deepEqual(appendDiscoveredRows(library, [first]), []);
    assert.equal(library.revision, 1);
    assert.equal(locatorKey(first), JSON.stringify(['packs/a.zip', 'disk\\inner.zip', 'SONGS\\A.XM']));
});

test('metadata patches update only allowed fields and identify stable IDs by locator', () => {
    const library = createDiscoveredLibrary();
    appendDiscoveredRows(library, [record('z.mod', { size: 12, timestamp: '1995' }), record('a.mod')]);
    const original = library.rows[0];
    assert.deepEqual(updateDiscoveredRows(library, [update('z.mod', { title: 'Café Moon', channels: 12, format: ' XM ', tracker: 'Träckér',
        instrumentNames: 'Purple\nMotion', sampleNames: 'Kick', songMessage: '', sha256: ` ${hash('A')} `, metadataState: 'ready',
        id: 99, name: 'wrong', folder: 'elsewhere', sourcePath: 'moved.mod', playable: false, size: 100, timestamp: '2000', rawHex: 'ab', sourceUrl: 'https://invalid/' }),
        update('not-yet.mod', { title: 'Do not retain' })]), [0]);
    assert.equal(library.rows[0], original);
    assert.equal(library.rows[0].id, 0);
    assert.equal(library.rows[0].name, 'z.mod');
    assert.equal(library.rows[0].folder, '');
    assert.equal(library.rows[0].sourcePath, 'z.mod');
    assert.equal(library.rows[0].playable, true);
    assert.equal(library.rows[0].size, 12);
    assert.equal(library.rows[0].timestamp, '1995');
    assert.equal(library.rows[0].rawHex, '');
    assert.equal(library.rows[0].sourceUrl, '');
    assert.equal(library.rows[0].sha256, hash('a'));
    assert.equal(library.revision, 2);
    assert.deepEqual(found(library, 'title:cafe tracker:tracker ch:>8 format:xm'), [0]);
    assert.deepEqual(found(library, 'text:purple-motion'), [0]);
    assert.deepEqual(found(library, 'purplemotion', { includeComments: false }), []);
    assert.deepEqual([...library.formats].sort(), ['mod', 'xm']);
    assert.deepEqual(updateDiscoveredRows(library, [update('z.mod', { title: 'Café Moon', metadataState: 'invalid' })]), []);
    assert.equal(library.revision, 2);
    assert.deepEqual(updateDiscoveredRows(library, [update('z.mod', { title: 'First' }), update('z.mod', { title: 'Last' }), update('a.mod', { title: 'A' })]), [0, 1]);
    assert.equal(library.revision, 3);
    assert.equal(library.rows[0].channels, 12, 'missing fields retain their values');
    assert.equal(library.rows[0].title, 'Last');
    assert.deepEqual(updateDiscoveredRows(library, [update('z.mod', { channels: '4', title: {}, tracker: null, sha256: 'invalid' })]), [0]);
    assert.equal(library.rows[0].channels, null);
    assert.equal(library.rows[0].title, '');
    assert.equal(library.rows[0].tracker, '');
    assert.equal(library.rows[0].sha256, '');
});

test('metadata errors stay playable and ready retries clear errors; structural errors remain visible', () => {
    const library = createDiscoveredLibrary();
    appendDiscoveredRows(library, [record('loose.mod'), record('pack.zip', { memberPath: 'huge.mod', playable: false, error: 'Entry exceeds size limit' })]);
    const archiveLocator = { memberPath: 'huge.mod' };
    assert.deepEqual(updateDiscoveredRows(library, [update('loose.mod', { metadataState: 'error', error: 'Invalid module' })]), [0]);
    assert.equal(library.rows[0].playable, true);
    assert.equal(library.rows[0].title, '');
    assert.equal(library.rows[0].error, 'Invalid module');
    assert.deepEqual(found(library, ''), [0]);
    assert.deepEqual(found(library, '', { showUnplayable: true }), [0, 1]);
    assert.equal(library.folderCounts.get(''), 1);
    updateDiscoveredRows(library, [update('loose.mod', { title: 'Recovered', metadataState: 'ready' }), update('pack.zip', { playable: true, metadataState: 'ready', error: '' }, archiveLocator)]);
    assert.equal(library.rows[0].error, '');
    assert.equal(library.rows[0].metadataState, 'ready');
    assert.equal(library.rows[1].playable, false);
    assert.equal(library.rows[1].error, 'Entry exceeds size limit');
    assert.equal(buildTree(library).trackCount, 1);
});

test('metadata enrichment and CSV rows share search, field, comment and sort preparation', () => {
    const library = createDiscoveredLibrary();
    appendDiscoveredRows(library, [record('music/a.mod'), record('packs/a.zip', { containerMember: 'disk/inner.zip', memberPath: 'music/b.xm', format: 'xm', size: 50 })]);
    const metadata = { title: 'Café', tracker: 'Tracker', channels: 4, instrumentNames: 'Purple Motion', sampleNames: 'Kick', metadataState: 'ready' };
    updateDiscoveredRows(library, [update('music/a.mod', metadata), update('packs/a.zip', { ...metadata, title: 'Other', channels: 8 }, { containerMember: 'disk/inner.zip', memberPath: 'music/b.xm' })]);
    const csv = parseLibraryCsv('source_path,container_member,member_path,size,title,tracker,channels,instrument_names,sample_names\nmusic/a.mod,,,,Café,Tracker,4,Purple Motion,Kick\npacks/a.zip,disk/inner.zip,music/b.xm,50,Other,Tracker,8,Purple Motion,Kick');
    for (const field of ['haystacks', 'compactHaystacks', 'fieldHaystacks', 'commentHaystacks', 'compactCommentHaystacks', 'sortKeys']) assert.deepEqual(library[field], csv[field], field);
    for (const query of ['', 'cafe', 'text:purplemotion', 'tracker:track', 'path:inner.zip', 'ch:>4', '-title:cafe']) assert.deepEqual(found(library, query), found(csv, query), query);
    for (const column of ['name', 'title', 'format', 'channels', 'size', 'timestamp', 'folder']) for (const descending of [false, true]) assert.deepEqual(found(library, '', { sort: { column, descending } }), found(csv, '', { sort: { column, descending } }), column);
    assert.deepEqual(buildTree(library), buildTree(csv));
    assert.equal(csv.locatorIds.get(locatorKey({ sourcePath: 'music/a.mod' })), 0);
});

test('case-sensitive full locator tuples break folded sort ties independently of arrival order', () => {
    const records = [
        record('pack.zip', { containerMember: 'inner.zip', memberPath: 'music/A.MOD' }),
        record('pack.zip', { memberPath: 'inner.zip/music/A.MOD' }),
        record('PACK.zip', { memberPath: 'inner.zip/music/A.MOD' }),
        record('pack.zip', { containerMember: 'inner.zip', memberPath: 'music/a.mod' }),
    ];
    const sortedKeys = rows => {
        const library = createDiscoveredLibrary();
        appendDiscoveredRows(library, rows);
        return found(library, '', { sort: { column: 'title' } }).map(id => locatorKey(library.rows[id]));
    };
    const expected = [records[2], records[1], records[0], records[3]].map(locatorKey);
    assert.deepEqual(sortedKeys(records), expected);
    assert.deepEqual(sortedKeys([...records].reverse()), expected);
    assert.deepEqual(sortedKeys([records[1], records[3], records[2], records[0]]), expected);
});

test('sidecars join before or after hashes and replacement removes old text without losing intrinsic names', () => {
    const library = createDiscoveredLibrary();
    const comments = new Map([[hash('a'), { instrumentNames: 'Sidecar', sampleNames: '', songMessage: 'Greetings' }], [hash('b'), { instrumentNames: 'Unmatched' }]]);
    assert.deepEqual(attachComments(library, comments), { joined: 0, unmatched: 2 });
    assert.equal(library.revision, 1);
    appendDiscoveredRows(library, [record('a.mod', { instrumentNames: 'Original', sampleNames: 'Original sample' }), record('b.mod', { sha256: hash('a') }), record('c.mod', { sha256: hash('c') })]);
    assert.equal(library.comments, comments);
    assert.deepEqual(library.commentCounts, { joined: 1, unmatched: 1, awaitingHashes: 1, failedHashes: 0, excludedHashes: 0 });
    assert.deepEqual(found(library, 'text:sidecar'), [1]);
    assert.deepEqual(updateDiscoveredRows(library, [update('a.mod', { sha256: hash('a'), instrumentNames: 'Extracted', metadataState: 'ready' })]), [0]);
    assert.deepEqual(library.commentCounts, { joined: 2, unmatched: 1, awaitingHashes: 0, failedHashes: 0, excludedHashes: 0 });
    assert.equal(library.commentsFor(0), library.commentsFor(1), 'same sidecar record shares display text');
    assert.deepEqual(found(library, 'text:sidecar'), [0, 1]);
    assert.deepEqual(found(library, 'text:extracted'), []);
    assert.equal(library.commentsFor(0).sampleNames, '', 'sidecar empty fields override intrinsic text');
    const replacement = new Map([[hash('b'), { instrumentNames: 'Replacement' }]]);
    assert.deepEqual(attachComments(library, replacement), { joined: 0, unmatched: 1 });
    assert.deepEqual(library.commentChangedRowIds, [0, 1]);
    assert.deepEqual(found(library, 'text:sidecar'), []);
    assert.deepEqual(found(library, 'text:greetings'), []);
    assert.deepEqual(found(library, 'text:extracted'), [0]);
    assert.equal(library.commentsFor(0).sampleNames, 'Original sample');
    updateDiscoveredRows(library, [update('b.mod', { sha256: hash('b') }), update('c.mod', { sha256: hash('b') })]);
    assert.deepEqual(library.commentCounts, { joined: 2, unmatched: 0, awaitingHashes: 0, failedHashes: 0, excludedHashes: 0 });
    updateDiscoveredRows(library, [update('b.mod', { sha256: '' })]);
    assert.deepEqual(library.commentCounts, { joined: 1, unmatched: 0, awaitingHashes: 1, failedHashes: 0, excludedHashes: 0 });
    assert.deepEqual(attachComments(library, null), { joined: 0, unmatched: 0 });
    assert.deepEqual(library.commentCounts, { joined: 0, unmatched: 0, awaitingHashes: 0, failedHashes: 0, excludedHashes: 0 });
    assert.deepEqual(found(library, 'text:replacement'), []);
    updateDiscoveredRows(library, [update('a.mod', { instrumentNames: '', sampleNames: '' })]);
    assert.equal(library.hasComments, false);
});

test('40,000 records append and enrich in batches without visiting prior rows or rebuilding indexes', () => {
    const library = createDiscoveredLibrary();
    const records = Array.from({ length: 40000 }, (_, index) => record(`music/folder${index % 100}/track${index}.MOD`, { size: index }));
    const arrays = Object.fromEntries(['haystacks', 'compactHaystacks', 'fieldHaystacks', 'sortKeys', 'commentHaystacks', 'compactCommentHaystacks'].map(field => [field, library[field]]));
    const rows = library.rows;
    library.rows = new Proxy(rows, { get(target, key, receiver) {
        assert.notEqual(key, Symbol.iterator, 'incremental model must not walk all rows');
        if (typeof key === 'string') assert.ok(!['map', 'forEach', 'filter', 'reduce', 'slice'].includes(key), `incremental model must not ${key} all rows`);
        return Reflect.get(target, key, receiver);
    } });
    const started = performance.now();
    for (let start = 0; start < records.length; start += 200) {
        const changed = appendDiscoveredRows(library, records.slice(start, start + 200));
        assert.equal(changed.length, 200);
        assert.equal(changed[0], start);
        assert.equal(changed.at(-1), start + 199);
    }
    const appendTime = performance.now() - started;
    const originalFieldIndex = library.fieldHaystacks[0], originalSortIndex = library.sortKeys[0];
    for (let start = 200; start < records.length; start += 200) {
        assert.equal(updateDiscoveredRows(library, records.slice(start, start + 200).map(row => ({ locator: row, patch: { title: 'Inspected', channels: 4, metadataState: 'ready' } }))).length, 200);
    }
    const enrichedTime = performance.now() - started - appendTime;
    library.rows = rows;
    assert.equal(library.rows.length, 40000);
    assert.equal(library.locatorIds.size, 40000);
    assert.equal(library.revision, 399);
    assert.equal(library.fieldHaystacks[0], originalFieldIndex, 'unaffected field index retains its object');
    assert.equal(library.sortKeys[0], originalSortIndex, 'unaffected sort keys retain their object');
    for (const [field, array] of Object.entries(arrays)) { assert.equal(library[field], array, field); assert.equal(array.length, 40000, field); }
    assert.equal(found(library, 'title:inspected ch:4').length, 39800);
    assert.equal(found(library, 'name:track39999').length, 1);
    assert.equal(buildTree(library).trackCount, 40000);
    assert.equal(library.folderCounts.get('music'), 40000);
    for (let folder = 0; folder < 100; folder += 1) assert.equal(library.folderCounts.get(`music/folder${folder}`), 400);
    console.log(`Incremental 40,000 rows: append ${appendTime.toFixed(1)} ms, enrich ${enrichedTime.toFixed(1)} ms`);
});


test('model deltas describe changed public domains and resolved comments exactly', () => {
    const library = createDiscoveredLibrary();
    appendDiscoveredRows(library, [record('a.mod')]);
    assert.deepEqual(library.lastRowChanges, [{ id: 0, added: true, fields: ['*'] }]);
    updateDiscoveredRows(library, [update('a.mod', { title: 'Title' })]);
    assert.deepEqual(library.lastRowChanges, [{ id: 0, added: false, fields: ['title'] }]);
    updateDiscoveredRows(library, [update('a.mod', { instrumentNames: 'Names' })]);
    assert.deepEqual(library.lastRowChanges, [{ id: 0, added: false, fields: ['comments'] }]);
    updateDiscoveredRows(library, [update('a.mod', { sha256: hash('a') })]);
    assert.deepEqual(library.lastRowChanges, [{ id: 0, added: false, fields: ['sha256'] }]);
    attachComments(library, new Map([[hash('a'), { instrumentNames: 'Names' }]]));
    assert.deepEqual(library.lastRowChanges, [], 'SHA joining identical display text is not a comments change');
    updateDiscoveredRows(library, [update('a.mod', { instrumentNames: 'Hidden intrinsic names' })]);
    assert.deepEqual(library.lastRowChanges, [], 'intrinsic names hidden by sidecar change no public index domain');
    attachComments(library, null);
    assert.deepEqual(library.lastRowChanges, [{ id: 0, added: false, fields: ['comments'] }]);
    updateDiscoveredRows(library, []); assert.deepEqual(library.lastRowChanges, []);
    updateDiscoveredRows(library, [update('a.mod', { title: 'Intermediate' }), update('a.mod', { title: 'Title' })]);
    assert.deepEqual(library.lastRowChanges, [], 'delta is final batch effect');
});

test('sidecar hash accounting snapshots eligibility and error state across patches and replacement', () => {
    for (const attachFirst of [false, true]) {
        const library = createDiscoveredLibrary();
        const sidecar = new Map([[hash('a'), { instrumentNames: 'Sidecar' }]]);
        if (attachFirst) attachComments(library, sidecar);
        appendDiscoveredRows(library, [record('unknown.mod'), record('error.mod', { metadataState: 'error' }),
            record('locked.zip', { memberPath: 'locked.mod', playable: false, error: 'encrypted' }),
            record('known.zip', { memberPath: 'locked.mod', playable: false, sha256: hash('a') })]);
        if (!attachFirst) attachComments(library, sidecar);
        assert.deepEqual(library.commentCounts, { joined: 1, unmatched: 0, awaitingHashes: 1, failedHashes: 1, excludedHashes: 1 });
        updateDiscoveredRows(library, [update('unknown.mod', { metadataState: 'error' }), update('error.mod', { metadataState: 'ready', sha256: hash('a') })]);
        assert.deepEqual(library.commentCounts, { joined: 2, unmatched: 0, awaitingHashes: 0, failedHashes: 1, excludedHashes: 1 });
        updateDiscoveredRows(library, [update('unknown.mod', { metadataState: 'ready' })]);
        assert.deepEqual(library.commentCounts, { joined: 2, unmatched: 0, awaitingHashes: 1, failedHashes: 0, excludedHashes: 1 });
        attachComments(library, new Map([[hash('b'), { instrumentNames: 'Other' }]]));
        assert.deepEqual(library.commentCounts, { joined: 0, unmatched: 1, awaitingHashes: 1, failedHashes: 0, excludedHashes: 1 });
        attachComments(library, null);
        assert.deepEqual(library.commentCounts, { joined: 0, unmatched: 0, awaitingHashes: 0, failedHashes: 0, excludedHashes: 0 });
    }
});

test('incremental predicate and comparator preserve full search semantics', () => {
    const library = createDiscoveredLibrary();
    appendDiscoveredRows(library, [record('music/A.mod', { title: 'Café', channels: 4, instrumentNames: 'Purple Motion', size: 5 }),
        record('music/a.mod', { title: 'Cafe', channels: 8, size: 1 }), record('other/b.it', { format: 'it', playable: false })]);
    for (const query of ['', 'text:purplemotion', '-title:cafe', 'ch:>4', 'title:cafe']) {
        for (const options of [{}, { scopeFolder: 'music', includeComments: false }, { showUnplayable: true }, { format: 'MOD' }]) {
            for (const column of ['title', 'name', 'folder', 'channels', 'size', 'timestamp', 'format']) {
                const sort = { column, descending: true };
                const actual = library.rows.filter(createRowPredicate(library, query, options)).map(row => row.id).sort(createRowComparator(library, sort));
                assert.deepEqual(actual, found(library, query, { ...options, sort }));
            }
        }
    }
});
