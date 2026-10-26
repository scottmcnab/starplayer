import test from 'node:test';
import assert from 'node:assert/strict';
import { exportPlaylist, importPlaylist, rebindCandidates, rebindEntries } from '../www/playlist-transfer.js';
const identity = { sha256: 'one', name: 'First' };
const library = { identity, library: { rows: [{ sourcePath: 'a.zip', containerMember: '', memberPath: 'é.s3m' }] } };
const entry = { id: 'a', locator: { kind: 'library', libraryIdentity: identity, ...library.library.rows[0], rawHex: 'c3a9' }, display: { title: 'é,"\n', format: 's3m', name: 'é.s3m' } };
test('CSV keeps A–B–A, Unicode/raw hex and library name, excludes uploads with count', () => {
    const second = { ...entry, id: 'b', locator: { ...entry.locator, sourcePath: 'b.s3m', memberPath: '' } };
    const result = exportPlaylist([entry, second, entry, { locator: { kind: 'upload' } }]);
    assert.equal(result.skipped, 1);
    const imported = importPlaylist(result.csv, library);
    assert.deepEqual(imported.items.map(item => item.locator.sourcePath), ['a.zip', 'b.s3m', 'a.zip']);
    assert.equal(imported.items[0].locator.rawHex, 'c3a9');
    assert.equal(imported.items[0].display.title, entry.display.title);
    assert.equal(imported.unavailable, 1);
    assert.match(result.csv, /First/);
});
test('rebind requires matching keys, changes identity explicitly, preserves ids/order', () => {
    const other = { ...library, identity: { sha256: 'two', name: 'Second' } };
    assert.deepEqual(rebindCandidates([entry], library), []);
    assert.deepEqual(rebindCandidates([entry], other), ['a']);
    const rebound = rebindEntries([entry], ['a'], other.identity);
    assert.equal(rebound[0].id, 'a');
    assert.equal(rebound[0].locator.libraryIdentity.sha256, 'two');
    assert.equal(entry.locator.libraryIdentity.sha256, 'one');
    assert.throws(() => importPlaylist('', null), /Open a library/);
});

import { keysForLibrary, locatorKey } from '../www/playlist-transfer.js';
import { createDiscoveredLibrary, appendDiscoveredRows, updateDiscoveredRows, parseCsv, locatorKey as modelLocatorKey } from '../www/library-index.js';
const discovered = phase => ({ identity, mode: 'discovered', library: createDiscoveredLibrary({ name: 'Folder' }), discoveryStatus: { phase } });
const record = sourcePath => ({ sourcePath, containerMember: '', memberPath: '', rawHex: '', format: 'mod', size: 1, timestamp: '', playable: true });

test('membership uses shared locator keys, follows revisions and caches immutable CSV libraries', () => {
    assert.equal(locatorKey, modelLocatorKey);
    const context = discovered('running');
    const empty = keysForLibrary(context.library);
    assert.equal(keysForLibrary(context.library), empty);
    appendDiscoveredRows(context.library, [record('new.mod')]);
    assert.ok(keysForLibrary(context.library).has(locatorKey(record('new.mod'))));
    assert.notEqual(keysForLibrary(context.library), empty);
    assert.equal(keysForLibrary(library.library), keysForLibrary(library.library));
});

for (const phase of ['running', 'paused', 'failed', 'complete']) test(`discovered imports preserve metadata provenance and ${phase} missing counts`, () => {
    const context = discovered(phase);
    appendDiscoveredRows(context.library, [record('unknown.mod'), record('ready.mod'), record('error.mod')]);
    updateDiscoveredRows(context.library, [{ locator: record('ready.mod'), patch: { metadataState: 'ready', title: '', format: 's3m' } }, { locator: record('error.mod'), patch: { metadataState: 'error', error: 'bad' } }]);
    const csv = 'source_path,title,format_detected\nunknown.mod,Claimed,xm\nready.mod,Claimed,xm\nerror.mod,Claimed,xm\nmissing.mod,Claimed,it';
    const result = importPlaylist(csv, context);
    assert.deepEqual(result.items.map(item => item.display.metadataState), ['unknown', 'ready', 'unknown', 'unknown']);
    assert.deepEqual(result.items.map(item => item.display.title), ['Claimed', '', 'Claimed', 'Claimed']);
    assert.deepEqual(result.items.map(item => item.display.format), ['xm', 's3m', 'xm', 'it']);
    assert.equal(result.pending, phase === 'complete' ? 0 : 1);
    assert.equal(result.unavailable, phase === 'complete' ? 1 : 0);
    const exported = parseCsv(exportPlaylist(result.items).csv);
    assert.deepEqual(exported.slice(1).map(row => row[5]), ['', 's3m', '', '']);
    const other = { ...context, identity: { sha256: 'two', name: 'New folder' } };
    assert.deepEqual(rebindCandidates(result.items.map((item, index) => ({ ...item, id: String(index) })), other), phase === 'complete' ? ['0', '1', '2'] : []);
});

test('ready and legacy export actual formats while unknown/error suppress detection', () => {
    const entries = [undefined, 'unknown', 'error', 'ready'].map(metadataState => ({ ...entry, display: { ...entry.display, ...(metadataState ? { metadataState } : {}) } }));
    const rows = parseCsv(exportPlaylist(entries).csv);
    assert.deepEqual(rows.slice(1).map(row => row[5]), ['s3m', '', '', 's3m']);
});
