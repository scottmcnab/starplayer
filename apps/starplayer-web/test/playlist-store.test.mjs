// Run against real IndexedDB in headless.mjs; no approximation of transaction timing.
export async function verifyPlaylistStore(openPlaylistStore) {
    const check = (condition, message) => { if (!condition) throw new Error(message); };
    const name = `playlist-test-${crypto.randomUUID()}`;
    const entry = { id: 'a', locator: { kind: 'upload', blobId: 'blob' }, display: { name: 'test' } };
    const snapshot = { entries: [entry, { ...entry, id: 'b' }], currentId: 'b', loop: true };
    const blob = { blobId: 'blob', name: 'test', size: 4, type: '', bytes: new Blob(['test']), addedAt: 1 };
    let store = await openPlaylistStore({ name, maxBytes: 4 });
    await store.save(snapshot, { uploads: [blob] });
    store.close();
    store = await openPlaylistStore({ name, maxBytes: 4 });
    check(JSON.stringify(store.state) === JSON.stringify(snapshot), 'state/current id round trip');
    check(await (await store.getBlob('blob')).bytes.text() === 'test', 'blob bytes round trip');
    await store.save({ ...snapshot, entries: [entry], currentId: 'a' });
    check(store.hasBlob('blob'), 'shared reference retained');
    let refused = false;
    try { await store.save({ ...snapshot, entries: [...snapshot.entries, { ...entry, id: 'c', locator: { kind: 'upload', blobId: 'other' } }] }, { uploads: [{ ...blob, blobId: 'other' }] }); } catch { refused = true; }
    check(refused && !store.hasBlob('other') && store.hasBlob('blob'), 'cap refusal is atomic');
    await store.save({ entries: [], currentId: null, loop: false });
    check(!store.hasBlob('blob'), 'last reference deleted');
    await store.save(snapshot); // Browser eviction: state can survive missing bytes.
    check(!store.hasBlob('blob'), 'missing blob state');
    store.close();
    // Seed a version-one database with an orphan. Upgrade must retain state,
    // create missing stores, and evict the orphan before accepting a capped upload.
    const upgradeName = `${name}-v1`;
    const database = await new Promise((resolve, reject) => {
        const request = indexedDB.open(upgradeName, 1);
        request.onupgradeneeded = () => { request.result.createObjectStore('state'); request.result.createObjectStore('blobs'); };
        request.onerror = () => reject(request.error);
        request.onsuccess = () => resolve(request.result);
    });
    await new Promise((resolve, reject) => {
        const transaction = database.transaction(['state', 'blobs'], 'readwrite');
        transaction.objectStore('state').put(snapshot, 'playlist');
        transaction.objectStore('blobs').put({ ...blob, blobId: 'orphan' }, 'orphan');
        transaction.oncomplete = resolve; transaction.onabort = () => reject(transaction.error);
    });
    database.close();
    store = await openPlaylistStore({ name: upgradeName, maxBytes: 4 });
    check(store.state.currentId === 'b', 'v1 state preserved');
    await store.save(snapshot, { uploads: [blob] });
    check(!store.hasBlob('orphan') && store.hasBlob('blob'), 'only unreferenced blobs evicted');
    store.close();
    const messages = [];
    const unavailable = await openPlaylistStore({ indexedDB: null, report: message => messages.push(message) });
    await unavailable.save(snapshot, { uploads: [blob] });
    await unavailable.save(snapshot);
    check(unavailable.sessionOnly && messages.length === 1 && unavailable.hasBlob('blob'), 'unavailable session fallback once');
    const failed = await openPlaylistStore({ indexedDB: { open() { throw new Error('failed open'); } } });
    check(failed.sessionOnly, 'failed open fallback');
    // Keep the real browser database, but inject a quota failure at write creation.
    const realOpen = indexedDB.open.bind(indexedDB);
    const quotaDB = { open(...args) {
        const request = realOpen(...args);
        request.addEventListener('success', () => {
            const database = request.result;
            const transaction = database.transaction.bind(database);
            database.transaction = (...args) => { if (args[1] === 'readwrite') throw new DOMException('Quota exhausted', 'QuotaExceededError'); return transaction(...args); };
        });
        return request;
    } };
    const quota = await openPlaylistStore({ indexedDB: quotaDB, name: `${name}-quota`, report: message => messages.push(message) });
    await quota.save(snapshot, { uploads: [blob] });
    check(quota.sessionOnly && quota.hasBlob('blob') && messages.length === 2, 'quota session fallback');
    // A synchronous put failure after the state put must abort the whole transaction.
    const partialName = `${name}-partial`;
    const partialDB = { open(...args) {
        const request = realOpen(...args);
        request.addEventListener('success', () => {
            const database = request.result;
            const transaction = database.transaction.bind(database);
            database.transaction = (...args) => {
                const result = transaction(...args);
                if (args[1] === 'readwrite') {
                    const objectStore = result.objectStore.bind(result);
                    result.objectStore = name => {
                        const store = objectStore(name);
                        if (name === 'blobs') store.put = () => { throw new DOMException('Cannot clone blob', 'DataCloneError'); };
                        return store;
                    };
                }
                return result;
            };
        });
        return request;
    } };
    const partial = await openPlaylistStore({ indexedDB: partialDB, name: partialName });
    await partial.save(snapshot, { uploads: [blob] });
    check(partial.sessionOnly && partial.hasBlob('blob'), 'synchronous write failure retains session data');
    const reopened = await openPlaylistStore({ name: partialName });
    check(reopened.state.entries.length === 0 && !reopened.hasBlob('blob'), 'synchronous write failure rolls back persistent state');
    // Uploads absent from the new state must not leave persistent orphan bytes.
    await reopened.save({ entries: [], currentId: null, loop: false }, { uploads: [blob] });
    reopened.close();
    const withoutOrphans = await openPlaylistStore({ name: partialName });
    check(!withoutOrphans.hasBlob('blob'), 'unreferenced upload is not persisted');
    withoutOrphans.close();
    for (const databaseName of [name, upgradeName, `${name}-quota`, partialName]) indexedDB.deleteDatabase(databaseName);
    return 'round trip, refcounts, cap eviction/refusal, missing blobs, v1 upgrade, unavailable/open/quota fallback passed';
}
