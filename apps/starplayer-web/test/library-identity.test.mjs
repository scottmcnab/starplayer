import test from 'node:test';
import assert from 'node:assert/strict';

// A deliberately small in-memory IndexedDB, including asynchronous upgrade/transaction events.
function memoryDatabase({ version = 0, stores = {}, failure = '', blocked = false } = {}) {
    const state = { version, stores: new Map(Object.entries(stores).map(([name, records]) => [name, new Map(records)])), failure, blocked, opens: [], databases: [], writes: 0, closes: 0 };
    state.open = (name, requestedVersion) => {
        assert.equal(name, 'starplayer-library');
        state.opens.push(requestedVersion);
        const request = {};
        queueMicrotask(() => {
            if (state.failure === 'open') {
                request.error = new DOMException('storage denied', 'SecurityError');
                request.onerror();
                return;
            }
            const database = {
                objectStoreNames: { contains: name => state.stores.has(name) },
                createObjectStore(name) { state.stores.set(name, new Map()); },
                close() { state.closes += 1; },
                transaction(name, mode) {
                    if (state.failure === 'transaction') throw new Error('transaction denied');
                    assert.equal(state.stores.has(name), true);
                    const transaction = { objectStore() { return {
                        get: key => perform(() => state.stores.get(name).get(key)),
                        getAll: () => perform(() => [...state.stores.get(name).values()]),
                        put: (value, key) => {
                            assert.equal(mode, 'readwrite');
                            return perform(() => { state.writes += 1; state.stores.get(name).set(key, value); return key; }, true);
                        },
                    }; } };
                    function perform(operation, writing = false) {
                        const result = {};
                        queueMicrotask(() => {
                            if (state.failure === 'abort' || (writing && state.failure === 'write')) {
                                transaction.error = new Error('storage write aborted');
                                transaction.onabort();
                            } else {
                                result.result = operation();
                                transaction.oncomplete();
                            }
                        });
                        return result;
                    }
                    return transaction;
                },
            };
            state.databases.push(database);
            request.result = database;
            if (state.blocked) request.onblocked();
            if (state.version < requestedVersion) {
                state.version = requestedVersion;
                request.onupgradeneeded();
            }
            request.onsuccess();
        });
        return request;
    };
    return state;
}

let moduleNumber = 0;
async function identities(database) {
    globalThis.indexedDB = database;
    return import(`../www/library-identity.js?identity-test=${++moduleNumber}`);
}
const token = character => character.repeat(64);
const handle = (key, name = 'Music') => ({ name, async isSameEntry(other) { return key === other.key; }, key });
const source = directory => ({ kind: 'directory', label: directory.name, handle: directory });

test('v2 upgrade retains root/remote records and migrates legacy handles only on discovered use', async () => {
    const root = handle('legacy');
    const remote = { name: 'Remote CSV', bytes: new Uint8Array([1, 2]) };
    const database = memoryDatabase({ version: 2, stores: { handles: [['root', root]], indexes: [['last', remote]] } });
    const { folderIdentity, storedLibraryValue } = await identities(database);
    assert.equal(await storedLibraryValue('handles', 'root'), root);
    assert.deepEqual(await storedLibraryValue('indexes', 'last'), remote);
    assert.equal(database.version, 3);
    assert.equal(database.stores.get('folderIdentities').size, 0);
    const result = await folderIdentity(source(root));
    assert.equal(result.storageFailed, false);
    assert.match(result.identity.sha256, /^[a-f0-9]{64}$/);
    assert.equal(result.identity.name, 'Music');
    assert.deepEqual(database.stores.get('folderIdentities').get(result.identity.sha256), { handle: root, identity: result.identity });
    assert.equal(await storedLibraryValue('handles', 'root'), root);
    assert.deepEqual(await storedLibraryValue('indexes', 'last'), remote);
    assert.deepEqual([...database.stores.keys()].sort(), ['folderIdentities', 'handles', 'indexes']);
    assert.equal(database.opens.every(version => version === 3), true);
});

test('same-entry reselections A to B to A reuse identity; same names do not imply same directory', async () => {
    const database = memoryDatabase();
    const { folderIdentity } = await identities(database);
    const first = await folderIdentity(source(handle('A')));
    const second = await folderIdentity(source(handle('B')));
    const again = await folderIdentity(source(handle('A')));
    assert.notEqual(first.identity.sha256, second.identity.sha256);
    assert.deepEqual(again.identity, first.identity);
    assert.equal(database.stores.get('folderIdentities').size, 2);
    assert.equal(database.writes, 2);
});

