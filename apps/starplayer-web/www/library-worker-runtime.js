import { MAX_SOURCE_BYTES, libraryWorkerError, validateLibraryPayload } from './library-worker-jobs.js';

const RECYCLE_BYTES = 32 * 1024 * 1024;
const PRIORITY = { catalog: 0, 'inspect-group': 1 };
let nextInstanceId = 1;
const defaultWorkerFactory = () => new Worker(new URL('./library-worker.js', import.meta.url), { type: 'module' });

// Queued jobs retain frozen descriptors only; source bytes are read at dispatch.
export function createLibraryWorkerRuntime({ source, workerFactory = defaultWorkerFactory, onMetric = () => {} }) {
    let nextId = 1;
    let active = null;
    let transport = null;
    let completedJobs = 0;
    let disposed = false;
    let suspended = false;
    let unreportedWorkerFailure = null;
    let catalogPaused = false;
    let pumpPending = false;
    const queues = { catalog: new Map(), 'inspect-group': new Map() };
    const queuedJobs = () => [...queues.catalog.values(), ...queues['inspect-group'].values()];
    const metric = (event, extra = {}) => { try { onMetric({ event, ...extra }); } catch (error) { console.error('Library worker metric callback failed', error); } };
    const completedTokens = job => job.members?.filter(member => job.completed.has(member.token)).map(member => member.token) || [];

    function schedule() {
        if (pumpPending) return;
        pumpPending = true;
        queueMicrotask(() => { pumpPending = false; dispatch(); });
    }
    function settle(job, error, payload) {
        if (job.settled) return;
        job.settled = true;
        if (job.kind === 'inspect-group') {
            const progress = { completedTokens: completedTokens(job) };
            if (error) error = Object.assign(libraryWorkerError(error.code, error.message), error, { progress });
            else payload = progress;
        }
        if (error) job.reject(error);
        else job.resolve({ generation: job.generation, kind: job.kind, payload });
    }
    function stopWorker() {
        const previous = transport;
        transport = null;
        completedJobs = 0;
        if (!previous) return;
        previous.worker.onmessage = previous.worker.onerror = previous.worker.onmessageerror = null;
        // Unblock startup waiters even if cancellation happens before ready.
        previous.rejectReady(libraryWorkerError('aborted', 'Worker interrupted'));
        try { previous.worker.terminate(); } catch { /* Already stopped. */ }
    }
    function workerFailed(error) {
        const failure = libraryWorkerError('worker-failed', error?.message ?? error);
        if (error?.cause !== undefined) failure.cause = error.cause;
        suspended = true;
        if (active) {
            const job = active;
            active = null;
            job.attempt += 1;
            job.controller?.abort();
            settle(job, failure);
        } else {
            // An idle worker has no active promise through which to notify the controller.
            // Keep one failure until a queued or future job can report it, even while paused.
            unreportedWorkerFailure = failure;
        }
        stopWorker();
        schedule();
    }
    function ensureWorker() {
        if (transport) return transport;
        const worker = workerFactory();
        let resolveReady;
        let rejectReady;
        const ready = new Promise((resolve, reject) => { resolveReady = resolve; rejectReady = reject; });
        // Startup can be interrupted synchronously, before dispatch attaches its await.
        ready.catch(() => {});
        const current = { worker, ready, resolveReady, rejectReady, instanceId: nextInstanceId++ };
        metric('instance', { instanceId: current.instanceId });
        transport = current;
        worker.onmessage = event => {
            if (transport !== current || disposed) return;
            const message = event.data;
            if (message?.type === 'ready') { resolveReady(); return; }
            if (message?.type === 'startup-error') { workerFailed(message.error || new Error('Worker initialization failed')); return; }
            const job = active;
            if (!job || message?.id !== job.id || message.generation !== job.generation || message.kind !== job.kind || message.instanceId !== current.instanceId || message.attempt !== job.attempt || !job.posted) return;
            if (message.type === 'member-result' && job.kind === 'inspect-group') {
                const member = job.memberMap.get(message.token);
                if (!member || job.completed.has(member.token)) return;
                let payload, error;
                try {
                    if (message.error) error = libraryWorkerError(message.error.code === 'source-too-large' ? 'source-too-large' : 'job-failed', message.error.message || 'Member inspection failed');
                    else payload = validateLibraryPayload('member', message.payload);
                } catch (failure) { error = failure; }
                // Delivery belongs to this runtime, even if the consumer fails or ignores stale data.
                job.completed.add(member.token);
                try { job.onMember?.({ generation: job.generation, id: job.id, ...member, ...(error ? { error } : { payload }) }); }
                catch (failure) {
                    console.error('Library metadata callback failed', failure);
                    try { job.onIssue?.({ generation: job.generation, id: job.id, ...member, path: [member.locator.sourcePath, member.locator.containerMember, member.locator.memberPath].filter(Boolean).join('/'), reason: 'internal error applying metadata' }); }
                    catch (issueError) { console.error('Library metadata issue callback failed', issueError); }
                }
                return;
            }
            if (message.type !== (job.kind === 'inspect-group' ? 'group-result' : 'result') && message.type !== 'error') return;
            let error;
            let payload;
            if (message.type === 'error') {
                if (message.error?.code === 'worker-failed') { workerFailed(message.error); return; }
                // Ordinary data errors settle only this job and keep the instance usable.
                const code = message.error?.code === 'source-too-large' ? 'source-too-large' : 'job-failed';
                error = libraryWorkerError(code, message.error?.message ?? 'Library worker job failed');
            } else {
                try { if (job.kind === 'catalog') payload = validateLibraryPayload(job.kind, message.payload);
                    else if (job.completed.size !== job.members.length) throw libraryWorkerError('job-failed', 'Incomplete inspection group'); }
                catch (failure) { error = failure; }
            }
            active = null;
            settle(job, error, payload);
            completedJobs += job.kind === 'inspect-group' ? job.members.length : 1;
            // Deliver the result before retiring high-water WASM memory, between jobs.
            if (job.inputBytes >= RECYCLE_BYTES || completedJobs >= 100) { metric('recycle', { instanceId: current.instanceId, operations: completedJobs }); stopWorker(); }
            schedule();
        };
        worker.onerror = event => {
            if (transport !== current || disposed) return;
            event.preventDefault?.();
            workerFailed(new Error(event.message || 'Library worker crashed'));
        };
        worker.onmessageerror = () => {
            if (transport === current && !disposed) workerFailed(new Error('Library worker transport failed'));
        };
        return current;
    }
    function currentAttempt(job, attempt) { return !disposed && active === job && job.attempt === attempt && !job.settled; }
    async function dispatch() {
        if (disposed || active || (suspended && !unreportedWorkerFailure)) return;
        let job;
        for (const kind of Object.keys(PRIORITY)) {
            if (catalogPaused && kind === 'catalog' && !suspended) continue;
            job = queues[kind].values().next().value;
            if (job) break;
        }
        if (!job) return;
        queues[job.kind].delete(job.id);
        if (suspended) {
            const failure = unreportedWorkerFailure;
            unreportedWorkerFailure = null;
            settle(job, failure);
            return;
        }
        active = job;
        const attempt = ++job.attempt;
        job.controller = new AbortController();
        let phase = 'worker';
        try {
            const current = ensureWorker();
            await current.ready;
            if (!currentAttempt(job, attempt)) return;
            phase = 'read';
            const size = await source.size(job.sourcePath);
            if (!currentAttempt(job, attempt)) return;
            if (size !== null && size > MAX_SOURCE_BYTES) throw libraryWorkerError('source-too-large', `Source exceeds ${MAX_SOURCE_BYTES} bytes`);
            const read = await source.readFile(job.sourcePath, { signal: job.controller.signal });
            if (!currentAttempt(job, attempt)) return;
            const view = read instanceof ArrayBuffer ? new Uint8Array(read) : new Uint8Array(read.buffer, read.byteOffset, read.byteLength);
            // Transfer exactly the source view; no per-member outer copies.
            const bytes = view.byteOffset === 0 && view.byteLength === view.buffer.byteLength ? view.buffer : view.slice().buffer;
            metric('read', { kind: job.kind, sourcePath: job.sourcePath, bytes: bytes.byteLength });
            if (bytes.byteLength > MAX_SOURCE_BYTES) throw libraryWorkerError('source-too-large', `Source exceeds ${MAX_SOURCE_BYTES} bytes`);
            job.inputBytes = bytes.byteLength;
            phase = 'worker';
            job.posted = true;
            current.worker.postMessage({ type: 'job', instanceId: current.instanceId, id: job.id, generation: job.generation, attempt, kind: job.kind, bytes, ...(job.members ? { members: job.members } : {}) }, [bytes]);
        } catch (error) {
            if (!currentAttempt(job, attempt)) return;
            if (phase === 'worker' && error.code !== 'source-too-large') { workerFailed(error); return; }
            active = null;
            settle(job, libraryWorkerError(error.code === 'source-too-large' ? 'source-too-large' : 'read-failed', error?.message ?? error));
            schedule();
        }
    }
    function cancel(job, code = 'aborted') {
        if (job.settled) return;
        queues[job.kind]?.delete(job.id);
        if (active === job) {
            active = null;
            job.attempt += 1;
            job.controller?.abort();
            stopWorker();
        }
        settle(job, libraryWorkerError(code, code === 'disposed' ? 'Library worker runtime disposed' : 'Library worker job cancelled'));
        schedule();
    }
    return {
        enqueue(request, { onMember, onIssue } = {}) {
            let resolve;
            let reject;
            const result = new Promise((accept, fail) => { resolve = accept; reject = fail; });
            const job = { id: nextId++, generation: request.generation, kind: request.kind, resolve, reject, attempt: 0, settled: false, posted: false, completed: new Set(), onMember, onIssue };
            if (disposed) queueMicrotask(() => settle(job, libraryWorkerError('disposed', 'Library worker runtime disposed')));
            else if (!Object.hasOwn(PRIORITY, request.kind)) queueMicrotask(() => settle(job, libraryWorkerError('job-failed', 'Unknown library worker job')));
            else {
                try {
                    if (typeof request.sourcePath !== 'string') throw new Error('Source path is required');
                    job.sourcePath = request.sourcePath;
                    if (request.kind === 'inspect-group') {
                        if (!Array.isArray(request.members) || !request.members.length || request.members.length > 64) throw new Error('Inspection groups require 1–64 members');
                        job.members = Object.freeze(request.members.map(member => {
                            if (typeof member.token !== 'string' || !Number.isSafeInteger(member.version) || member.locator?.sourcePath !== job.sourcePath) throw new Error('Invalid inspection member');
                            const locator = Object.freeze(Object.fromEntries(['sourcePath', 'containerMember', 'memberPath', 'rawHex'].map(field => [field, typeof member.locator[field] === 'string' ? member.locator[field] : ''])));
                            return Object.freeze({ token: member.token, version: member.version, locator });
                        }));
                        job.memberMap = new Map(job.members.map(member => [member.token, member]));
                        if (job.memberMap.size !== job.members.length || job.members.some(member => !member.locator.memberPath) && job.members.length !== 1) throw new Error('Invalid inspection group');
                    }
                    queues[job.kind].set(job.id, job); schedule();
                } catch (error) { queueMicrotask(() => settle(job, libraryWorkerError('job-failed', error.message))); }
            }
            return { id: job.id, result, cancel: () => cancel(job) };
        },
        pauseCatalog() {
            catalogPaused = true;
            if (active?.kind === 'catalog') {
                const job = active;
                active = null;
                job.attempt += 1;
                job.controller?.abort();
                job.posted = false;
                stopWorker();
                queues.catalog = new Map([[job.id, job], ...queues.catalog]);
                schedule();
            }
        },
        resumeCatalog() { catalogPaused = false; schedule(); },
        cancelMetadata() {
            for (const job of [...queues['inspect-group'].values(), ...(active?.kind === 'inspect-group' ? [active] : [])]) cancel(job);
        },
        dispose() {
            if (disposed) return;
            disposed = true;
            unreportedWorkerFailure = null;
            for (const job of [...queuedJobs(), ...(active ? [active] : [])]) cancel(job, 'disposed');
            stopWorker();
        },
    };
}
