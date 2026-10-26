import { MAX_SOURCE_BYTES, libraryWorkerError, validateLibraryPayload } from './library-worker-jobs.js';

const RECYCLE_BYTES = 32 * 1024 * 1024;
const PRIORITY = { hash: 0, catalog: 1, inspect: 2 };
const defaultWorkerFactory = () => new Worker(new URL('./library-worker.js', import.meta.url), { type: 'module' });

// Queued source jobs retain locators only. The sole exception is one owned playback-hash copy.
export function createLibraryWorkerRuntime({ source, workerFactory = defaultWorkerFactory }) {
    let nextId = 1;
    let active = null;
    let transport = null;
    let completedJobs = 0;
    let disposed = false;
    let suspended = false;
    let unreportedWorkerFailure = null;
    let catalogPaused = false;
    let pumpPending = false;
    const queues = { hash: new Map(), catalog: new Map(), inspect: new Map() };
    const queuedJobs = () => [...queues.hash.values(), ...queues.catalog.values(), ...queues.inspect.values()];

    function schedule() {
        if (pumpPending) return;
        pumpPending = true;
        queueMicrotask(() => { pumpPending = false; dispatch(); });
    }
    function settle(job, error, payload) {
        if (job.settled) return;
        job.settled = true;
        job.bytes = null;
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
        const current = { worker, ready, resolveReady, rejectReady };
        transport = current;
        worker.onmessage = event => {
            if (transport !== current || disposed) return;
            const message = event.data;
            if (message?.type === 'ready') { resolveReady(); return; }
            if (message?.type === 'startup-error') { workerFailed(message.error || new Error('Worker initialization failed')); return; }
            const job = active;
            if (!job || message?.id !== job.id || message.generation !== job.generation || message.kind !== job.kind || !job.posted) return;
            if (message.type !== 'result' && message.type !== 'error') return;
            let error;
            let payload;
            if (message.type === 'error') {
                if (message.error?.code === 'worker-failed') { workerFailed(message.error); return; }
                // Ordinary data errors settle only this job and keep the instance usable.
                const code = message.error?.code === 'source-too-large' ? 'source-too-large' : 'job-failed';
                error = libraryWorkerError(code, message.error?.message ?? 'Library worker job failed');
            } else {
                try { payload = validateLibraryPayload(job.kind, message.payload); }
                catch (failure) { error = failure; }
            }
            active = null;
            settle(job, error, payload);
            completedJobs += 1;
            // Deliver the result before retiring high-water WASM memory, between jobs.
            if (job.inputBytes >= RECYCLE_BYTES || completedJobs >= 100) stopWorker();
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
            let bytes;
            if (job.kind === 'hash') {
                bytes = job.bytes;
            } else {
                phase = 'read';
                const size = await source.size(job.locator.sourcePath);
                if (!currentAttempt(job, attempt)) return;
                if (size !== null && size > MAX_SOURCE_BYTES) throw libraryWorkerError('source-too-large', `Source exceeds ${MAX_SOURCE_BYTES} bytes`);
                const read = await source.readFile(job.locator.sourcePath, { signal: job.controller.signal });
                if (!currentAttempt(job, attempt)) return;
                const view = read instanceof ArrayBuffer ? new Uint8Array(read) : new Uint8Array(read.buffer, read.byteOffset, read.byteLength);
                // Source reads own their buffer; trim a view only if needed for safe transfer.
                bytes = view.byteOffset === 0 && view.byteLength === view.buffer.byteLength ? view.buffer : view.slice().buffer;
            }
            if (bytes.byteLength > MAX_SOURCE_BYTES) throw libraryWorkerError('source-too-large', `Source exceeds ${MAX_SOURCE_BYTES} bytes`);
            job.inputBytes = bytes.byteLength;
            job.bytes = null;
            phase = 'worker';
            job.posted = true;
            current.worker.postMessage({ type: 'job', id: job.id, generation: job.generation, kind: job.kind, bytes, ...(job.kind === 'inspect' ? { locator: job.locator } : {}) }, [bytes]);
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
        enqueue(request) {
            let resolve;
            let reject;
            const result = new Promise((accept, fail) => { resolve = accept; reject = fail; });
            const job = { id: nextId++, generation: request.generation, kind: request.kind, resolve, reject, attempt: 0, settled: false, bytes: null, posted: false };
            if (disposed) queueMicrotask(() => settle(job, libraryWorkerError('disposed', 'Library worker runtime disposed')));
            else if (!Object.hasOwn(PRIORITY, request.kind)) queueMicrotask(() => settle(job, libraryWorkerError('job-failed', 'Unknown library worker job')));
            else {
                if (request.kind === 'hash') {
                    // Release the preceding owned copy before retaining its replacement.
                    for (const previous of [...queues.hash.values(), ...(active?.kind === 'hash' ? [active] : [])]) cancel(previous);
                    try {
                        const bytes = request.bytes;
                        if (!(bytes instanceof ArrayBuffer) && !ArrayBuffer.isView(bytes)) throw new Error('Hash input must be bytes');
                        if (bytes.byteLength > MAX_SOURCE_BYTES) throw libraryWorkerError('source-too-large', `Source exceeds ${MAX_SOURCE_BYTES} bytes`);
                        job.bytes = (bytes instanceof ArrayBuffer ? new Uint8Array(bytes) : new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength)).slice().buffer;
                    } catch (error) { queueMicrotask(() => settle(job, libraryWorkerError(error.code === 'source-too-large' ? 'source-too-large' : 'job-failed', error?.message ?? error))); }
                } else job.locator = { ...(request.kind === 'catalog' ? { sourcePath: request.sourcePath } : request.locator) };
                if (job.bytes || job.kind !== 'hash') { queues[job.kind].set(job.id, job); schedule(); }
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
            for (const job of [...queues.inspect.values(), ...(active?.kind === 'inspect' ? [active] : [])]) cancel(job);
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
