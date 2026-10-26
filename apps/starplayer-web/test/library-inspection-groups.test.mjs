import test from 'node:test';
import assert from 'node:assert/strict';
import { createLibraryWorkerRuntime } from '../www/library-worker-runtime.js';
import { createDiscoveryController } from '../www/library-discovery.js';
import { createDiscoveredLibrary, appendDiscoveredRows } from '../www/library-index.js';
import { executeLibraryJob, startLibraryWorker } from '../www/library-worker-jobs.js';

const tick = () => new Promise(resolve => setImmediate(resolve));
const metadata = title => ({ format: 'mod', title, channels: 4, instrumentNames: '', sampleNames: '', songMessage: '' });
const sha256 = 'a'.repeat(64);
const locator = (memberPath = 'a.mod', containerMember = '', sourcePath = 'outer.zip') => ({ sourcePath, containerMember, memberPath, rawHex: 'abcd' });
const member = (token, where = locator()) => ({ token, locator: where, version: 0 });
const group = (members = [member('a')]) => ({ kind: 'inspect-group', generation: 7, sourcePath: members[0].locator.sourcePath, members, purpose: 'scan' });
const tags = job => Object.fromEntries(['instanceId', 'id', 'generation', 'attempt', 'kind'].map(field => [field, job[field]]));
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
function bindings() {
    const counts = { sessions: 0, frees: 0, extractionCalls: 0, modeledInnerLoads: 0 };
    const observed = [];
    const loader = {
        // Models the native cache contract; actual inner-decode counters live in Rust tests.
        ArchiveInspectionSession: class {
            constructor(bytes) { counts.sessions += 1; this.snapshot = bytes[0]; this.inner = ''; }
            extract_member(container, path, raw) {
                counts.extractionCalls += 1;
                if (container && container !== this.inner) counts.modeledInnerLoads += 1;
                this.inner = container; observed.push([this.snapshot, container, path, raw]); return new Uint8Array([this.snapshot]);
            }
            free() { counts.frees += 1; }
        },
        archive_catalog: () => JSON.stringify({ entries: [], issues: [] }),
        inspect_s3m() {}, module_metadata: () => JSON.stringify(metadata('scan')),
    };
    return { counts, observed, loader };
}
function harness({ size = 4, read, binding = bindings(), records = [], wrapCallbacks = callbacks => callbacks, enumerate, digest } = {}) {
    const workers = [], metrics = [], inspectionMetrics = [], digestPending = deferred();
    const source = { size: async () => size, readFile: async () => read ? read() : new Uint8Array(size).fill(1).buffer,
        enumerate: enumerate || (async function* () {}) };
    const runtimeFactory = ({ source }) => {
        const runtime = createLibraryWorkerRuntime({ source, onMetric: metric => metrics.push(metric), workerFactory() {
            const scope = { postMessage: message => { if (!worker.terminated) worker.onmessage?.({ data: structuredClone(message) }); } };
            const worker = {
                jobs: [], terminated: false,
                postMessage(message, transfer) { this.jobs.push(structuredClone(message, { transfer })); },
                run() { return scope.onmessage({ data: this.jobs.at(-1) }); },
                emit(message) { this.onmessage?.({ data: message }); },
                frame(token, extra = {}) { this.emit({ type: 'member-result', ...tags(this.jobs.at(-1)), token, payload: { metadata: metadata('scan'), sha256 }, ...extra }); },
                finish() { this.emit({ type: 'group-result', ...tags(this.jobs.at(-1)), payload: { completedTokens: ['forged'] } }); },
                terminate() { this.terminated = true; },
            };
            workers.push(worker); void startLibraryWorker({ scope, loadLoader: async () => binding.loader }); return worker;
        } });
        const enqueue = runtime.enqueue;
        runtime.enqueue = (request, callbacks) => enqueue(request, callbacks ? wrapCallbacks(callbacks) : callbacks);
        return runtime;
    };
    const library = createDiscoveredLibrary({ name: 'groups' });
    appendDiscoveredRows(library, records.map(where => ({ ...where, format: 'mod', size: 4, timestamp: '', playable: true, error: '' })));
    const context = { source, library }, runtime = runtimeFactory({ source });
    const controller = createDiscoveryController({ context, runtimeFactory, digest: digest || (() => digestPending.promise), onInspectionMetric: metric => inspectionMetrics.push(metric) });
    const enqueue = (request, callbacks) => { const job = runtime.enqueue(request, callbacks); job.result.catch(() => {}); return job; };
    return { workers, metrics, inspectionMetrics, runtime, enqueue, controller, context, binding, digestPending, dispose() { runtime.dispose(); controller.dispose(); } };
}

