import { appendDiscoveredRows, updateDiscoveredRows, attachComments, locatorKey } from './library-index.js';
import { createLibraryWorkerRuntime } from './library-worker-runtime.js';

let nextGeneration = 1;
const supported = /\.(mod|s3m|mtm|xm|it)$/i;
const controlFlow = error => error?.code === 'aborted' || error?.code === 'disposed' || error?.name === 'AbortError';
const joinedPath = locator => [locator.sourcePath, locator.containerMember, locator.memberPath].filter(Boolean).join('/');
const metadataPatch = metadata => ({ format: metadata.format, title: metadata.title, channels: metadata.channels,
    instrumentNames: metadata.instrumentNames, sampleNames: metadata.sampleNames, songMessage: metadata.songMessage,
    metadataState: 'ready', error: '' });

// All mutable lifecycle state belongs to one accepted context. Runtime epochs distinguish
// requeued attempts; locator versions keep older inspections from replacing playback.
export function createDiscoveryController({ context, generation = nextGeneration++, onUpdate = () => {},
    runtimeFactory = createLibraryWorkerRuntime, schedule = setTimeout, unschedule = clearTimeout }) {
    const { source, library } = context;
    let runtime = runtimeFactory({ source });
    let runtimeEpoch = 1;
    let disposed = false, started = false, paused = false, walkDone = false, rootFailed = false;
    let walkController = null, iterator = null, walkTask = null, wakeWalk = null;
    let runtimeError = '';
    let scanActive = false, metadataPhase = 'idle';
    let timer = null;
    const catalogs = new Map(), inspected = new Map(), hashes = new Map();
    const examined = new Set(), cataloged = new Set(), cache = new Map(), versions = new Map();
    const issues = [], issueKeys = new Set();
    const changedRows = new Set(), metadataUpdates = new Map();
    const counts = { ready: 0, unknown: 0, error: 0 };
    let missingHashes = 0;
    let lastPhase = 'running', lastMetadataPhase = 'idle';

    function rowFor(locator) { return library.rows[library.locatorIds.get(locatorKey(locator))]; }
    function countRow(row, direction) {
        counts[row.metadataState] += direction;
        if (!row.sha256) missingHashes += direction;
    }
    function status() {
        const phase = paused ? 'paused' : !walkDone ? 'running' : rootFailed || (runtimeError && catalogs.size) ? 'failed' : catalogs.size ? 'running' : 'complete';
        if (scanActive || inspected.size) {
            if (runtimeError) metadataPhase = 'suspended';
            else if (inspected.size || hashes.size) metadataPhase = 'running';
            else if (phase !== 'complete') metadataPhase = 'waiting';
            else { scanActive = false; metadataPhase = 'complete'; }
        }
        let issueSnapshot;
        const issueLength = issues.length;
        if (!scanActive && metadataPhase === 'running' && !inspected.size) metadataPhase = 'complete';
        return Object.freeze({ phase, filesExamined: examined.size, archivesPending: catalogs.size, tracksFound: library.rows.length,
            get issues() { return issueSnapshot ||= Object.freeze(issues.slice(0, issueLength)); }, issueCount: issues.length, metadataPhase, runtimeError,
            metadataCounts: Object.freeze({ ...counts, hashesPending: hashes.size }), metadataIncomplete: counts.unknown + counts.error > 0 || missingHashes > 0,
            sidecarCounts: Object.freeze({ ...library.commentCounts }) });
    }
    function flush() {
        if (disposed) return;
        if (timer !== null) { unschedule(timer); timer = null; }
        const change = { changedRowIds: [...changedRows], statusChanged: true, metadataUpdates: [...metadataUpdates.values()] };
        changedRows.clear(); metadataUpdates.clear();
        onUpdate(context, change);
    }
    function changed(ids = [], updates = [], immediate = false) {
        if (disposed) return;
        for (const id of ids) changedRows.add(id);
        for (const update of updates) {
            const key = locatorKey(update.locator), previous = metadataUpdates.get(key);
            metadataUpdates.set(key, { locator: { ...update.locator }, patch: { ...previous?.patch, ...update.patch } });
        }
        context.discoveryStatus = status();
        const completed = context.discoveryStatus.phase === 'complete' && lastPhase !== 'complete'
            || context.discoveryStatus.metadataPhase === 'complete' && lastMetadataPhase !== 'complete';
        lastPhase = context.discoveryStatus.phase; lastMetadataPhase = context.discoveryStatus.metadataPhase;
        if (immediate || completed) flush();
        else if (timer === null) timer = schedule(flush, 100);
    }
    function addIssue(path, reason) {
        const key = JSON.stringify([path, reason]);
        if (!issueKeys.has(key)) { issueKeys.add(key); issues.push(Object.freeze({ path, reason })); }
    }
    function patchRow(locator, patch) {
        const key = locatorKey(locator);
        cache.set(key, { ...cache.get(key), ...patch });
        const row = rowFor(locator);
        if (row) countRow(row, -1);
        const ids = updateDiscoveredRows(library, [{ locator, patch }]);
        if (row) countRow(row, 1);
        changed(ids, [{ locator, patch }]);
    }
    function append(records) {
        const ids = appendDiscoveredRows(library, records.map(record => ({ ...record, ...cache.get(locatorKey(record)), ...(record.playable === false ? { error: record.error } : {}) })));
        for (const id of ids) countRow(library.rows[id], 1);
        for (const id of ids) queueInspection(library.rows[id]);
        changed(ids);
    }
    function failRuntime(error) {
        runtimeError = error.message || String(error);
        for (const item of hashes.values()) item.job.cancel();
        hashes.clear();
        changed();
    }
    function enqueueCatalog(item) {
        if (disposed || runtimeError || item.job) return;
        const epoch = runtimeEpoch;
        const job = runtime.enqueue({ generation, kind: 'catalog', sourcePath: item.path });
        item.job = job;
        const current = () => !disposed && epoch === runtimeEpoch && item.job === job;
        job.result.then(result => {
            if (!current() || result.generation !== generation) return;
            catalogs.delete(item.path); cataloged.add(item.path);
            for (const issue of result.payload.issues) addIssue([item.path, issue.path].filter(Boolean).join('/'), issue.reason);
            append(result.payload.entries.map(entry => ({ ...entry, sourcePath: item.path, timestamp: '' })));
            changed();
        }, error => {
            if (!current()) return;
            item.job = null;
            if (error.code === 'worker-failed') { failRuntime(error); return; }
            catalogs.delete(item.path);
            if (!controlFlow(error)) { cataloged.add(item.path); addIssue(item.path, error.message || String(error)); }
            changed();
        });
    }
    function queueInspection(row, force = false) {
        if (disposed || !row?.playable || (!scanActive && !force) || runtimeError) return;
        const key = locatorKey(row);
        if (inspected.has(key) || hashes.has(key) || !force && (row.metadataState === 'error' || row.metadataState === 'ready' && row.sha256)) return;
        const item = { locator: { sourcePath: row.sourcePath, containerMember: row.containerMember, memberPath: row.memberPath, rawHex: row.rawHex }, version: versions.get(key) || 0, job: null };
        inspected.set(key, item);
        enqueueInspection(key, item);
    }
    function enqueueInspection(key, item) {
        if (disposed || runtimeError || item.job) return;
        const epoch = runtimeEpoch;
        const job = runtime.enqueue({ generation, kind: 'inspect', locator: item.locator });
        item.job = job;
        const current = () => !disposed && epoch === runtimeEpoch && inspected.get(key) === item && item.job === job && item.version === (versions.get(key) || 0);
        job.result.then(result => {
            if (!current() || result.generation !== generation) return;
            inspected.delete(key);
            patchRow(item.locator, { ...metadataPatch(result.payload.metadata), sha256: result.payload.sha256 });
        }, error => {
            if (!current()) return;
            item.job = null;
            if (error.code === 'worker-failed') { failRuntime(error); return; }
            inspected.delete(key);
            if (!controlFlow(error)) patchRow(item.locator, { metadataState: 'error', error: error.message || String(error) });
            else changed();
        });
    }
    async function waitForResume() {
        while (paused && !disposed) await new Promise(resolve => { wakeWalk = resolve; });
    }
    function startWalk() {
        if (disposed || walkTask) return;
        walkDone = false; rootFailed = false;
        walkController = new AbortController();
        const currentController = walkController;
        walkTask = (async () => {
            await Promise.resolve();
            try {
                iterator = source.enumerate({ signal: currentController.signal })[Symbol.asyncIterator]();
                while (!disposed && currentController === walkController) {
                    await waitForResume();
                    if (disposed) return;
                    const next = await iterator.next();
                    // A next() already outstanding when Stop was clicked may settle, but
                    // its record and any subsequent next() stay backpressured until Resume.
                    await waitForResume();
                    if (disposed || currentController !== walkController) return;
                    if (next.done) break;
                    const record = next.value;
                    if (record.kind === 'issue') {
                        addIssue(record.path, record.reason);
                        if (!record.path) rootFailed = true;
                    } else if (record.kind === 'file') {
                        if (!examined.has(record.path)) {
                            examined.add(record.path);
                            const format = record.path.match(supported)?.[1].toLowerCase();
                            if (format) append([{ sourcePath: record.path, containerMember: '', memberPath: '', rawHex: '', format,
                                size: record.size, timestamp: record.timestamp, playable: true, error: '' }]);
                        }
                        if (/\.zip$/i.test(record.path) && !catalogs.has(record.path) && !cataloged.has(record.path)) {
                            const item = { path: record.path, job: null };
                            catalogs.set(record.path, item); enqueueCatalog(item);
                        }
                    }
                    changed();
                }
            } catch (error) {
                if (!disposed && currentController === walkController && !controlFlow(error)) { rootFailed = true; addIssue('', error.message || String(error)); }
            } finally {
                if (!disposed && currentController === walkController) { walkDone = true; walkTask = null; changed(); }
            }
        })();
    }
    const controller = {
        start() { if (!disposed && !started) { started = true; startWalk(); changed(); } },
        pause() { if (disposed || paused) return; paused = true; runtime.pauseCatalog(); changed(); },
        resume() {
            if (disposed) return;
            paused = false; wakeWalk?.(); wakeWalk = null;
            if (!runtimeError) runtime.resumeCatalog();
            changed();
        },
        retry() {
            if (disposed) return;
            if (runtimeError) {
                runtimeEpoch += 1;
                runtime.dispose();
                hashes.clear(); // Owned bytes die with the failed runtime; do not retain/replay them.
                runtime = runtimeFactory({ source }); runtimeError = '';
                if (paused) runtime.pauseCatalog();
                for (const item of catalogs.values()) { item.job = null; enqueueCatalog(item); }
                for (const [key, item] of inspected) { item.job = null; enqueueInspection(key, item); }
                if (scanActive) for (const row of library.rows) queueInspection(row);
            }
            if (rootFailed && !walkTask) startWalk();
            changed();
        },
        scanMetadata() {
            if (disposed) return;
            scanActive = true;
            for (const row of library.rows) queueInspection(row);
            changed();
        },
        cancelMetadata() {
            if (disposed) return;
            scanActive = false; metadataPhase = 'cancelled';
            inspected.clear(); runtime.cancelMetadata(); changed();
        },
        retryMetadata(locator) {
            if (disposed) return;
            queueInspection(rowFor(locator), true);
            if (inspected.size && !scanActive) metadataPhase = 'running';
            changed();
        },
        acceptPlaybackMetadata(locator, metadata, bytes) {
            if (disposed) return;
            locator = { ...locator };
            const key = locatorKey(locator), version = (versions.get(key) || 0) + 1;
            versions.set(key, version);
            const previousInspection = inspected.get(key);
            inspected.delete(key); previousInspection?.job?.cancel();
            const previousHash = hashes.get(key);
            hashes.delete(key); previousHash?.job.cancel();
            // Publish a ready patch even before enumeration reaches this locator.
            if (runtimeError) { patchRow(locator, { ...metadataPatch(metadata), sha256: '' }); return; }
            const epoch = runtimeEpoch;
            const job = runtime.enqueue({ generation, kind: 'hash', bytes });
            const item = { job, version };
            hashes.set(key, item);
            patchRow(locator, { ...metadataPatch(metadata), sha256: '' });
            const current = () => !disposed && epoch === runtimeEpoch && hashes.get(key) === item && version === versions.get(key);
            job.result.then(result => {
                if (!current() || result.generation !== generation) return;
                hashes.delete(key); patchRow(locator, { sha256: result.payload.sha256 });
            }, error => {
                if (!current()) return;
                hashes.delete(key);
                if (error.code === 'worker-failed') failRuntime(error);
                else if (!controlFlow(error)) addIssue(joinedPath(locator), error.message || String(error));
                if (controlFlow(error)) queueInspection(rowFor(locator));
                changed();
            });
            changed();
        },
        attachComments(comments) {
            if (disposed) return;
            const result = attachComments(library, comments);
            changed(library.commentChangedRowIds);
            return result;
        },
        dispose() {
            if (disposed) return;
            disposed = true; runtimeEpoch += 1;
            walkController?.abort(); wakeWalk?.(); wakeWalk = null;
            // Do not await an outstanding directory read in order to dispose the context.
            try { Promise.resolve(iterator?.return?.()).catch(() => {}); } catch { /* Cleanup must remain silent. */ }
            runtime.dispose(); catalogs.clear(); inspected.clear(); hashes.clear(); cache.clear();
            if (timer !== null) unschedule(timer);
            changedRows.clear(); metadataUpdates.clear();
        },
    };
    context.discovery = controller;
    context.discoveryStatus = status();
    return controller;
}
