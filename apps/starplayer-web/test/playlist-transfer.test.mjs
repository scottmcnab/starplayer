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