for (const nested of [false, true]) for (const size of [4, 32 * 1024 * 1024]) for (const total of [3, 64, 130]) {
    test(`${total} ${nested ? 'nested' : 'direct'} members, ${size} bytes: bounded reads, sessions and recycling`, async t => {
        const records = Array.from({ length: total }, (_, index) => locator(`${String(index).padStart(3, '0')}.mod`, nested ? 'inner.zip' : ''));
        const h = harness({ size, records }); t.after(() => h.dispose()); h.controller.start(); await tick(); h.controller.scanMetadata();
        const groups = Math.ceil(total / 64);
        for (let index = 0; index < groups; index += 1) { await tick(); const worker = h.workers.at(-1); assert.ok(worker.jobs.at(-1).members.length <= 64); await worker.run(); await tick(); }
        const reads = h.metrics.filter(metric => metric.event === 'read');
        assert.equal(reads.length, groups); assert.equal(reads.reduce((sum, metric) => sum + metric.bytes, 0), size * groups);
        assert.equal(h.binding.counts.sessions, groups); assert.equal(h.binding.counts.frees, groups); assert.equal(h.binding.counts.extractionCalls, total);
        assert.equal(h.binding.counts.modeledInnerLoads, nested ? groups : 0);
        assert.equal(h.metrics.filter(metric => metric.event === 'recycle').length, size >= 32 * 1024 * 1024 ? groups : total >= 100 ? 1 : 0);
        assert.equal(h.workers.length, size >= 32 * 1024 * 1024 ? groups : total > 128 ? 2 : 1);
        assert.equal(h.context.discoveryStatus.metadataCounts.ready, total); assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
        assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 0);
    });
}

test('trusted token mapping ignores duplicate/unknown/stale frames, snapshots member locator/version and orders progress', async t => {
    const h = harness(); t.after(() => h.dispose()); const original = member('a', locator(), 0), seen = [];
    original.version = 4;
    const job = h.enqueue(group([original, member('b')]), { onMember: result => seen.push(result) }); original.locator.sourcePath = 'mutated.zip'; original.version = 8;
    await tick(); const worker = h.workers[0];
    for (const extra of [{ id: -1 }, { generation: -1 }, { instanceId: -1 }, { attempt: -1 }, { kind: 'catalog' }]) worker.frame('a', extra);
    worker.frame('unknown'); assert.equal(seen.length, 0);
    worker.frame('b'); worker.frame('a', { locator: locator('forged.mod'), version: 999 }); worker.frame('a'); worker.finish();
    assert.deepEqual((await job.result).payload.completedTokens, ['a', 'b']); assert.deepEqual(seen.map(result => result.token), ['b', 'a']);
    assert.equal(seen[1].locator.sourcePath, 'outer.zip'); assert.equal(seen[1].version, 4);
});

test('delivery is marked before a cancelling callback; progress cannot come from the worker', async t => {
    const h = harness(); t.after(() => h.dispose()); let job;
    job = h.enqueue(group(), { onMember() { job.cancel(); } }); await tick(); h.workers[0].frame('a');
    await assert.rejects(job.result, error => error.code === 'aborted' && error.progress.completedTokens.join() === 'a'); assert.equal(h.workers[0].terminated, true);
});

test('playing A invalidates only A in frozen A/B/C; B/C settle and newer playback wins', async t => {
    const h = harness({ records: ['a', 'b', 'c'].map(name => locator(`${name}.mod`)) }); t.after(() => h.dispose());
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick(); const worker = h.workers[0], sent = worker.jobs[0];
    h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('played A'), new Uint8Array([3]));
    assert.equal(worker.terminated, false); h.digestPending.resolve(new Uint8Array(32).fill(2).buffer); await tick();
    await worker.run(); await tick();
    assert.deepEqual(h.context.library.rows.map(row => row.title), ['played A', 'scan', 'scan']); assert.equal(h.context.discoveryStatus.metadataCounts.ready, 3);
    assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 0); assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
    worker.emit({ type: 'member-result', ...tags(sent), token: sent.members[0].token, payload: { metadata: metadata('late A'), sha256 } });
    h.controller.scanMetadata(); assert.equal(worker.jobs.length, 1); assert.equal(h.context.library.rows[0].title, 'played A');
});