test('persisted identity is found with isSameEntry on another page, retained through temporary denial', async () => {
    const oldIdentity = { sha256: token('a'), name: 'Old label' };
    const database = memoryDatabase({ version: 3, stores: { handles: [], indexes: [], folderIdentities: [[oldIdentity.sha256, { handle: handle('A'), identity: oldIdentity }]] } });
    const { folderIdentity } = await identities(database);
    const first = await folderIdentity(source(handle('A', 'New label')));
    assert.deepEqual(first, { identity: oldIdentity, storageFailed: false });
    database.failure = 'open';
    const next = await folderIdentity(source(handle('A', 'New label')));
    assert.deepEqual(next, { identity: oldIdentity, storageFailed: true });
    assert.equal(database.writes, 0);
});

test('concurrent same-page lookup/create mints one token, including same accepted source reuse', async () => {
    const database = memoryDatabase();
    const { folderIdentity } = await identities(database);
    const accepted = source(handle('A'));
    const results = await Promise.all([folderIdentity(accepted), folderIdentity(source(handle('A'))), folderIdentity(accepted), folderIdentity(source(handle('A')))]);
    assert.equal(new Set(results.map(result => result.identity.sha256)).size, 1);
    assert.equal(database.writes, 1);
    assert.equal(results[0], results[2]);
});

test('FileList selections receive distinct source-stable session tokens without opening storage', async () => {
    const database = memoryDatabase();
    const { folderIdentity } = await identities(database);
    const accepted = { kind: 'filelist', label: 'Music' };
    const first = await folderIdentity(accepted);
    const again = await folderIdentity(accepted);
    const reselected = await folderIdentity({ kind: 'filelist', label: 'Music' });
    assert.equal(first, again);
    assert.equal(first.storageFailed, true);
    assert.match(first.identity.sha256, /^[a-f0-9]{64}$/);
    assert.notEqual(first.identity.sha256, reselected.identity.sha256);
    assert.equal(database.opens.length, 0);
});

test('storage denial and transaction/write failure keep selected folders usable with session reuse', async () => {
    for (const failure of ['open', 'transaction', 'write', 'abort']) {
        const database = memoryDatabase({ failure });
        const { folderIdentity } = await identities(database);
        const accepted = source(handle('A'));
        const first = await folderIdentity(accepted);
        const second = await folderIdentity(source(handle('B')));
        const again = await folderIdentity(source(handle('A')));
        assert.equal(first.storageFailed, true, failure);
        assert.match(first.identity.sha256, /^[a-f0-9]{64}$/);
        assert.notEqual(second.identity.sha256, first.identity.sha256);
        assert.deepEqual(again.identity, first.identity);
        assert.equal(await folderIdentity(accepted), first);
    }
});

test('comparison failure skips that record, finds later matches, and uses session fallback if uncertain', async () => {
    const oldIdentity = { sha256: token('b'), name: 'Old' };
    const match = { sha256: token('c'), name: 'Music' };
    const database = memoryDatabase({ version: 3, stores: { handles: [], indexes: [], folderIdentities: [
        [oldIdentity.sha256, { handle: handle('broken'), identity: oldIdentity }], [match.sha256, { handle: handle('A'), identity: match }],
    ] } });
    const { folderIdentity } = await identities(database);
    const directory = key => ({ ...handle(key), async isSameEntry(other) { if (other.key === 'broken') throw new Error('handle inaccessible'); return key === other.key; } });
    assert.deepEqual(await folderIdentity(source(directory('A'))), { identity: match, storageFailed: false });
    const unknown = await folderIdentity(source(directory('B')));
    assert.equal(unknown.storageFailed, true);
    assert.notEqual(unknown.identity.sha256, match.sha256);
    assert.equal(database.writes, 0);
});

test('blocked upgrade settles fallback and closes a database that opens later; versionchange closes live connections', async () => {
    const database = memoryDatabase({ blocked: true });
    const { folderIdentity, storedLibraryValue } = await identities(database);
    const result = await folderIdentity(source(handle('A')));
    assert.equal(result.storageFailed, true);
    assert.equal(database.closes, 1);
    database.blocked = false;
    assert.equal(await storedLibraryValue('handles', 'root'), undefined);
    const closes = database.closes;
    database.databases.at(-1).onversionchange();
    assert.equal(database.closes, closes + 1);
});

test('backward-compatible storage API preserves last handle and remote index independently of registry writes', async () => {
    const database = memoryDatabase();
    const { folderIdentity, storedLibraryValue } = await identities(database);
    const root = handle('A');
    const remote = { bytes: 'CSV', name: 'Index' };
    await storedLibraryValue('handles', 'root', root);
    await storedLibraryValue('indexes', 'last', remote);
    await folderIdentity(source(root));
    await storedLibraryValue('handles', 'root', handle('B'));
    assert.equal((await storedLibraryValue('handles', 'root')).key, 'B');
    assert.equal(await storedLibraryValue('indexes', 'last'), remote);
    assert.equal(database.stores.get('folderIdentities').size, 1);
});
