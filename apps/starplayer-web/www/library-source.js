import { isCommentsCsvHeader, parseCsv } from './library-index.js';
import { storedLibraryValue } from './library-identity.js';
import { yieldCooperatively } from './cooperative-yield.js';
export { folderIdentity } from './library-identity.js';

// Library roots expose bytes, never archive-member interpretation.
export function checkAbort(signal) {
    if (signal?.aborted) throw new DOMException('Library request cancelled', 'AbortError');
}

function pathSegments(path) {
    const segments = path.split('/');
    if (/^[a-z]:/i.test(path) || segments.some(segment => !segment || segment === '.' || segment === '..' || /[\\\0]/.test(segment))) throw new Error(`${path}: invalid library path`);
    return segments;
}

const SOURCE_BATCH_SIZE = 128;
const SOURCE_SLICE_MS = 8;
const sourceClock = () => globalThis.performance?.now() ?? Date.now();

function sourceSlice({ signal, yieldTask = yieldCooperatively, now = sourceClock } = {}) {
    let records = 0, started = now();
    return {
        due() { checkAbort(signal); return ++records >= SOURCE_BATCH_SIZE || now() - started >= SOURCE_SLICE_MS; },
        async yield() { await yieldTask({ signal }); checkAbort(signal); records = 0; started = now(); },
    };
}

// Keep deterministic candidate order without an unbounded native sort on a large root.
async function sortPaths(paths, slice) {
    let sorted = paths, buffer = new Array(paths.length);
    for (let width = 1; width < paths.length; width *= 2) {
        for (let start = 0; start < paths.length; start += width * 2) {
            const middle = Math.min(start + width, paths.length), end = Math.min(start + width * 2, paths.length);
            let left = start, right = middle;
            for (let index = start; index < end; index += 1) {
                buffer[index] = left < middle && (right >= end || sorted[left] <= sorted[right]) ? sorted[left++] : sorted[right++];
                if (slice.due()) await slice.yield();
            }
        }
        [sorted, buffer] = [buffer, sorted];
    }
    return sorted;
}

// Read only the first logical CSV record: malformed bodies still reach the index error UI.
function csvHeader(text) {
    let quoted = false;
    for (let index = 0; index < text.length; index += 1) {
        if (text[index] === '"') {
            if (quoted && text[index + 1] === '"') index += 1;
            else quoted = !quoted;
        } else if (!quoted && /[\r\n]/.test(text[index])) return text.slice(0, index);
    }
    return text;
}

export function discoverIndex(paths, readHead, options = {}) {
    return discoverSourceIndex(paths, readHead, options, sourceSlice(options));
}