for (const when of ['before', 'after']) test(`callback throw ${when} mutation records a controller issue, reconciles delivered descriptors and continues`, async t => {
    let thrown = false; const logs = [], previous = console.error; console.error = (...args) => logs.push(args); t.after(() => { console.error = previous; });
    const h = harness({ records: ['a', 'b', 'c'].map(name => locator(`${name}.mod`)), wrapCallbacks(callbacks) { return { ...callbacks, onMember(result) {
        if (!thrown) { thrown = true; if (when === 'after') callbacks.onMember(result); throw new Error('application bug'); }
        callbacks.onMember(result);
    } }; } });
    t.after(() => h.dispose()); h.controller.start(); await tick(); h.controller.scanMetadata(); await tick(); await h.workers[0].run(); await tick();
    assert.deepEqual(h.context.library.rows.map(row => row.metadataState), [when === 'before' ? 'unknown' : 'ready', 'ready', 'ready']);
    assert.deepEqual(h.context.discoveryStatus.issues.map(issue => [issue.path, issue.reason]), [['outer.zip/a.mod', 'internal error applying metadata']]);
    assert.equal(h.context.discoveryStatus.runtimeError, ''); assert.equal(logs.length, 1); assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
    assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, when === 'before' ? 1 : 0);
    h.controller.scanMetadata(); await tick(); assert.equal(h.workers[0].jobs.length, when === 'before' ? 2 : 1);
    if (when === 'before') { assert.equal(h.workers[0].jobs[1].members.length, 1); await h.workers[0].run(); await tick(); }
    assert.equal(h.context.discoveryStatus.metadataCounts.ready, 3);
});

test('reporting-hook throw logs once without becoming fatal or losing progress', async t => {
    const h = harness(), previous = console.error, logs = []; console.error = (...args) => logs.push(args); t.after(() => { console.error = previous; h.dispose(); });
    const seen = []; const job = h.enqueue(group([member('a'), member('b')]), { onMember(result) { seen.push(result.token); if (result.token === 'a') throw new Error('callback'); }, onIssue() { throw new Error('hook'); } });
    await tick(); h.workers[0].frame('a'); h.workers[0].frame('b'); h.workers[0].finish();
    assert.deepEqual((await job.result).payload.completedTokens, ['a', 'b']); assert.deepEqual(seen, ['a', 'b']); assert.equal(logs.length, 2); assert.equal(h.workers[0].terminated, false);
});

test('partial trap Retry resubmits only undelivered members while page digest spans failure and Retry', async t => {
    const h = harness({ records: ['a', 'b', 'c'].map(name => locator(`${name}.mod`)) }); t.after(() => h.dispose());
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick(); const worker = h.workers[0], sent = worker.jobs[0]; worker.frame(sent.members[0].token);
    h.controller.acceptPlaybackMetadata(locator('c.mod'), metadata('played C'), new Uint8Array([1, 2]));
    worker.emit({ type: 'error', ...tags(sent), error: { code: 'worker-failed', cause: 'wasm-trap', message: 'unreachable' } }); await tick();
    assert.equal(worker.terminated, true); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 1); assert.equal(h.context.discoveryStatus.metadataCounts.error, 0);
    h.controller.retry(); await tick(); const retry = h.workers[1].jobs[0]; assert.deepEqual(retry.members.map(item => item.locator.memberPath), ['b.mod']);
    assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 1); await h.workers[1].run(); await tick();
    h.digestPending.resolve(new Uint8Array(32).fill(3).buffer); await tick();
    assert.deepEqual(h.context.library.rows.map(row => row.title), ['scan', 'scan', 'played C']); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 0);
    assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 0); assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
});

