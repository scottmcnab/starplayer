import { isCommentsCsvHeader } from './library-index.js';

// Library roots expose bytes, never archive-member interpretation.
export function checkAbort(signal) {
    if (signal?.aborted) throw new DOMException('Library request cancelled', 'AbortError');
}

function pathSegments(path) {
    const segments = path.split('/');
    if (/^[a-z]:/i.test(path) || segments.some(segment => !segment || segment === '.' || segment === '..' || /[\\\0]/.test(segment))) throw new Error(`${path}: invalid library path`);
    return segments;
}

export async function discoverIndex(paths, readHead) {
    const candidates = [], commentsCandidates = [];
    for (const path of [...paths].filter(path => !path.includes('/') && /\.csv$/i.test(path)).sort()) {
        try {
            const header = (await readHead(path)).split('\n')[0].replace(/\r$/, '');
            (isCommentsCsvHeader(header) ? commentsCandidates : candidates).push(path);
        } catch { /* An unreadable header cannot take down the library. */ }
    }
    return { indexPath: candidates.find(path => path.toLowerCase() === 'index.csv') ?? (candidates.length === 1 ? candidates[0] : null), candidates,
        commentsPath: commentsCandidates.length === 1 ? commentsCandidates[0] : null, commentsCandidates };
}

async function fileOperation(path, signal, operation) {
    try {
        checkAbort(signal);
        const result = await operation();
        checkAbort(signal);
        return result;
    } catch (error) {
        if (error.name === 'AbortError') throw error;
        throw new Error(`${path}: ${error.name === 'NotFoundError' ? 'file missing' : error.message}`, { cause: error });
    }
}

export async function createFileListSource(files) {
    const entries = new Map();
    let label = 'library folder';
    for (const file of files) {
        const separator = file.webkitRelativePath.indexOf('/');
        if (separator < 0) continue;
        label = file.webkitRelativePath.slice(0, separator);
        entries.set(file.webkitRelativePath.slice(separator + 1), file);
    }
    const getFile = path => {
        pathSegments(path);
        if (!entries.has(path)) throw new Error('file missing');
        return entries.get(path);
    };
    const readHead = (path, bytes = 65536) => fileOperation(path, null, () => getFile(path).slice(0, bytes).text());
    return { kind: 'filelist', label, readHead, ...await discoverIndex(entries.keys(), readHead),
        readFile: (path, { signal } = {}) => fileOperation(path, signal, () => getFile(path).arrayBuffer()),
        size: path => fileOperation(path, null, () => getFile(path).size) };
}

export async function createDirectorySource(handle) {
    const paths = [];
    for await (const [name, entry] of handle.entries()) if (entry.kind === 'file') paths.push(name);
    const getFile = async path => {
        const segments = pathSegments(path);
        let directory = handle;
        for (const segment of segments.slice(0, -1)) directory = await directory.getDirectoryHandle(segment);
        return (await directory.getFileHandle(segments.at(-1))).getFile();
    };
    const readHead = (path, bytes = 65536) => fileOperation(path, null, async () => (await getFile(path)).slice(0, bytes).text());
    return { kind: 'directory', label: handle.name, handle, readHead, ...await discoverIndex(paths, readHead),
        readFile: (path, { signal } = {}) => fileOperation(path, signal, async () => (await getFile(path)).arrayBuffer()),
        size: path => fileOperation(path, null, async () => (await getFile(path)).size) };
}

export function directoryPickerAvailable() {
    return typeof window !== 'undefined' && 'showDirectoryPicker' in window && globalThis.isSecureContext;
}

// Called by the click handler: the picker runs before any other asynchronous work.
export async function openDirectorySource() {
    const handle = await window.showDirectoryPicker({ mode: 'read' });
    return createDirectorySource(handle);
}

export async function reconnectDirectorySource(handle) {
    const permission = await handle.requestPermission({ mode: 'read' });
    if (permission !== 'granted') throw new Error(`${handle.name}: library read permission denied`);
    return createDirectorySource(handle);
}

async function storedLibraryValue(storeName, key, value) {
    const database = await new Promise((resolve, reject) => {
        const request = indexedDB.open('starplayer-library', 2);
        request.onupgradeneeded = () => {
            for (const name of ['handles', 'indexes']) if (!request.result.objectStoreNames.contains(name)) request.result.createObjectStore(name);
        };
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
    });
    try {
        return await new Promise((resolve, reject) => {
            const transaction = database.transaction(storeName, value === undefined ? 'readonly' : 'readwrite');
            const store = transaction.objectStore(storeName);
            const request = value === undefined ? store.get(key) : store.put(value, key);
            transaction.oncomplete = () => resolve(request.result);
            transaction.onerror = () => reject(transaction.error);
            transaction.onabort = () => reject(transaction.error ?? new Error('Handle storage cancelled'));
        });
    } finally { database.close(); }
}