async function discoverSourceIndex(paths, readHead, options, slice) {
    const { signal } = options;
    checkAbort(signal);
    const candidates = [], commentsCandidates = [], rootPaths = [];
    let preferredIndex = null;
    for (const path of paths) {
        checkAbort(signal);
        if (!path.includes('/') && /\.csv$/i.test(path)) rootPaths.push(path);
        if (slice.due()) await slice.yield();
    }
    for (const path of await sortPaths(rootPaths, slice)) {
        try {
            pathSegments(path);
            checkAbort(signal);
            const head = await readHead(path);
            checkAbort(signal);
            const header = csvHeader(head);
            const columns = (parseCsv(header)[0] || []).map(column => column.trim().toLowerCase());
            if (isCommentsCsvHeader(header)) commentsCandidates.push(path);
            else if (columns.includes('source_path') && !columns.includes('library')) {
                candidates.push(path);
                if (preferredIndex === null && path.toLowerCase() === 'index.csv') preferredIndex = path;
            }
        } catch (error) {
            if (error.name === 'AbortError') throw error;
            /* An unreadable or invalid header cannot take down the library. */
        }
        if (slice.due()) await slice.yield();
    }
    return { indexPath: preferredIndex ?? (candidates.length === 1 ? candidates[0] : null), candidates,
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

function issue(path, error) {
    return { kind: 'issue', path, reason: error.name === 'NotFoundError' ? 'file missing' : error.message || String(error) };
}

function timestamp(lastModified) {
    if (!Number.isFinite(lastModified)) return '';
    const date = new Date(lastModified);
    return Number.isNaN(date.getTime()) ? '' : date.toISOString();
}

// Local enumerate({signal}) yields all files or path issues, reads no contents, and throws
// AbortError on cancellation. Keep its iterator to pause/resume without restarting.
export async function createFileListSource(files, options = {}) {
    const { signal } = options;
    checkAbort(signal);
    const entries = new Map(), issues = [], slice = sourceSlice(options);
    let label = 'library folder', root = null;
    // The browser finishes its picker before delivering FileList. Application mapping yields separately.
    for (const file of files) {
        checkAbort(signal);
        let path = '';
        try {
            const relativePath = file.webkitRelativePath;
            path = relativePath || file.name || '';
            const segments = pathSegments(relativePath);
            if (segments.length < 2) throw new Error('invalid library path: missing selected folder');
            if (root !== null && root !== segments[0]) throw new Error('file is outside the selected folder');
            root = segments[0];
            label = root;
            path = segments.slice(1).join('/');
            pathSegments(path);
            if (entries.has(path)) throw new Error('duplicate library path');
            entries.set(path, file);
        } catch (error) { issues.push(issue(path, error)); }
        if (slice.due()) await slice.yield();
    }
    const getFile = path => {
        pathSegments(path);
        if (!entries.has(path)) throw new Error('file missing');
        return entries.get(path);
    };
    const readHead = (path, bytes = 65536) => fileOperation(path, null, () => getFile(path).slice(0, bytes).text());
    return { kind: 'filelist', label, readHead, ...await discoverSourceIndex(entries.keys(), readHead, options, slice),
        async *enumerate({ signal } = {}) {
            checkAbort(signal);
            const slice = sourceSlice({ ...options, signal });
            for (const entryIssue of issues) {
                checkAbort(signal);
                yield entryIssue;
                if (slice.due()) await slice.yield();
            }
            for (const [path, file] of entries) {
                checkAbort(signal);
                let record;
                try {
                    const size = file.size;
                    record = { kind: 'file', path, size: Number.isFinite(size) && size >= 0 ? size : null, timestamp: timestamp(file.lastModified) };
                }
                catch (error) { record = issue(path, error); }
                yield record;
                if (slice.due()) await slice.yield();
            }
        },
        readFile: (path, { signal } = {}) => fileOperation(path, signal, () => getFile(path).arrayBuffer()),
        size: path => fileOperation(path, null, () => getFile(path).size) };
}

export async function createDirectorySource(handle, options = {}) {
    const { signal } = options;
    checkAbort(signal);
    const paths = [], slice = sourceSlice(options);
    try {
        for await (const [name, entry] of handle.entries()) {
            checkAbort(signal);
            try { pathSegments(name); if (name.includes('/')) throw new Error('invalid path segment'); if (entry.kind === 'file') paths.push(name); }
            catch { /* enumerate reports invalid root entries; candidate discovery remains best effort. */ }
            if (slice.due()) await slice.yield();
        }
    } catch (error) {
        if (error.name === 'AbortError') throw error;
        /* Keep permission-denied roots usable for the discovery retry UI. */
    }
    checkAbort(signal);
    const getFile = async (path, signal) => {
        const segments = pathSegments(path);
        let directory = handle;
        for (const segment of segments.slice(0, -1)) {
            checkAbort(signal);
            directory = await directory.getDirectoryHandle(segment);
            checkAbort(signal);
        }
        checkAbort(signal);
        const fileHandle = await directory.getFileHandle(segments.at(-1));
        checkAbort(signal);
        const file = await fileHandle.getFile();
        checkAbort(signal);
        return file;
    };
    const readHead = (path, bytes = 65536) => fileOperation(path, null, async () => (await getFile(path)).slice(0, bytes).text());
    return { kind: 'directory', label: handle.name, handle, readHead, ...await discoverSourceIndex(paths, readHead, options, slice),
        async *enumerate({ signal } = {}) {
            const stack = [{ directory: handle, path: '', iterator: null }];
            const slice = sourceSlice({ ...options, signal });
            try {
                while (stack.length) {
                    checkAbort(signal);
                    const current = stack.at(-1);
                    let record;
                    try {
                        current.iterator ??= current.directory.entries()[Symbol.asyncIterator]();
                        checkAbort(signal);
                        const next = await current.iterator.next();
                        checkAbort(signal);
                        if (next.done) stack.pop();
                        else {
                            const [name, entry] = next.value;
                            const path = current.path ? `${current.path}/${name}` : name;
                            try {
                                pathSegments(path);
                                if (name.includes('/')) throw new Error('invalid library path segment');
                                if (entry.kind === 'file') record = { kind: 'file', path, size: null, timestamp: '' };
                                else if (entry.kind === 'directory') {
                                    checkAbort(signal);
                                    const directory = typeof entry.entries === 'function' ? entry : await current.directory.getDirectoryHandle(name);
                                    checkAbort(signal);
                                    stack.push({ directory, path, iterator: null });
                                }
                            } catch (error) {
                                if (error.name === 'AbortError') throw error;
                                record = issue(path, error);
                            }
                        }
                    } catch (error) {
                        if (error.name === 'AbortError') throw error;
                        stack.pop();
                        // An empty path denotes failure of the root walk: the controller can offer Retry.
                        record = issue(current.path, error);
                    }
                    if (record) yield record;
                    if (slice.due()) await slice.yield();
                }
            } finally {
                for (const current of stack.reverse()) {
                    try { await current.iterator?.return?.(); } catch { /* Cleanup must preserve the original error. */ }
                }
            }
        },
        readFile: (path, { signal } = {}) => fileOperation(path, signal, async () => {
            const file = await getFile(path, signal);
            checkAbort(signal);
            return file.arrayBuffer();
        }),
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