test('actual extraction trap skips poisoned free, suspends later groups and Retry preserves completed rows', async t => {
    const records = Array.from({ length: 65 }, (_, index) => locator(`${String(index).padStart(3, '0')}.mod`));
    const h = harness({ records }), Base = h.binding.loader.ArchiveInspectionSession;
    t.after(() => h.dispose());
    let trap = true, poisonedFrees = 0;
    h.binding.loader.ArchiveInspectionSession = class extends Base {
        extract_member(...args) {
            if (trap && args[1] === '001.mod') { this.poisoned = true; wasmUnreachable(); }
            return super.extract_member(...args);
        }
        free() {
            if (this.poisoned) {
                poisonedFrees += 1;
                throw new Error('recursive use of an object detected which would lead to unsafe aliasing in rust');
            }
            super.free();
        }
    };
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick();
    const worker = h.workers[0];
    await worker.run(); await tick();
    assert.equal(h.context.discoveryStatus.runtimeError, 'unreachable');
    assert.equal(h.context.discoveryStatus.metadataPhase, 'suspended');
    assert.deepEqual(h.context.library.rows.map(row => row.metadataState), ['ready', ...Array(64).fill('unknown')]);
    assert.equal(h.context.discoveryStatus.metadataCounts.error, 0);
    assert.equal(h.binding.counts.sessions, 1); assert.equal(h.binding.counts.frees, 0); assert.equal(poisonedFrees, 0);
    assert.equal(worker.terminated, true);
    h.controller.resume(); h.controller.scanMetadata(); await tick();
    assert.equal(h.workers.length, 1); assert.equal(worker.jobs.length, 1);
    assert.equal(h.metrics.filter(metric => metric.event === 'read').length, 1);

    trap = false;
    h.controller.retry(); await tick();
    assert.equal(h.workers.length, 2);
    const retry = h.workers[1], sent = retry.jobs[0];
    assert.deepEqual(sent.members.map(item => item.locator.memberPath), records.slice(1).map(item => item.memberPath));
    await retry.run(); await tick();
    assert.equal(retry.terminated, false); assert.equal(retry.jobs.length, 1);
    assert.equal(h.binding.counts.sessions, 2); assert.equal(h.binding.counts.frees, 1); assert.equal(poisonedFrees, 0);
    assert.equal(h.context.discoveryStatus.metadataCounts.ready, 65);
    assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 0);
    assert.equal(h.context.discoveryStatus.runtimeError, ''); assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
});

test('Stop preserves executing group; Cancel metadata terminates it and never requeues undispatched groups', async t => {
    const h = harness({ records: Array.from({ length: 130 }, (_, index) => locator(`${index}.mod`)) }); t.after(() => h.dispose());
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick(); const worker = h.workers[0]; worker.frame(worker.jobs[0].members[0].token);
    h.controller.pause(); assert.equal(worker.terminated, false); h.controller.acceptPlaybackMetadata(locator('new.mod', '', 'new.mod'), metadata('early'), new Uint8Array([1]));
    h.controller.cancelMetadata(); await tick(); assert.equal(worker.terminated, true); assert.equal(h.context.discoveryStatus.metadataCounts.ready, 1); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 1);
    h.controller.resume(); await tick(); assert.equal(h.workers.length, 1); h.digestPending.resolve(new Uint8Array(32).buffer); await tick();
    assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 0); assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 129);
    h.controller.scanMetadata(); await tick(); assert.equal(h.workers.length, 2); assert.equal(h.workers[1].jobs[0].members.length, 64);
});

test('outer source mutation is observed on the next group, never midway through a snapshot', async t => {
    let content = 1; const h = harness({ records: Array.from({ length: 65 }, (_, index) => locator(`${String(index).padStart(3, '0')}.mod`)), read: () => new Uint8Array([content]).buffer }); t.after(() => h.dispose());
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick(); const original = h.binding.loader.ArchiveInspectionSession.prototype.extract_member;
    h.binding.loader.ArchiveInspectionSession.prototype.extract_member = function(...args) { content = 2; return original.call(this, ...args); };
    await h.workers[0].run(); await tick(); await h.workers[0].run(); await tick(); assert.deepEqual(h.binding.observed.map(item => item[0]), [...Array(64).fill(1), 2]);
});

