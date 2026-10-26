import test from 'node:test';
import assert from 'node:assert/strict';
import { createLibraryWorkerRuntime } from '../www/library-worker-runtime.js';
import { executeLibraryJob, startLibraryWorker, MAX_SOURCE_BYTES, MAX_EXTRACTED_BYTES } from '../www/library-worker-jobs.js';
import { webcrypto } from 'node:crypto';

const tick = () => new Promise(resolve => setImmediate(resolve));
const deferred = () => { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const catalog = { entries: [], issues: [] };
const metadata = { format: 'mod', title: 'A title', channels: 4, instrumentNames: '\nBass', sampleNames: 'Kick\n', songMessage: '' };
const sha256 = 'a'.repeat(64);
const locator = sourcePath => ({ kind: 'library', sourcePath, containerMember: 'inner.zip', memberPath: 'song.mod', rawHex: 'abcd' });
function fixture({ source: overrides = {}, autoReady = true, workerFactory } = {}) {
    const workers = [];
    const reads = [];
    const sizes = [];
    const source = { size: async path => { sizes.push(path); return 4; }, readFile: async path => { reads.push(path); return new Uint8Array([1, 2, 3, 4]).buffer; }, ...overrides };
    function factory() {
        const worker = {
            jobs: [], transfers: [], terminated: false,
            postMessage(message, transfer) {
                this.transfers.push(transfer[0]);
                this.jobs.push(structuredClone(message, { transfer }));
            },
            emit(message) { this.onmessage?.({ data: message }); },
            ready() { this.emit({ type: 'ready' }); },
            reply(payload, tag = {}) {
                const job = this.jobs.at(-1);
                this.emit({ type: 'result', id: job.id, generation: job.generation, kind: job.kind, payload: payload ?? (job.kind === 'catalog' ? catalog : job.kind === 'inspect' ? { metadata, sha256 } : { sha256 }), ...tag });
            },
            fail(code = 'job-failed', message = 'bad file') {
                const { id, generation, kind } = this.jobs.at(-1);
                this.emit({ type: 'error', id, generation, kind, error: { name: 'Error', code, message } });
            },
            terminate() { this.terminated = true; },
        };
        workers.push(worker);
        if (autoReady) queueMicrotask(() => worker.ready());
        return worker;
    }
    const runtime = createLibraryWorkerRuntime({ source, workerFactory: workerFactory || factory });
    const enqueue = job => {
        const handle = runtime.enqueue({ generation: 7, ...job });
        // Consumers may retain work through multiple turns without unhandled rejections.
        handle.result.catch(() => {});
        return handle;
    };
    return { runtime, enqueue, workers, reads, sizes };
}
const expectCode = (promise, code) => assert.rejects(promise, error => error.code === code && error.name === (code === 'aborted' || code === 'disposed' ? 'AbortError' : 'Error') && typeof error.message === 'string');

test('priority is hash/catalog/inspect with serial dispatch and no queued archive reads', async () => {
    const { runtime, enqueue, workers, reads, sizes } = fixture();
    const inspect = enqueue({ kind: 'inspect', locator: locator('track.zip') });
    const first = enqueue({ kind: 'catalog', sourcePath: 'first.zip' });
    const second = enqueue({ kind: 'catalog', sourcePath: 'second.zip' });
    const hash = enqueue({ kind: 'hash', bytes: new Uint8Array([9]) });
    await tick();
    assert.equal(workers[0].jobs[0].kind, 'hash');
    assert.deepEqual(reads, []);
    workers[0].reply();
    assert.deepEqual(await hash.result, { generation: 7, kind: 'hash', payload: { sha256 } });
    await tick();
    assert.deepEqual(reads, ['first.zip']);
    assert.deepEqual(sizes, ['first.zip']);
    workers[0].reply(); await first.result; await tick();
    assert.deepEqual(reads, ['first.zip', 'second.zip']);
    workers[0].reply(); await second.result; await tick();
    assert.deepEqual(reads, ['first.zip', 'second.zip', 'track.zip']);
    workers[0].reply(); await inspect.result;
    runtime.dispose();
});

test('hash copies borrowed subview once and transfers only its owned buffer', async () => {
    const { runtime, enqueue, workers } = fixture();
    const bytes = new Uint8Array([0, 1, 2, 3]);
    const job = enqueue({ kind: 'hash', bytes: bytes.subarray(1, 3) });
    bytes[1] = 99;
    await tick();
    assert.equal(bytes.byteLength, 4);
    assert.deepEqual([...new Uint8Array(workers[0].jobs[0].bytes)], [1, 2]);
    assert.equal(workers[0].transfers[0].byteLength, 0);
    workers[0].reply(); await job.result;
    runtime.dispose();
});

test('source reads transfer ownership, queued jobs retain locator snapshots only', async () => {
    const input = new Uint8Array([1, 2]);
    const { runtime, enqueue, workers } = fixture({ source: { readFile: async () => input.buffer } });
    const original = locator('original.zip');
    const job = enqueue({ kind: 'inspect', locator: original });
    original.sourcePath = 'mutated.zip';
    await tick();
    assert.equal(input.byteLength, 0);
    assert.equal(workers[0].jobs[0].locator.sourcePath, 'original.zip');
    workers[0].reply(); await job.result;
    runtime.dispose();
});

test('superseding pending and active hashes rejects old work and keeps caller buffers intact', async () => {
    const { runtime, enqueue, workers } = fixture();
    const bytes = new Uint8Array([1]);
    const first = enqueue({ kind: 'hash', bytes });
    const second = enqueue({ kind: 'hash', bytes: bytes.buffer });
    await expectCode(first.result, 'aborted');
    await tick();
    const stale = workers[0].onmessage;
    const oldJob = workers[0].jobs[0];
    const third = enqueue({ kind: 'hash', bytes });
    await expectCode(second.result, 'aborted');
    assert.equal(workers[0].terminated, true);
    await tick();
    stale({ data: { ...oldJob, type: 'result', payload: { sha256 } } });
    workers[1].reply(); await third.result;
    assert.equal(bytes.byteLength, 1);
    runtime.dispose();
});

test('size rejects before reading and unrelated work continues', async () => {
    const { runtime, enqueue, workers, reads } = fixture({ source: { size: async path => path === 'huge.zip' ? MAX_SOURCE_BYTES + 1 : null } });
    const huge = enqueue({ kind: 'catalog', sourcePath: 'huge.zip' });
    const good = enqueue({ kind: 'catalog', sourcePath: 'good.zip' });
    await expectCode(huge.result, 'source-too-large'); await tick();
    assert.deepEqual(reads, ['good.zip']);
    workers[0].reply(); await good.result;
    runtime.dispose();
});

test('post-read size checks unknown sizes and read failures do not suspend', async () => {
    const { runtime, enqueue, workers } = fixture({ source: {
        size: async () => null,
        readFile: async path => {
            if (path === 'bad.zip') throw new Error('permission denied');
            if (path === 'huge.zip') return new ArrayBuffer(MAX_SOURCE_BYTES + 1);
            return new ArrayBuffer(1);
        },
    } });
    const bad = enqueue({ kind: 'catalog', sourcePath: 'bad.zip' });
    const huge = enqueue({ kind: 'catalog', sourcePath: 'huge.zip' });
    const good = enqueue({ kind: 'catalog', sourcePath: 'good.zip' });
    await expectCode(bad.result, 'read-failed');
    await expectCode(huge.result, 'source-too-large'); await tick();
    workers[0].reply(); await good.result;
    runtime.dispose();
});

test('binding failures and invalid result JSON/schema reject only their job', async () => {
    const { runtime, enqueue, workers } = fixture();
    const first = enqueue({ kind: 'catalog', sourcePath: 'bad.zip' });
    const second = enqueue({ kind: 'inspect', locator: locator('bad.mod') });
    const third = enqueue({ kind: 'inspect', locator: locator('good.mod') });
    await tick(); workers[0].fail(); await expectCode(first.result, 'job-failed');
    await tick(); workers[0].reply({ metadata: { ...metadata, channels: 0 }, sha256 }); await expectCode(second.result, 'job-failed');
    await tick(); workers[0].reply(); await third.result;
    runtime.dispose();
});

for (const failure of ['startup', 'crash', 'messageerror', 'post']) {
    test(`${failure} worker failure rejects active and suspends queued work until recreation`, async () => {
        const { runtime, enqueue, workers, reads } = fixture({ autoReady: false });
        const first = enqueue({ kind: 'catalog', sourcePath: 'a.zip' });
        const second = enqueue({ kind: 'catalog', sourcePath: 'b.zip' });
        await tick();
        if (failure === 'startup') workers[0].emit({ type: 'startup-error', error: { message: 'no wasm' } });
        if (failure === 'crash') workers[0].onerror({ message: 'crashed', preventDefault() {} });
        if (failure === 'messageerror') workers[0].onmessageerror();
        if (failure === 'post') { workers[0].postMessage = () => { throw new Error('clone failed'); }; workers[0].ready(); }
        await expectCode(first.result, 'worker-failed');
        await tick();
        assert.equal(workers.length, 1);
        assert.equal(workers[0].terminated, true);
        assert.equal(reads.includes('b.zip'), false);
        runtime.resumeCatalog(); await tick(); assert.equal(workers.length, 1);
        runtime.dispose(); await expectCode(second.result, 'disposed');
    });
}

test('synchronous worker construction failure is asynchronous to enqueue caller', async () => {
    const { runtime, enqueue } = fixture({ workerFactory: () => { throw new Error('CSP'); } });
    const job = enqueue({ kind: 'catalog', sourcePath: 'a.zip' });
    await expectCode(job.result, 'worker-failed');
    runtime.dispose();
});

test('cancel mid-read releases slot immediately and late file data cannot post', async () => {
    const read = deferred();
    const signals = [];
    const { runtime, enqueue, workers } = fixture({ source: { readFile: async (path, { signal }) => { signals.push(signal); return path === 'slow.zip' ? read.promise : new ArrayBuffer(1); } } });
    const slow = enqueue({ kind: 'catalog', sourcePath: 'slow.zip' });
    const next = enqueue({ kind: 'catalog', sourcePath: 'next.zip' });
    await tick(); slow.cancel(); await expectCode(slow.result, 'aborted'); await tick();
    assert.equal(signals[0].aborted, true);
    assert.equal(workers[1].jobs.length, 1);
    read.resolve(new ArrayBuffer(1)); await tick();
    assert.equal(workers[0].jobs.length, 0);
    workers[1].reply(); await next.result;
    runtime.dispose();
});

test('cancel during size lookup does not read, and queued cancellation creates no worker', async () => {
    const size = deferred();
    const { runtime, enqueue, workers, reads } = fixture({ source: { size: () => size.promise } });
    const first = enqueue({ kind: 'catalog', sourcePath: 'a.zip' });
    const queued = enqueue({ kind: 'inspect', locator: locator('b.zip') });
    queued.cancel(); await expectCode(queued.result, 'aborted');
    await tick(); first.cancel(); await expectCode(first.result, 'aborted');
    size.resolve(2); await tick(); assert.deepEqual(reads, []); assert.equal(workers.length, 1);
    runtime.dispose();
});

test('pause active catalog requeues same promise while inspect proceeds; stale replies ignored', async () => {
    const { runtime, enqueue, workers } = fixture();
    const archive = enqueue({ kind: 'catalog', sourcePath: 'a.zip' });
    const inspect = enqueue({ kind: 'inspect', locator: locator('b.zip') });
    let settlements = 0;
    archive.result.then(() => { settlements += 1; });
    await tick();
    const stale = workers[0].onmessage;
    const old = workers[0].jobs[0];
    runtime.pauseCatalog(); await tick();
    assert.equal(workers[0].terminated, true);
    assert.equal(workers[1].jobs[0].kind, 'inspect');
    stale({ data: { ...old, type: 'result', payload: catalog } });
    workers[1].reply(); await inspect.result; await tick(); assert.equal(settlements, 0);
    runtime.resumeCatalog(); await tick();
    assert.equal(workers[1].jobs[1].id, archive.id);
    workers[1].reply(); await archive.result; workers[1].reply(); await tick();
    assert.equal(settlements, 1);
    runtime.dispose();
});

test('pause before startup and late ready cannot dispatch the interrupted attempt', async () => {
    const { runtime, enqueue, workers } = fixture({ autoReady: false });
    const archive = enqueue({ kind: 'catalog', sourcePath: 'a.zip' });
    await tick();
    const stale = workers[0].onmessage;
    runtime.pauseCatalog();
    stale({ data: { type: 'ready' } }); await tick();
    assert.equal(workers[0].jobs.length, 0);
    runtime.resumeCatalog(); await tick(); workers[1].ready(); await tick();
    workers[1].reply(); await archive.result;
    runtime.dispose();
});

test('pause mid-read reuses promise and rereads on resume without buffering stale bytes', async () => {
    const read = deferred();
    let reads = 0;
    const { runtime, enqueue, workers } = fixture({ source: { readFile: () => ++reads === 1 ? read.promise : new ArrayBuffer(1) } });
    const job = enqueue({ kind: 'catalog', sourcePath: 'a.zip' });
    await tick(); runtime.pauseCatalog(); runtime.resumeCatalog(); await tick();
    read.resolve(new ArrayBuffer(5)); await tick();
    assert.equal(workers[1].jobs.length, 1);
    workers[1].reply(); await job.result; runtime.dispose();
});

test('cancelMetadata cancels inspect only and preserves active/queued archive jobs', async () => {
    const { runtime, enqueue, workers } = fixture();
    const inspect = enqueue({ kind: 'inspect', locator: locator('a.zip') });
    await tick();
    const archive = enqueue({ kind: 'catalog', sourcePath: 'b.zip' });
    const next = enqueue({ kind: 'inspect', locator: locator('c.zip') });
    runtime.cancelMetadata(); await expectCode(inspect.result, 'aborted'); await expectCode(next.result, 'aborted');
    await tick(); assert.equal(workers[1].jobs[0].kind, 'catalog'); workers[1].reply(); await archive.result;
    const later = enqueue({ kind: 'inspect', locator: locator('d.zip') });
    await tick(); workers[1].reply(); await later.result;
    runtime.dispose();
});

test('job and generation tags gate stale replies and settle exactly once', async () => {
    const { runtime, enqueue, workers } = fixture();
    const job = enqueue({ kind: 'catalog', sourcePath: 'a.zip', generation: 12 });
    let settlements = 0;
    job.result.then(() => { settlements += 1; });
    await tick();
    workers[0].reply(undefined, { generation: 11 });
    workers[0].reply(undefined, { id: job.id + 1 });
    workers[0].reply(undefined, { kind: 'inspect' });
    await tick(); assert.equal(settlements, 0);
    workers[0].reply(); assert.equal((await job.result).generation, 12);
    workers[0].reply(); job.cancel(); runtime.dispose(); await tick(); assert.equal(settlements, 1);
});

test('recycle after 32 MiB input and every 100 completed jobs, preserving queue', async () => {
    const { runtime, enqueue, workers } = fixture({ source: { size: async () => 32 * 1024 * 1024, readFile: async () => new ArrayBuffer(32 * 1024 * 1024) } });
    const big = enqueue({ kind: 'catalog', sourcePath: 'big.zip' });
    const next = enqueue({ kind: 'hash', bytes: new Uint8Array([1]) });
    await tick(); workers[0].reply(); await next.result; await tick();
    workers[0].reply(); await big.result;
    assert.equal(workers[0].terminated, true);
    for (let count = 0; count < 100; count += 1) {
        const job = enqueue({ kind: 'hash', bytes: new Uint8Array([count]) });
        await tick(); workers[1].reply(); await job.result;
        assert.equal(workers[1].terminated, count === 99);
    }
    const after = enqueue({ kind: 'hash', bytes: new Uint8Array([1]) });
    await tick(); assert.equal(workers.length, 3); workers[2].reply(); await after.result;
    runtime.dispose();
});

test('dispose rejects active and queued jobs, is idempotent and rejects later enqueues asynchronously', async () => {
    const { runtime, enqueue, workers } = fixture({ autoReady: false });
    const first = enqueue({ kind: 'catalog', sourcePath: 'a.zip' });
    const second = enqueue({ kind: 'hash', bytes: new Uint8Array([1]) });
    const third = enqueue({ kind: 'inspect', locator: locator('b.zip') });
    await tick(); runtime.dispose(); runtime.dispose();
    await Promise.all([first, second, third].map(job => expectCode(job.result, 'disposed')));
    assert.equal(workers[0].terminated, true);
    await expectCode(enqueue({ kind: 'catalog', sourcePath: 'c.zip' }).result, 'disposed');
});

function bindings(overrides = {}) {
    const calls = [];
    const loader = {
        archive_catalog(bytes) { calls.push(['catalog', bytes.byteLength]); return JSON.stringify(catalog); },
        archive_extract_path(bytes, ...names) { calls.push(['extract', ...names]); return new Uint8Array([9, 8]); },
        inspect_s3m(bytes) { calls.push(['inspect', [...bytes]]); },
        module_metadata() { calls.push(['metadata']); return JSON.stringify(metadata); },
        ...overrides,
    };
    return { calls, loader, digest: bytes => webcrypto.subtle.digest('SHA-256', bytes) };
}

test('worker catalog never extracts/inspects members; no-module catalog is valid', async () => {
    const fixture = bindings();
    assert.deepEqual(await executeLibraryJob({ kind: 'catalog', bytes: new ArrayBuffer(2) }, fixture), catalog);
    assert.deepEqual(fixture.calls, [['catalog', 2]]);
});

test('worker inspect uses exact existing path extraction and hashes extracted bytes', async () => {
    const fixture = bindings();
    const result = await executeLibraryJob({ kind: 'inspect', locator: locator('a.zip'), bytes: new ArrayBuffer(4) }, fixture);
    assert.deepEqual(result.metadata, metadata);
    assert.equal(result.sha256, '4674d5929bf80d90d5d455d37bd530ca60bdf6a870c7014d92b529a89c4c4e95');
    assert.deepEqual(fixture.calls, [['extract', 'inner.zip', 'song.mod', 'abcd'], ['inspect', [9, 8]], ['metadata']]);
});

test('worker loose inspection captures metadata before asynchronous digest', async () => {
    const pending = deferred();
    const fixture = bindings();
    const promise = executeLibraryJob({ kind: 'inspect', locator: { sourcePath: 'a.mod' }, bytes: new Uint8Array([1]).buffer }, { ...fixture, digest: () => pending.promise });
    assert.deepEqual(fixture.calls, [['inspect', [1]], ['metadata']]);
    pending.resolve(new Uint8Array(32).buffer);
    assert.deepEqual((await promise).metadata, metadata);
});

test('worker rejects invalid binding JSON/schema, throws and oversized extracted entries', async () => {
    await assert.rejects(executeLibraryJob({ kind: 'catalog', bytes: new ArrayBuffer(1) }, bindings({ archive_catalog: () => '{' })), SyntaxError);
    await expectCode(executeLibraryJob({ kind: 'catalog', bytes: new ArrayBuffer(1) }, bindings({ archive_catalog: () => '{}' })), 'job-failed');
    await assert.rejects(executeLibraryJob({ kind: 'catalog', bytes: new ArrayBuffer(1) }, bindings({ archive_catalog: () => { throw new Error('corrupt'); } })), /corrupt/);
    await expectCode(executeLibraryJob({ kind: 'inspect', locator: locator('a.zip'), bytes: new ArrayBuffer(1) }, bindings({ archive_extract_path: () => new Uint8Array(MAX_EXTRACTED_BYTES + 1) })), 'source-too-large');
});


test('unknown job kinds and malformed/oversized hash inputs reject asynchronously without worker failure', async () => {
    const { runtime, enqueue, workers } = fixture();
    await expectCode(enqueue({ kind: 'toString' }).result, 'job-failed');
    await expectCode(enqueue({ kind: 'hash', bytes: 1 }).result, 'job-failed');
    await expectCode(enqueue({ kind: 'hash', bytes: new ArrayBuffer(MAX_SOURCE_BYTES + 1) }).result, 'source-too-large');
    assert.equal(workers.length, 0);
    const good = enqueue({ kind: 'hash', bytes: new Uint8Array(1) });
    await tick(); workers[0].reply(); await good.result; runtime.dispose();
});

test('worker catalog validation rejects unknown format tokens', async () => {
    const invalid = { entries: [{ containerMember: '', memberPath: 'song.mod', rawHex: 'ab', size: 2, format: 'MOD', playable: true, error: '' }], issues: [] };
    await expectCode(executeLibraryJob({ kind: 'catalog', bytes: new ArrayBuffer(1) }, bindings({ archive_catalog: () => JSON.stringify(invalid) })), 'job-failed');
});


test('worker endpoint reports startup errors separately from binding/decoding job errors', async () => {
    const messages = [];
    const scope = { postMessage: message => messages.push(message) };
    await startLibraryWorker({ scope, loadLoader: async () => { throw new Error('no WASM'); } });
    assert.deepEqual(messages.pop(), { type: 'startup-error', error: { name: 'Error', code: 'worker-failed', message: 'no WASM' } });
    assert.equal(scope.onmessage, undefined);
    const { loader } = bindings({ archive_catalog: () => '{' });
    await startLibraryWorker({ scope, loadLoader: async () => loader });
    assert.deepEqual(messages.pop(), { type: 'ready' });
    const tag = { id: 42, generation: 9, kind: 'catalog' };
    await scope.onmessage({ data: { ...tag, bytes: new ArrayBuffer(1) } });
    const failure = messages.pop();
    assert.deepEqual({ type: failure.type, id: failure.id, generation: failure.generation, kind: failure.kind }, { type: 'error', ...tag });
    assert.equal(failure.error.code, 'job-failed');
    assert.equal(failure.error.name, 'Error');
    loader.archive_catalog = () => { throw new Error('bad outer ZIP'); };
    await scope.onmessage({ data: { ...tag, bytes: new ArrayBuffer(1) } });
    assert.deepEqual(messages.pop().error, { name: 'Error', code: 'job-failed', message: 'bad outer ZIP' });
    loader.archive_catalog = () => JSON.stringify(catalog);
    await scope.onmessage({ data: { ...tag, bytes: new ArrayBuffer(1) } });
    assert.deepEqual(messages.pop(), { type: 'result', ...tag, payload: catalog });
});

test('paused catalog returns ahead of its queued siblings with original promise identity', async () => {
    const { runtime, enqueue, workers, reads } = fixture();
    const first = enqueue({ kind: 'catalog', sourcePath: 'first.zip' });
    const second = enqueue({ kind: 'catalog', sourcePath: 'second.zip' });
    const third = enqueue({ kind: 'catalog', sourcePath: 'third.zip' });
    const promise = first.result;
    await tick(); runtime.pauseCatalog(); await tick(); runtime.resumeCatalog(); await tick();
    assert.equal(first.result, promise);
    assert.equal(workers[1].jobs[0].id, first.id);
    workers[1].reply(); await promise; await tick();
    assert.equal(workers[1].jobs[1].id, second.id);
    workers[1].reply(); await second.result; await tick();
    assert.equal(workers[1].jobs[2].id, third.id);
    workers[1].reply(); await third.result;
    assert.deepEqual(reads, ['first.zip', 'first.zip', 'second.zip', 'third.zip']);
    runtime.dispose();
});


for (const kind of ['catalog', 'inspect', 'hash']) {
    test(`idle worker crash is reported through the next ${kind} job without resuming the queue`, async () => {
        const { runtime, enqueue, workers, reads } = fixture();
        const initial = enqueue({ kind: 'catalog', sourcePath: 'initial.zip' });
        await tick(); workers[0].reply(); await initial.result; await tick();
        const staleError = workers[0].onerror;
        staleError({ message: 'idle crash' });
        assert.equal(workers[0].terminated, true);
        const request = kind === 'catalog' ? { kind, sourcePath: 'next.zip' } : kind === 'inspect' ? { kind, locator: locator('next.zip') } : { kind, bytes: new Uint8Array([1]) };
        const next = enqueue(request);
        const unrelated = enqueue({ kind: 'inspect', locator: locator('waiting.zip') });
        let settlements = 0;
        next.result.catch(() => { settlements += 1; });
        await assert.rejects(next.result, error => error.code === 'worker-failed' && error.name === 'Error' && error.message === 'idle crash');
        next.cancel(); staleError({ message: 'duplicate stale crash' });
        runtime.resumeCatalog(); await tick();
        assert.equal(workers.length, 1);
        assert.deepEqual(reads, ['initial.zip']);
        assert.equal(settlements, 1);
        runtime.dispose(); await expectCode(unrelated.result, 'disposed');
    });
}

test('idle transport failure reports one paused catalog job while preserving the remaining queue', async () => {
    const { runtime, enqueue, workers, reads } = fixture();
    const initial = enqueue({ kind: 'catalog', sourcePath: 'initial.zip' });
    await tick(); workers[0].reply(); await initial.result; await tick();
    runtime.pauseCatalog();
    const first = enqueue({ kind: 'catalog', sourcePath: 'first.zip' });
    const second = enqueue({ kind: 'catalog', sourcePath: 'second.zip' });
    workers[0].onmessageerror();
    await expectCode(first.result, 'worker-failed');
    let secondSettled = false;
    second.result.catch(() => { secondSettled = true; });
    runtime.resumeCatalog(); await tick();
    assert.equal(secondSettled, false);
    assert.deepEqual(reads, ['initial.zip']);
    assert.equal(workers.length, 1);
    runtime.dispose(); await expectCode(second.result, 'disposed');
});

test('cancellation and hash supersession do not consume an unreported idle failure', async () => {
    const { runtime, enqueue, workers } = fixture();
    const initial = enqueue({ kind: 'catalog', sourcePath: 'initial.zip' });
    await tick(); workers[0].reply(); await initial.result; await tick();
    workers[0].onerror({ message: 'idle crash' });
    const cancelled = enqueue({ kind: 'catalog', sourcePath: 'cancelled.zip' });
    cancelled.cancel();
    await expectCode(cancelled.result, 'aborted');
    const firstHash = enqueue({ kind: 'hash', bytes: new Uint8Array([1]) });
    const replacement = enqueue({ kind: 'hash', bytes: new Uint8Array([2]) });
    await expectCode(firstHash.result, 'aborted');
    await expectCode(replacement.result, 'worker-failed');
    assert.equal(workers.length, 1);
    runtime.dispose();
});

test('dispose before deferred idle-failure delivery rejects queued work once as disposed', async () => {
    const { runtime, enqueue, workers } = fixture();
    const initial = enqueue({ kind: 'catalog', sourcePath: 'initial.zip' });
    await tick(); workers[0].reply(); await initial.result; await tick();
    workers[0].onerror({ message: 'idle crash' });
    const next = enqueue({ kind: 'hash', bytes: new Uint8Array([1]) });
    let settlements = 0;
    next.result.catch(() => { settlements += 1; });
    runtime.dispose(); runtime.dispose();
    await expectCode(next.result, 'disposed');
    await tick(); assert.equal(settlements, 1);
    await expectCode(enqueue({ kind: 'catalog', sourcePath: 'later.zip' }).result, 'disposed');
});
