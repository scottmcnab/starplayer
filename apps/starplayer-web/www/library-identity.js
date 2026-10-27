// Local-folder identities are opaque tokens, independent of contents and enumeration order.
// Keep the existing last-reconnect handle and remote index slots when upgrading v2.
function openLibraryDatabase() {
    return new Promise((resolve, reject) => {
        let abandoned = false;
        const request = indexedDB.open('starplayer-library', 3);
        request.onupgradeneeded = () => {
            for (const name of ['handles', 'indexes', 'folderIdentities']) {
                if (!request.result.objectStoreNames.contains(name)) request.result.createObjectStore(name);
            }
        };
        request.onblocked = () => {
            abandoned = true;
            reject(new Error('Library storage upgrade blocked by another open page'));
        };
        request.onsuccess = () => {
            const database = request.result;
            database.onversionchange = () => database.close();
            if (abandoned) database.close();
            else resolve(database);
        };
        request.onerror = () => reject(request.error ?? new Error('Library storage unavailable'));
    });
}

// Shared storage backend; omitted value reads, supplied value writes. Existing callers retain their API.
export async function storedLibraryValue(storeName, key, value) {
    const database = await openLibraryDatabase();
    try {
        return await new Promise((resolve, reject) => {
            const transaction = database.transaction(storeName, value === undefined ? 'readonly' : 'readwrite');
            const store = transaction.objectStore(storeName);
            const request = value === undefined ? (key === undefined ? store.getAll() : store.get(key)) : store.put(value, key);
            transaction.oncomplete = () => resolve(request.result);
            transaction.onerror = () => reject(transaction.error ?? request.error ?? new Error('Library storage failed'));
            transaction.onabort = () => reject(transaction.error ?? new Error('Library storage cancelled'));
        });
    } finally { database.close(); }
}

const sourceIdentities = new WeakMap();
const sessionRecords = [];
let pendingIdentity = Promise.resolve();

function newIdentity(source) {
    const bytes = new Uint8Array(32);
    globalThis.crypto.getRandomValues(bytes);
    return { sha256: Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join(''), name: source.label };
}

function validIdentity(identity) {
    return identity && /^[a-f0-9]{64}$/.test(identity.sha256) && typeof identity.name === 'string';
}

async function lookupIdentity(source) {
    const cached = sourceIdentities.get(source);
    if (cached) return cached;
    let storageFailed = false, records = [];
    if (source.kind === 'directory' && source.handle) {
        try { records = await storedLibraryValue('folderIdentities'); }
        catch { storageFailed = true; }
        // Session records preserve same-entry reuse even when IndexedDB is unavailable.
        records = [...sessionRecords, ...records];
        let comparisonFailed = false;
        for (const record of records) {
            if (!record?.handle || !validIdentity(record.identity)) continue;
            try {
                if (source.handle === record.handle || await source.handle.isSameEntry(record.handle)) {
                    const result = { identity: record.identity, storageFailed: storageFailed || !!record.storageFailed };
                    sourceIdentities.set(source, result);
                    if (!sessionRecords.some(session => session.identity.sha256 === record.identity.sha256)) sessionRecords.push({ handle: source.handle, ...result });
                    return result;
                }
            } catch { comparisonFailed = true; }
        }
        storageFailed ||= comparisonFailed;
    } else storageFailed = true;
    const identity = newIdentity(source);
    if (!storageFailed) {
        try { await storedLibraryValue('folderIdentities', identity.sha256, { handle: source.handle, identity }); }
        catch { storageFailed = true; }
    }
    const result = { identity, storageFailed };
    sourceIdentities.set(source, result);
    if (source.handle) sessionRecords.push({ handle: source.handle, identity, storageFailed });
    return result;
}

/**
 * Resolve {identity:{sha256,name},storageFailed}. sha256 is a random 64-hex folder token,
 * not a digest. FileList selections and failed persistence use a source-stable session
 * token. Lookups are serialized in this page; registry entries are never evicted.
 * The legacy handles/root slot remains separate and gains a registry identity only
 * when its reconnected source first enters discovered-library mode.
 */
export function folderIdentity(source) {
    const result = pendingIdentity.then(() => lookupIdentity(source));
    pendingIdentity = result.catch(() => {});
    return result;
}