test('mixed nested and direct errors are per member and exact raw names reach the session', async () => {
    const binding = bindings(), Base = binding.loader.ArchiveInspectionSession, delivered = [];
    binding.loader.ArchiveInspectionSession = class extends Base { extract_member(container, path, raw) { if (path === 'bad.mod') throw 'ordinary Rust error'; return super.extract_member(container, path, raw); } };
    await executeLibraryJob({ ...group([member('bad', locator('bad.mod')), member('nested', locator('next.mod', 'inner.zip')), member('direct', locator('last.mod'))]), bytes: new Uint8Array([7]).buffer }, { loader: binding.loader, onMember: result => delivered.push(result) });
    assert.equal(delivered[0].error.code, 'job-failed'); assert.equal(delivered[1].payload.metadata.title, 'scan'); assert.equal(delivered[2].payload.metadata.title, 'scan');
    assert.deepEqual(binding.observed.map(item => item.slice(1)), [['inner.zip', 'next.mod', 'abcd'], ['', 'last.mod', 'abcd']]); assert.equal(binding.counts.frees, 1);
});

test('incremental arrivals submit only new descriptors despite thousands of frozen queued members', async t => {
    let release; const waiting = new Promise(resolve => { release = resolve; });
    const h = harness({ records: Array.from({ length: 4096 }, (_, index) => locator(`${String(index).padStart(4, '0')}.mod`)), enumerate: async function* () {
        await waiting;
        for (let index = 0; index < 128; index += 1) yield { kind: 'file', path: `${index}.mod`, size: 4, timestamp: '' };
    } }); t.after(() => h.dispose()); h.controller.start(); await tick(); h.controller.scanMetadata();
    assert.deepEqual(h.inspectionMetrics, [{ event: 'submit', descriptors: 4096 }]); release(); await tick();
    assert.equal(h.context.library.rows.length, 4224); assert.equal(h.inspectionMetrics.length, 129);
    assert.equal(h.inspectionMetrics.slice(1).reduce((sum, metric) => sum + metric.descriptors, 0), 128);
    assert.ok(h.inspectionMetrics.slice(1).every(metric => metric.descriptors === 1));
});

test('Cancel between groups preserves completed rows, cancels every queued group and leaves a page digest outstanding', async t => {
    const h = harness({ records: Array.from({ length: 130 }, (_, index) => locator(`${String(index).padStart(3, '0')}.mod`)) }); t.after(() => h.dispose());
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick(); const worker = h.workers[0];
    for (const item of worker.jobs[0].members) worker.frame(item.token);
    worker.finish(); // The runtime has settled group one, before its queued dispatch microtask.
    h.controller.acceptPlaybackMetadata(locator('129.mod'), metadata('played'), new Uint8Array([4])); h.controller.cancelMetadata(); await tick();
    assert.equal(worker.jobs.length, 1); assert.equal(h.context.discoveryStatus.metadataCounts.ready, 64); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 1);
    assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 65); h.digestPending.resolve(new Uint8Array(32).buffer); await tick();
    assert.equal(h.context.discoveryStatus.metadataCounts.ready, 65); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 0);
});

test('Cancel during extraction terminates the executing instance and suppresses its remaining frames', async t => {
    const h = harness({ records: ['a', 'b', 'c'].map(name => locator(`${name}.mod`)) }); t.after(() => h.dispose());
    const Base = h.binding.loader.ArchiveInspectionSession;
    h.binding.loader.ArchiveInspectionSession = class extends Base {
        extract_member(...args) { const bytes = super.extract_member(...args); if (args[1] === 'b.mod') h.controller.cancelMetadata(); return bytes; }
    };
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick(); const worker = h.workers[0]; await worker.run(); await tick();
    assert.equal(worker.terminated, true); assert.deepEqual(h.context.library.rows.map(row => row.metadataState), ['ready', 'unknown', 'unknown']);
    assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 2); assert.equal(h.context.discoveryStatus.runtimeError, '');
});

