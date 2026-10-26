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
    runtimeFactory = createLibraryWorkerRuntime, schedule = setTimeout, unschedule = clearTimeout,
    digest = bytes => globalThis.crypto.subtle.digest('SHA-256', bytes), onDigestMetric = () => {}, now = () => performance.now() }) {
    const { source, library } = context;
    let runtime = runtimeFactory({ source });
    let runtimeEpoch = 1;
    let disposed = false, started = false, paused = false, walkDone = false, rootFailed = false;
    let walkController = null, iterator = null, walkTask = null, wakeWalk = null;
    let runtimeError = '';
    let scanActive = false, metadataPhase = 'idle';
    let timer = null;
    const catalogs = new Map(), inspected = new Map(), digests = new Map(), refreshes = new Map();
    const examined = new Set(), cataloged = new Set(), cache = new Map(), versions = new Map();
    const issues = [], issueKeys = new Set();
    const changedRows = new Set(), rowChanges = new Map(), metadataUpdates = new Map();
    const counts = { ready: 0, unknown: 0, error: 0, excluded: 0 };
    const actionableRows = new Set();
    let missingHashes = 0, refreshPending = 0, pendingInputBytes = 0, nextDigestId = 1, issuesRevision = 0;
    let lastPhase = 'running', lastMetadataPhase = 'idle';

    function rowFor(locator) { return library.rows[library.locatorIds.get(locatorKey(locator))]; }
    function countRow(row, direction) {
        if (!row.playable) { counts.excluded += direction; return; }
        counts[row.metadataState] += direction;
        if (!row.sha256) missingHashes += direction;
        if (direction < 0) actionableRows.delete(row.id);
        else if (isScanActionable(row)) actionableRows.add(row.id);
    }
    function isScanActionable(row) {
        const key = locatorKey(row);
        return row.playable && row.metadataState !== 'error' && (row.metadataState !== 'ready' || !row.sha256)
            && !inspected.has(key) && !refreshes.has(key);
    }
    function refreshActionable(locator) {
        const row = rowFor(locator);
        if (row) { actionableRows.delete(row.id); if (isScanActionable(row)) actionableRows.add(row.id); }
    }
    function status() {
        const phase = paused ? 'paused' : !walkDone ? 'running' : rootFailed || (runtimeError && catalogs.size) ? 'failed' : catalogs.size ? 'running' : 'complete';
        if (scanActive || inspected.size || refreshPending) {
            if (runtimeError && (scanActive || inspected.size)) metadataPhase = 'suspended';
            else if (inspected.size || refreshPending) metadataPhase = 'running';
            else if (phase !== 'complete') metadataPhase = 'waiting';
            else { scanActive = false; metadataPhase = 'complete'; }
        }
        let issueSnapshot;
        const issueLength = issues.length;
        if (!scanActive && metadataPhase === 'running' && !inspected.size && !refreshPending) metadataPhase = 'complete';
        return Object.freeze({ phase, filesExamined: examined.size, archivesPending: catalogs.size, tracksFound: library.rows.length,
            issuesRevision, get issues() { return issueSnapshot ||= Object.freeze(issues.slice(0, issueLength)); }, issueCount: issues.length, metadataPhase, runtimeError,
            metadataCounts: Object.freeze({ ...counts, hashesPending: digests.size, refreshPending, scanActionable: actionableRows.size }),
            metadataIncomplete: counts.unknown + counts.error > 0 || missingHashes > 0 || refreshPending > 0,
            sidecarCounts: Object.freeze({ ...library.commentCounts }) });
    }
    function flush() {
        if (disposed) return;
        if (timer !== null) { unschedule(timer); timer = null; }
        const change = { changedRowIds: [...changedRows], statusChanged: true, metadataUpdates: [...metadataUpdates.values()],
            rowChanges: [...rowChanges.values()].map(change => ({ ...change, fields: [...change.fields] })), libraryRevision: library.revision, issuesRevision };
        changedRows.clear(); rowChanges.clear(); metadataUpdates.clear();
        onUpdate(context, change);
    }
    function changed(ids = [], updates = [], immediate = false, deltas = []) {
        if (disposed) return;
        for (const id of ids) changedRows.add(id);
        for (const delta of deltas) {
            const previous = rowChanges.get(delta.id);
            const added = Boolean(previous?.added || delta.added);
            rowChanges.set(delta.id, { id: delta.id, added, fields: new Set(added ? ['*'] : [...(previous?.fields || []), ...delta.fields]) });
        }
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
        if (!issueKeys.has(key)) { issueKeys.add(key); issues.push(Object.freeze({ id: issues.length, path, reason })); issuesRevision += 1; }
    }
    function patchRow(locator, patch) {
        const key = locatorKey(locator);
        cache.set(key, { ...cache.get(key), ...patch });
        const row = rowFor(locator);
        if (row) countRow(row, -1);
        const ids = updateDiscoveredRows(library, [{ locator, patch }]);
        if (row) countRow(row, 1);
        changed(ids, [{ locator, patch }], false, library.lastRowChanges);
    }
    function append(records) {
        const ids = appendDiscoveredRows(library, records.map(record => ({ ...record, ...cache.get(locatorKey(record)), ...(record.playable === false ? { error: record.error } : {}) })));
        const deltas = library.lastRowChanges;
        for (const id of ids) {
            const row = library.rows[id];
            if (!row.playable && refreshes.has(locatorKey(row))) refreshPending -= 1;
            countRow(row, 1);
        }
        for (const id of ids) queueInspection(library.rows[id]);
        changed(ids, [], false, deltas);
    }
    function failRuntime(error) {
        runtimeError = error.message || String(error);
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
        if (inspected.has(key) || refreshes.has(key) || !force && (row.metadataState === 'error' || row.metadataState === 'ready' && row.sha256)) return;
        const item = { locator: { sourcePath: row.sourcePath, containerMember: row.containerMember, memberPath: row.memberPath, rawHex: row.rawHex }, version: versions.get(key) || 0, job: null };
        inspected.set(key, item);
        refreshActionable(row);
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
            refreshActionable(item.locator);
            if (!controlFlow(error)) patchRow(item.locator, { metadataState: 'error', error: error.message || String(error) });
            else changed();
        });
    }
    function emitDigestMetric(event, item, extra = {}) {
        // Optional instrumentation contains scalars only; never retain or expose input bytes.
        try { onDigestMetric({ event, id: item.id, byteLength: item.byteLength, timestamp: now(), pending: digests.size, pendingInputBytes, ...extra }); }
        catch (error) { console.error('Playback digest metric callback failed', error); }
    }
    function finishDigest(item, sha256, error, failed = false) {
        const current = !disposed && item.generation === generation && refreshes.get(item.key) === item && versions.get(item.key) === item.version;
        if (refreshes.get(item.key) === item) {
            refreshes.delete(item.key);
            if (rowFor(item.locator)?.playable !== false) refreshPending -= 1;
            refreshActionable(item.locator);
        }
        if (!current) return;
        if (failed) addIssue(joinedPath(item.locator), error?.message || String(error));
        patchRow(item.locator, { ...item.patch, sha256: failed ? '' : sha256 });
    }
    function watchDigest(item, promise) {
        // This closure receives the promise and scalar/snapshot record, never caller bytes.
        Promise.resolve(promise).then(result => {
            if (!(result instanceof ArrayBuffer) || result.byteLength !== 32) throw new Error('Invalid SHA-256 digest result');
            const sha256 = Array.from(new Uint8Array(result), byte => byte.toString(16).padStart(2, '0')).join('');
            finishDigest(item, sha256, null);
        }).catch(error => finishDigest(item, null, error, true)).finally(() => {
            digests.delete(item.id); pendingInputBytes -= item.byteLength;
            emitDigestMetric('settle', item, { latency: now() - item.started });
            changed();
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
            const cancelled = [...inspected.values()];
            inspected.clear(); runtime.cancelMetadata();
            for (const item of cancelled) refreshActionable(item.locator);
            changed();
        },
        retryMetadata(locator) {
            if (disposed) return;
            queueInspection(rowFor(locator), true);
            if (inspected.size && !scanActive) metadataPhase = 'running';
            changed();
        },
        acceptPlaybackMetadata(locator, metadata, bytes) {
            if (disposed) return;
            const capturedLocator = Object.fromEntries(['sourcePath', 'containerMember', 'memberPath', 'rawHex'].map(field => [field, typeof locator[field] === 'string' ? locator[field] : '']));
            const capturedMetadata = Object.fromEntries(['format', 'title', 'instrumentNames', 'sampleNames', 'songMessage'].map(field => [field, typeof metadata[field] === 'string' ? metadata[field] : '']));
            capturedMetadata.channels = Number.isSafeInteger(metadata.channels) && metadata.channels > 0 ? metadata.channels : null;
            const key = locatorKey(capturedLocator), version = (versions.get(key) || 0) + 1;
            versions.set(key, version);
            const previousInspection = inspected.get(key);
            inspected.delete(key); previousInspection?.job?.cancel();
            // R4b must invalidate only this descriptor/member, never cancel a shared group.
            if (!refreshes.has(key) && rowFor(capturedLocator)?.playable !== false) refreshPending += 1;
            const item = { id: nextDigestId++, key, locator: capturedLocator, patch: metadataPatch(capturedMetadata), version, generation,
                byteLength: bytes.byteLength, started: now() };
            refreshes.set(key, item);
            refreshActionable(capturedLocator);
            let promise;
            try { promise = digest(bytes); }
            catch (error) { finishDigest(item, null, error, true); changed(); return; }
            digests.set(item.id, item); pendingInputBytes += item.byteLength;
            emitDigestMetric('start', item, { startCost: now() - item.started });
            watchDigest(item, promise);
            changed();
        },
        attachComments(comments) {
            if (disposed) return;
            const result = attachComments(library, comments);
            changed(library.commentChangedRowIds, [], false, library.lastRowChanges);
            return result;
        },
        dispose() {
            if (disposed) return;
            disposed = true; runtimeEpoch += 1;
            walkController?.abort(); wakeWalk?.(); wakeWalk = null;
            // Do not await an outstanding directory read in order to dispose the context.
            try { Promise.resolve(iterator?.return?.()).catch(() => {}); } catch { /* Cleanup must remain silent. */ }
            runtime.dispose(); catalogs.clear(); inspected.clear(); cache.clear();
            if (timer !== null) unschedule(timer);
            changedRows.clear(); rowChanges.clear(); metadataUpdates.clear();
        },
    };
    for (const row of library.rows) countRow(row, 1);
    context.discovery = controller;
    context.discoveryStatus = status();
    return controller;
}
