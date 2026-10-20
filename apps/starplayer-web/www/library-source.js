// Library roots expose bytes, never archive-member interpretation.
export function checkAbort(signal) {
    if (signal?.aborted) throw new DOMException('Library request cancelled', 'AbortError');
}

function pathSegments(path) {
    const segments = path.split('/');
    if (/^[a-z]:/i.test(path) || segments.some(segment => !segment || segment === '.' || segment === '..' || /[\\\0]/.test(segment))) throw new Error(`${path}: invalid library path`);
    return segments;
}

export function discoverIndex(paths) {
    const candidates = [...paths].filter(path => !path.includes('/') && /\.csv$/i.test(path)).sort();
    return { indexPath: candidates.find(path => path.toLowerCase() === 'index.csv') ?? (candidates.length === 1 ? candidates[0] : null), candidates };
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

export function createFileListSource(files) {
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
    return { kind: 'filelist', label, ...discoverIndex(entries.keys()),
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
    return { kind: 'directory', label: handle.name, handle, ...discoverIndex(paths),
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

export async function storedDirectoryHandle(handle) {
    const database = await new Promise((resolve, reject) => {
        const request = indexedDB.open('starplayer-library', 1);
        request.onupgradeneeded = () => request.result.createObjectStore('handles');
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error);
    });
    try {
        return await new Promise((resolve, reject) => {
            const transaction = database.transaction('handles', handle ? 'readwrite' : 'readonly');
            const store = transaction.objectStore('handles');
            const request = handle ? store.put(handle, 'root') : store.get('root');
            transaction.oncomplete = () => resolve(request.result);
            transaction.onerror = () => reject(transaction.error);
            transaction.onabort = () => reject(transaction.error ?? new Error('Handle storage cancelled'));
        });
    } finally { database.close(); }
}

export function createHttpSource({ fetch: fetchFile = globalThis.fetch } = {}) {
    const request = async (path, method, signal) => {
        const url = '/library/' + pathSegments(path).map(encodeURIComponent).join('/');
        const response = await fetchFile(url, { method, signal });
        checkAbort(signal);
        if (!response.ok) throw new Error(`${path}: ${response.status === 404 ? 'file missing' : `HTTP ${response.status}`}`);
        return response;
    };
    return { kind: 'http', label: 'server library', indexPath: '/library-index.csv',
        readFile: (path, { signal } = {}) => fileOperation(path, signal, async () => (await request(path, 'GET', signal)).arrayBuffer()),
        size: path => fileOperation(path, null, async () => {
            const value = (await request(path, 'HEAD')).headers.get('Content-Length');
            return value !== null && /^\d+$/.test(value) ? Number(value) : null;
        }) };
}