test('source replacement disposes old descriptors; in-flight frames and digests cannot update either accepted context', async t => {
    const old = harness({ records: [locator()] }), replacement = harness({ records: [locator()] }); t.after(() => { old.dispose(); replacement.dispose(); });
    old.controller.start(); await tick(); old.controller.scanMetadata(); await tick(); const worker = old.workers[0], stale = worker.onmessage, sent = worker.jobs[0];
    old.controller.acceptPlaybackMetadata(locator(), metadata('obsolete'), new Uint8Array([1])); old.controller.dispose();
    replacement.controller.start(); await tick(); stale({ data: { type: 'member-result', ...tags(sent), token: sent.members[0].token, payload: { metadata: metadata('stale'), sha256 } } });
    old.digestPending.resolve(new Uint8Array(32).buffer); await tick();
    assert.equal(worker.terminated, true); assert.equal(old.context.library.rows[0].metadataState, 'unknown'); assert.equal(replacement.context.library.rows[0].metadataState, 'unknown');
    assert.equal(replacement.context.discoveryStatus.metadataCounts.hashesPending, 0); assert.equal(replacement.context.discoveryStatus.issueCount, 0);
});

test('normal group recycling preserves pending page digests and later playback', async t => {
    const h = harness({ size: 32 * 1024 * 1024, records: [locator()] }); t.after(() => h.dispose());
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick();
    h.controller.acceptPlaybackMetadata(locator('early.mod', '', 'early.mod'), metadata('early playback'), new Uint8Array([9]));
    await h.workers[0].run(); await tick(); assert.equal(h.workers[0].terminated, true); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 1);
    h.digestPending.resolve(new Uint8Array(32).fill(4).buffer); await tick(); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 0);
    assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 0); assert.equal(h.context.library.rows[0].metadataState, 'ready');
    h.controller.acceptPlaybackMetadata(locator(), metadata('post recycle'), new Uint8Array([1])); await tick(); assert.equal(h.context.library.rows[0].title, 'post recycle');
});

function wasmUnreachable() {
    const module = new WebAssembly.Module(new Uint8Array([
        0, 97, 115, 109, 1, 0, 0, 0, 1, 4, 1, 96, 0, 0, 3, 2, 1, 0,
        7, 8, 1, 4, 116, 114, 97, 112, 0, 0, 10, 5, 1, 3, 0, 0, 11,
    ]));
    new WebAssembly.Instance(module).exports.trap();
}

for (const stage of ['constructor', 'extract_member', 'inspect_s3m', 'module_metadata', 'free']) test(`actual WASM trap in ${stage} retires the worker and preserves delivered progress`, async t => {
    const h = harness(), Base = h.binding.loader.ArchiveInspectionSession; t.after(() => h.dispose());
    let currentMember = '';
    h.binding.loader.ArchiveInspectionSession = class extends Base {
        constructor(bytes) { super(bytes); if (stage === 'constructor') wasmUnreachable(); }
        extract_member(...args) { currentMember = args[1]; if (stage === 'extract_member' && currentMember === 'b.mod') wasmUnreachable(); return super.extract_member(...args); }
        free() {
            super.free();
            if (stage === 'free') wasmUnreachable();
            throw new Error('recursive use of an object detected which would lead to unsafe aliasing in rust');
        }
    };
    for (const binding of ['inspect_s3m', 'module_metadata']) {
        const original = h.binding.loader[binding];
        h.binding.loader[binding] = (...args) => { if (stage === binding && currentMember === 'b.mod') wasmUnreachable(); return original(...args); };
    }
    const delivered = [], job = h.enqueue(group(['a', 'b', 'c'].map(token => member(token, locator(`${token}.mod`)))), { onMember: result => delivered.push(result.token) });
    await tick(); await h.workers[0].run();
    const expected = stage === 'constructor' ? [] : stage === 'free' ? ['a', 'b', 'c'] : ['a'];
    await assert.rejects(job.result, error => { assert.equal(error.code, 'worker-failed'); assert.equal(error.cause, 'wasm-trap'); assert.deepEqual(error.progress.completedTokens, expected); return true; });
    assert.deepEqual(delivered, expected); assert.equal(h.workers[0].terminated, true);
    assert.equal(h.binding.counts.frees, stage === 'free' ? 1 : 0);
});

