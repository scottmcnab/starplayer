// Blob data and ordered state share a transaction. All mutations are serialised.
export async function openPlaylistStore({ indexedDB, name = 'starplayer-playlist', maxBytes = 256 * 1024 * 1024, report = () => {} } = {}) {
    let database = null;
    let sessionOnly = false;
    let state = { entries: [], currentId: null, loop: false };
    let blobs = new Map();
    let libraries = new Map();
    let queue = Promise.resolve();
    function fallback(error) {
        if (!sessionOnly) report(`Playlist storage unavailable (${error?.message || 'IndexedDB unavailable'}). Continuing session-only.`);
        sessionOnly = true;
        database?.close();
        database = null;
    }
    const requestValue = request => new Promise((resolve, reject) => { request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error); });
    const completed = transaction => new Promise((resolve, reject) => { transaction.oncomplete = resolve; transaction.onabort = transaction.onerror = () => reject(transaction.error || new Error('Storage transaction failed')); });
    try {
        if (indexedDB === undefined) indexedDB = globalThis.indexedDB;
        database = await new Promise((resolve, reject) => {
            if (!indexedDB) { reject(new Error('IndexedDB unavailable')); return; }
            const request = indexedDB.open(name, 2);
            let abandoned = false;
            request.onupgradeneeded = () => {
                for (const store of ['state', 'blobs', 'libraries']) if (!request.result.objectStoreNames.contains(store)) request.result.createObjectStore(store);
            };
            request.onsuccess = () => { if (abandoned) request.result.close(); else resolve(request.result); };
            request.onerror = () => reject(request.error);
            request.onblocked = () => { abandoned = true; reject(new Error('Database open blocked')); };
        });
        database.onversionchange = () => fallback(new Error('Database version changed'));
        const transaction = database.transaction(['state', 'blobs', 'libraries'], 'readonly');
        const done = completed(transaction);
        const [saved, storedBlobs, storedLibraries] = await Promise.all([
            requestValue(transaction.objectStore('state').get('playlist')),
            requestValue(transaction.objectStore('blobs').getAll()),
            requestValue(transaction.objectStore('libraries').getAll()),
            done,
        ]);
        if (saved) state = saved;
        blobs = new Map(storedBlobs.map(blob => [blob.blobId, blob]));
        libraries = new Map(storedLibraries.map(library => [library.sha256, library]));
    } catch (error) { fallback(error); }
    return {
        get sessionOnly() { return sessionOnly; },
        get state() { return structuredClone(state); },
        hasBlob: blobId => blobs.has(blobId),
        async getBlob(blobId) { return blobs.get(blobId) ?? null; },
        save(nextState, { uploads = [], knownLibraries = [] } = {}) {
            const snapshot = structuredClone(nextState);
            const operation = queue.then(async () => {
                const nextBlobs = new Map(blobs);
                for (const blob of uploads) nextBlobs.set(blob.blobId, blob);
                const references = new Set(snapshot.entries.filter(entry => entry.locator.kind === 'upload').map(entry => entry.locator.blobId));
                // Removing the last reference immediately retires a blob. Old orphaned
                // blobs (including those from v1) are also the only eviction candidates.
                for (const blobId of nextBlobs.keys()) if (!references.has(blobId)) nextBlobs.delete(blobId);
                const size = [...nextBlobs.values()].reduce((total, blob) => total + blob.size, 0);
                if (size > maxBytes) throw new Error(`Playlist upload refused: stored files exceed the ${maxBytes / 1024 / 1024} MiB limit. Use Open file to play once.`);
                const nextLibraries = new Map(libraries);
                for (const library of knownLibraries) nextLibraries.set(library.sha256, library);
                if (database) {
                    try {
                        const transaction = database.transaction(['state', 'blobs', 'libraries'], 'readwrite');
                        const done = completed(transaction);
                        try {
                            transaction.objectStore('state').put(snapshot, 'playlist');
                            for (const blobId of blobs.keys()) if (!nextBlobs.has(blobId)) transaction.objectStore('blobs').delete(blobId);
                            for (const blob of uploads) if (references.has(blob.blobId)) transaction.objectStore('blobs').put(blob, blob.blobId);
                            for (const library of knownLibraries) transaction.objectStore('libraries').put(library, library.sha256);
                        } catch (error) {
                            transaction.abort();
                            await done.catch(() => {});
                            throw error;
                        }
                        await done;
                    } catch (error) { fallback(error); }
                }
                state = snapshot;
                blobs = nextBlobs;
                libraries = nextLibraries;
            });
            queue = operation.catch(() => {});
            return operation;
        },
        settled: () => queue,
        close() { database?.close(); },
    };
}