export const storedDirectoryHandle = handle => storedLibraryValue('handles', 'root', handle);
export const storedRemoteIndex = record => storedLibraryValue('indexes', 'last', record);

export function createHttpSource({ base = '/library/', kind = 'http', label = 'server library', indexPath = '/library-index.csv', fetch: fetchFile = globalThis.fetch } = {}) {
    const request = async (path, method, signal) => {
        const url = base + pathSegments(path).map(encodeURIComponent).join('/');
        const response = await fetchFile(url, { method, signal });
        checkAbort(signal);
        if (!response.ok) throw new Error(`${path}: ${response.status === 404 ? 'file missing' : `HTTP ${response.status}`}`);
        return response;
    };
    return { kind, label, indexPath,
        readFile: (path, { signal } = {}) => fileOperation(path, signal, async () => (await request(path, 'GET', signal)).arrayBuffer()),
        size: path => fileOperation(path, null, async () => {
            const value = (await request(path, 'HEAD')).headers.get('Content-Length');
            return value !== null && /^\d+$/.test(value) ? Number(value) : null;
        }) };
}

export function normalizeProxy(value) {
    if (!value.trim()) return '';
    let url;
    try {
        if (!/^https?:\/\//i.test(value.trim())) throw new Error('Not absolute');
        url = new URL(value.trim());
    } catch { throw new Error('Proxy URL must be an absolute http(s) URL'); }
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error('Proxy URL must be an absolute http(s) URL without credentials, query or fragment');
    return url.href.endsWith('/') ? url.href : url.href + '/';
}

export function createRemoteSource({ proxy = '', fetch: fetchFile = globalThis.fetch } = {}) {
    let base = '', matching = new Set(), http = null;
    let proxyUrl = normalizeProxy(proxy);
    const source = { kind: 'remote', indexPath: null,
        get base() { return base; },
        get proxy() { return proxyUrl; },
        get label() { return base ? new URL(base).host + (proxyUrl ? ` via ${new URL(proxyUrl).host}` : '') : 'source URLs'; },
        setProxy(value) { proxyUrl = normalizeProxy(value); rebuild(); },
        attach(library) {
            http = null;
            const counts = new Map();
            for (const row of library.rows) {
                if (!row.sourceUrl) continue;
                const candidates = new Set();
                for (const suffix of [row.sourcePath, pathSegments(row.sourcePath).map(encodeURIComponent).join('/')]) {
                    if (!row.sourceUrl.endsWith(suffix)) continue;
                    const prefix = row.sourceUrl.slice(0, -suffix.length);
                    try {
                        const url = new URL(prefix);
                        if (/^https?:$/.test(url.protocol) && prefix.endsWith('/') && !url.username && !url.password && !url.search && !url.hash) candidates.add(prefix);
                    } catch { /* Not an absolute usable base. */ }
                }
                for (const candidate of candidates) counts.set(candidate, (counts.get(candidate) || 0) + 1);
            }
            base = [...counts].sort((left, right) => right[1] - left[1])[0]?.[0] || '';
            if (!base) throw new Error('No usable source_url values or consistent base in this index');
            const matchedRows = library.rows.filter(row => row.sourceUrl === base + row.sourcePath || row.sourceUrl === base + pathSegments(row.sourcePath).map(encodeURIComponent).join('/'));
            matching = new Set(matchedRows.map(row => row.sourcePath));
            const streamable = matchedRows.length;
            rebuild();
            return { base, streamable, unmatched: library.rows.length - streamable };
        },
        size() { return null; },
        async readFile(path, options) {
            if (!http) throw new Error('Remote source must attach a library before reading');
            if (!matching.has(path)) throw new Error(`${path}: source_url does not follow ${base}`);
            return http.readFile(path, options);
        },
    };
    function rebuild() {
        if (!base) return;
        const target = proxyUrl || base;
        http = createHttpSource({ base: target, kind: 'remote', indexPath: null, fetch: async (...args) => {
            try { return await fetchFile(args[0], { ...args[1], credentials: 'omit' }); }
            catch (error) {
                if (error instanceof TypeError) throw new Error(`could not fetch from ${new URL(target).host}; it may not allow cross-origin requests — set a Proxy URL`);
                throw error;
            }
        } });
    }
    return source;
}