test('invalid group descriptors reject before reading or constructing a worker', async t => {
    const h = harness(); t.after(() => h.dispose());
    for (const request of [{ ...group(), members: [] }, { ...group(), members: Array.from({ length: 65 }, (_, index) => member(String(index))) },
        group([member('a'), member('a')]), group([member('a'), member('b', locator('b.mod', '', 'other.zip'))]),
        group([member('a', locator('', '', 'outer.zip')), member('b')])]) await assert.rejects(h.enqueue(request).result, error => error.code === 'job-failed' && error.progress.completedTokens.length === 0);
    assert.equal(h.workers.length, 0); assert.equal(h.metrics.length, 0);
});

test('explicit runtime Retry never resubmits delivered callback failures from the same scan pass', async t => {
    let first = true; const previous = console.error; console.error = () => {}; t.after(() => { console.error = previous; });
    const h = harness({ records: ['a', 'b', 'c'].map(name => locator(`${name}.mod`)), wrapCallbacks(callbacks) { return { ...callbacks, onMember(result) {
        if (first) { first = false; throw new Error('before mutation'); } callbacks.onMember(result);
    } }; } }); t.after(() => h.dispose()); h.controller.start(); await tick(); h.controller.scanMetadata(); await tick();
    const worker = h.workers[0], sent = worker.jobs[0]; worker.frame(sent.members[0].token);
    worker.emit({ type: 'error', ...tags(sent), error: { code: 'worker-failed', message: 'trap after callback failure', cause: 'wasm-trap' } }); await tick();
    h.controller.retry(); await tick(); assert.deepEqual(h.workers[1].jobs[0].members.map(item => item.locator.memberPath), ['b.mod', 'c.mod']);
    await h.workers[1].run(); await tick(); assert.equal(h.context.library.rows[0].metadataState, 'unknown'); assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 1);
    assert.equal(h.context.discoveryStatus.issueCount, 1); h.controller.scanMetadata(); await tick(); assert.deepEqual(h.workers[1].jobs[1].members.map(item => item.locator.memberPath), ['a.mod']);
});

for (const failure of ['read-failed', 'source-too-large']) test(`outer ${failure} maps only undelivered eligible members to errors with no fabricated callbacks`, async t => {
    const h = harness({ size: failure === 'source-too-large' ? 128 * 1024 * 1024 + 1 : 4,
        read() { throw new Error('outer unreadable'); }, records: ['a', 'b', 'c'].map(name => locator(`${name}.mod`, 'inner.zip')) }); t.after(() => h.dispose());
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick();
    assert.deepEqual(h.context.library.rows.map(row => row.metadataState), ['error', 'error', 'error']); assert.equal(h.context.discoveryStatus.metadataCounts.error, 3);
    assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 0); assert.equal(h.context.discoveryStatus.runtimeError, ''); assert.equal(h.binding.counts.sessions, 0);
    assert.equal(h.workers[0].jobs.length, 0); assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
});

test('group ordering sorts sources, containers and locator names deterministically; loose modules stand alone', async t => {
    const h = harness({ records: [locator('z.mod', 'inner.zip', 'a.zip'), locator('b.mod', '', 'b.zip'), locator('a.mod', 'inner.zip', 'a.zip'),
        locator('z.mod', '', 'a.zip'), locator('a.mod', '', 'a.zip'), locator('', '', 'a.mod')] }); t.after(() => h.dispose());
    h.controller.start(); await tick(); h.controller.scanMetadata(); await tick();
    assert.equal(h.workers[0].jobs[0].members.length, 1); assert.equal(h.workers[0].jobs[0].members[0].locator.sourcePath, 'a.mod');
    await h.workers[0].run(); await tick();
    assert.deepEqual(h.workers[0].jobs[1].members.map(item => [item.locator.containerMember, item.locator.memberPath]), [['', 'a.mod'], ['', 'z.mod'], ['inner.zip', 'a.mod'], ['inner.zip', 'z.mod']]);
    await h.workers[0].run(); await tick(); assert.equal(h.workers[0].jobs[2].members[0].locator.sourcePath, 'b.zip'); await h.workers[0].run(); await tick();
    assert.equal(h.context.discoveryStatus.metadataCounts.ready, 6); assert.equal(h.binding.counts.sessions, 2); assert.equal(h.binding.counts.frees, 2);
});
