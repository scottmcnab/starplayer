import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createDiscoveryController } from '../www/library-discovery.js';
import { createDiscoveredLibrary, search } from '../www/library-index.js';

const sha = 'a'.repeat(64), secondSha = 'b'.repeat(64);
const metadata = title => ({ format: 'mod', title, channels: 4, instrumentNames: 'inline names', sampleNames: '', songMessage: '' });
const file = (path, size = 12, timestamp = '') => ({ kind: 'file', path, size, timestamp });
const locator = sourcePath => ({ sourcePath, containerMember: '', memberPath: '', rawHex: '' });
const entry = (memberPath, extra = {}) => ({ containerMember: '', memberPath, rawHex: '61', size: 9, format: 'mod', playable: true, error: '', ...extra });
const failure = code => Object.assign(new Error(`failure: ${code}`), { code, name: ['aborted', 'disposed'].includes(code) ? 'AbortError' : 'Error' });
const deferred = () => { let resolve, reject; const promise = new Promise((done, fail) => { resolve = done; reject = fail; }); return { promise, resolve, reject }; };
const tick = async () => { for (let index = 0; index < 20; index += 1) await Promise.resolve(); };
async function until(predicate) {
    for (let index = 0; index < 100; index += 1) { if (predicate()) return; await new Promise(resolve => setTimeout(resolve, 1)); }
    assert.fail('discovery did not reach expected state');
}
function stream() {
    const queue = [], waiting = [];
    let calls = 0, returned = false;
    return {
        push(record) { const result = { value: record, done: false }; if (waiting.length) waiting.shift()(result); else queue.push(result); },
        end() { const result = { done: true }; if (waiting.length) waiting.shift()(result); else queue.push(result); },
        get calls() { return calls; }, get returned() { return returned; },
        [Symbol.asyncIterator]() { return this; },
        next() { calls += 1; return queue.length ? Promise.resolve(queue.shift()) : new Promise(resolve => waiting.push(resolve)); },
        return() { returned = true; for (const done of waiting.splice(0)) done({ done: true }); return Promise.resolve({ done: true }); },
    };
}
function harness({ records = [], source, autoStart = true, digest, nativeDigest = false, onDigestMetric } = {}) {
    const runtimes = [], notifications = [], timers = new Map(), digests = [], metrics = [];
    digest ||= bytes => {
        const snapshot = bytes instanceof ArrayBuffer ? new Uint8Array(bytes).slice() : new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength).slice();
        const pending = deferred();
        digests.push({ snapshot, reject: pending.reject, resolveRaw: pending.resolve,
            resolve(sha256 = createHash('sha256').update(snapshot).digest('hex')) { pending.resolve(Uint8Array.from(sha256.match(/../g), pair => parseInt(pair, 16)).buffer); } });
        return pending.promise;
    };
    let timerId = 0, reads = 0;
    source ||= { kind: 'directory', label: 'root', async *enumerate() { yield* records; }, readFile() { reads += 1; throw new Error('no content read allowed'); } };
    const context = { source, identity: { sha256: 'f'.repeat(64), name: 'root' }, library: createDiscoveredLibrary({ name: 'root' }) };
    const runtimeFactory = () => {
        const jobs = [];
        const runtime = { jobs, paused: false, disposed: false,
            enqueue(request) {
                const pending = deferred();
                const job = { ...request, ...(request.bytes ? { bytes: new Uint8Array(request.bytes).slice() } : {}),
                    result: pending.promise, id: jobs.length + 1, resolve(payload, generation = request.generation) { pending.resolve({ generation, kind: request.kind, payload }); },
                    reject: pending.reject, cancel() { pending.reject(failure('aborted')); } };
                jobs.push(job); return job;
            },
            pauseCatalog() { runtime.paused = true; }, resumeCatalog() { runtime.paused = false; },
            cancelMetadata() { for (const job of jobs.filter(job => job.kind === 'inspect')) job.cancel(); },
            dispose() { runtime.disposed = true; for (const job of jobs) job.reject(failure('disposed')); },
        };
        runtimes.push(runtime); return runtime;
    };
    const controller = createDiscoveryController({ context, generation: 17, runtimeFactory, ...(!nativeDigest ? { digest } : {}), onDigestMetric: onDigestMetric || (metric => metrics.push(metric)),
        onUpdate(context, change) { notifications.push({ context, change }); },
        schedule(callback, delay) { assert.equal(delay, 100); timers.set(++timerId, callback); return timerId; }, unschedule(id) { timers.delete(id); } });
    if (autoStart) controller.start();
    return { context, controller, runtimes, notifications, timers, digests, metrics, get reads() { return reads; },
        jobs(kind, runtime = runtimes.at(-1)) { return runtime.jobs.filter(job => job.kind === kind); },
        publish() { for (const [id, callback] of [...timers]) { timers.delete(id); callback(); } } };
}

test('loose rows publish before a slow ZIP, preserve schemas, and read no contents', async t => {
    const harnessState = harness({ records: [file('album.ZIP'), file('one.MOD', 42, '2020-01-01T00:00:00.000Z'), file('two.s3m'), file('three.mtm'), file('four.xm'), file('five.it'), file('readme.txt')] });
    const { context, controller, jobs } = harnessState; t.after(() => controller.dispose());
    await until(() => context.discoveryStatus.filesExamined === 7);
    assert.equal(context.library.rows.length, 5);
    assert.equal(harnessState.reads, 0);
    assert.equal(context.discoveryStatus.phase, 'running');
    assert.equal(context.discoveryStatus.archivesPending, 1);
    assert.equal(context.discoveryStatus.filesExamined, 7);
    const row = context.library.rows[0];
    assert.equal(row.title, ''); assert.equal(row.channels, null); assert.equal(row.sha256, ''); assert.equal(row.metadataState, 'unknown');
    assert.equal(row.format, 'mod'); assert.equal(row.size, 42); assert.equal(row.timestamp, '2020-01-01T00:00:00.000Z');
    assert.deepEqual(context.discoveryStatus.metadataCounts, { ready: 0, unknown: 5, error: 0, excluded: 0, hashesPending: 0, refreshPending: 0, scanActionable: 5 });
    harnessState.publish(); assert.equal(harnessState.notifications.length, 1);
    jobs('catalog')[0].resolve({ entries: [entry('inner/song.mod', { containerMember: 'child.zip' }), entry('locked.mod', { playable: false, error: 'encrypted' })], issues: [{ path: 'bad.zip', reason: 'corrupt' }] });
    await tick();
    assert.equal(context.discoveryStatus.phase, 'complete'); assert.equal(context.library.rows.length, 7);
    assert.equal(context.library.rows[5].timestamp, ''); assert.equal(context.library.rows[5].size, 9); assert.equal(context.library.rows[5].rawHex, '61');
    assert.equal(context.library.rows[6].playable, false);
    assert.deepEqual(context.discoveryStatus.issues, [{ id: 0, path: 'album.ZIP/bad.zip', reason: 'corrupt' }]);
    assert.equal(harnessState.notifications.length, 2, 'completion publishes immediately');
    assert.ok(Object.isFrozen(context.discoveryStatus)); assert.ok(Object.isFrozen(context.discoveryStatus.issues));
});

test('empty folders complete distinctly from running folders; notifications batch for 100ms', async t => {
    const incoming = stream(), h = harness({ source: { label: 'empty', enumerate: () => incoming } }); t.after(() => h.controller.dispose());
    await tick(); assert.equal(h.context.discoveryStatus.phase, 'running'); assert.equal(h.notifications.length, 0);
    for (let index = 0; index < 20; index += 1) incoming.push(file(`song${index}.mod`));
    await tick(); await until(() => h.context.library.rows.length === 20);
    assert.equal(h.notifications.length, 0); assert.equal(h.timers.size, 1);
    h.publish(); assert.equal(h.notifications.length, 1); assert.equal(h.notifications[0].change.changedRowIds.length, 20);
    incoming.end(); await tick(); assert.equal(h.context.discoveryStatus.phase, 'complete'); assert.equal(h.notifications.length, 2);
    const empty = harness(); t.after(() => empty.controller.dispose()); await tick();
    assert.equal(empty.context.discoveryStatus.phase, 'complete'); assert.equal(empty.context.discoveryStatus.tracksFound, 0);
});

test('Stop holds an outstanding next result and resumes without duplicates; inspect work continues', async t => {
    const incoming = stream(), h = harness({ source: { label: 'root', enumerate: () => incoming } }); t.after(() => h.controller.dispose());
    incoming.push(file('known.mod')); incoming.push(file('archive.zip')); await until(() => incoming.calls === 3);
    h.controller.scanMetadata(); h.controller.pause();
    incoming.push(file('held.mod')); await tick();
    assert.equal(h.context.discoveryStatus.phase, 'paused'); assert.equal(h.context.library.rows.length, 1); assert.equal(incoming.calls, 3);
    assert.equal(h.runtimes[0].paused, true); assert.equal(h.jobs('inspect').length, 1);
    h.jobs('inspect')[0].resolve({ metadata: metadata('known'), sha256: sha }); await tick();
    assert.equal(h.context.library.rows[0].title, 'known'); assert.equal(h.context.discoveryStatus.metadataPhase, 'waiting');
    h.controller.resume(); await tick(); assert.equal(h.context.library.rows.length, 2); assert.equal(h.runtimes[0].paused, false);
    h.controller.cancelMetadata(); assert.equal(h.context.discoveryStatus.metadataPhase, 'cancelled');
    assert.equal(h.context.discoveryStatus.archivesPending, 1, 'metadata cancellation leaves archives alone');
    incoming.end(); h.jobs('catalog')[0].resolve({ entries: [], issues: [] }); await tick();
    assert.equal(h.context.discoveryStatus.phase, 'complete'); assert.equal(h.context.library.rows.length, 2);
});

test('root retry deduplicates rows and completed archives while retaining issues', async t => {
    let walks = 0;
    const h = harness({ source: { label: 'root', async *enumerate() { walks += 1; yield file('a.mod'); yield file('a.zip'); if (walks === 1) yield { kind: 'issue', path: '', reason: 'permission denied' }; else yield file('b.mod'); } } });
    t.after(() => h.controller.dispose()); await tick();
    h.jobs('catalog')[0].resolve({ entries: [entry('member.mod')], issues: [] }); await tick();
    assert.equal(h.context.discoveryStatus.phase, 'failed'); assert.equal(h.context.library.rows.length, 2);
    h.controller.retry(); await tick();
    assert.equal(h.context.discoveryStatus.phase, 'complete'); assert.equal(h.context.library.rows.length, 3);
    assert.equal(h.context.discoveryStatus.filesExamined, 3); assert.equal(h.jobs('catalog').length, 1); assert.equal(h.context.discoveryStatus.issueCount, 1);
});

test('worker failure allows loose walking and fails only when unfinished archives remain', async t => {
    for (const archive of [true, false]) {
        const incoming = stream(), h = harness({ source: { label: 'root', enumerate: () => incoming } }); t.after(() => h.controller.dispose());
        incoming.push(file(archive ? 'pending.zip' : 'known.mod')); await tick();
        if (!archive) h.controller.scanMetadata();
        h.jobs(archive ? 'catalog' : 'inspect')[0].reject(failure('worker-failed')); await tick();
        assert.equal(h.context.discoveryStatus.phase, 'running'); assert.match(h.context.discoveryStatus.runtimeError, /worker-failed/);
        incoming.push(file('later.mod')); incoming.end(); await tick();
        assert.equal(h.context.library.rows.at(-1).sourcePath, 'later.mod');
        assert.equal(h.context.discoveryStatus.phase, archive ? 'failed' : 'complete');
        if (!archive) { assert.equal(h.context.discoveryStatus.metadataPhase, 'suspended'); assert.equal(h.context.library.rows[0].metadataState, 'unknown'); }
    }
});

test('paused worker retry preserves pause and resubmits each unfinished job once', async t => {
    const h = harness({ records: [file('pending.zip'), file('a.mod'), file('b.mod')] }); t.after(() => h.controller.dispose()); await tick();
    h.controller.scanMetadata(); h.controller.pause();
    h.jobs('catalog')[0].reject(failure('worker-failed')); await tick();
    h.controller.resume(); assert.equal(h.runtimes[0].paused, true, 'Resume cannot restart failed runtime');
    h.controller.pause(); h.controller.retry(); await tick();
    assert.equal(h.runtimes.length, 2); assert.equal(h.runtimes[0].disposed, true); assert.equal(h.runtimes[1].paused, true);
    assert.equal(h.jobs('catalog').length, 1); assert.equal(h.jobs('inspect').length, 2);
    assert.equal(h.context.discoveryStatus.phase, 'paused'); assert.equal(h.context.discoveryStatus.runtimeError, '');
    assert.equal(h.context.library.rows[0].metadataState, 'unknown', 'old dispose errors are silent');
    h.controller.retry(); assert.equal(h.runtimes.length, 2); assert.equal(h.jobs('catalog').length, 1);
    h.jobs('catalog')[0].resolve({ entries: [], issues: [] });
    for (const job of h.jobs('inspect')) job.resolve({ metadata: metadata('inspected'), sha256: sha });
    await tick(); assert.equal(h.context.discoveryStatus.phase, 'paused'); assert.equal(h.context.discoveryStatus.metadataPhase, 'waiting');
    h.controller.resume(); assert.equal(h.context.discoveryStatus.phase, 'complete'); assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
});

test('metadata scan includes arrivals, waits for discovery, and retries errors only explicitly', async t => {
    const incoming = stream(), h = harness({ source: { label: 'root', enumerate: () => incoming } }); t.after(() => h.controller.dispose());
    incoming.push(file('bad.mod')); await tick(); h.controller.scanMetadata();
    h.jobs('inspect')[0].reject(failure('job-failed')); await tick();
    assert.equal(h.context.library.rows[0].playable, true); assert.equal(h.context.library.rows[0].metadataState, 'error');
    assert.equal(h.context.discoveryStatus.metadataCounts.error, 1); assert.equal(h.context.discoveryStatus.metadataPhase, 'waiting');
    incoming.push(file('late.mod')); await tick(); assert.equal(h.jobs('inspect').length, 2);
    h.jobs('inspect')[1].resolve({ metadata: metadata('late title'), sha256: sha }); await tick();
    h.controller.scanMetadata(); assert.equal(h.jobs('inspect').length, 2, 'new scans do not retry row errors');
    h.controller.retryMetadata(locator('bad.mod')); assert.equal(h.jobs('inspect').length, 3);
    h.jobs('inspect')[2].resolve({ metadata: metadata('recovered'), sha256: sha }); incoming.end(); await tick();
    assert.equal(h.context.discoveryStatus.metadataPhase, 'complete'); assert.equal(h.context.library.rows[0].error, '');
    assert.deepEqual(h.context.discoveryStatus.metadataCounts, { ready: 2, unknown: 0, error: 0, excluded: 0, hashesPending: 0, refreshPending: 0, scanActionable: 0 });
    assert.equal(h.context.discoveryStatus.metadataIncomplete, false);
});

test('scan cancellation ignores disposed/cancelled inspection and leaves archive discovery running', async t => {
    const h = harness({ records: [file('a.mod'), file('pending.zip')] }); t.after(() => h.controller.dispose()); await tick();
    h.controller.scanMetadata(); const old = h.jobs('inspect')[0];
    h.controller.cancelMetadata(); await tick(); assert.equal(h.context.library.rows[0].metadataState, 'unknown');
    h.controller.scanMetadata(); assert.equal(h.jobs('inspect').length, 2);
    old.resolve({ metadata: metadata('stale'), sha256: sha });
    h.jobs('inspect')[1].resolve({ metadata: metadata('fresh'), sha256: sha }); await tick();
    assert.equal(h.context.library.rows[0].title, 'fresh'); assert.equal(h.context.discoveryStatus.metadataPhase, 'waiting');
    h.jobs('catalog')[0].resolve({ entries: [], issues: [] }); await tick(); assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
});

test('playback before row arrival caches only completed coherent patches and joins latest sidecar', async t => {
    const incoming = stream(), h = harness({ source: { label: 'root', enumerate: () => incoming } }); t.after(() => h.controller.dispose());
    const where = { sourcePath: 'outer.zip', containerMember: 'inner.zip', memberPath: 'music.mod', rawHex: '61' };
    const bytes = new Uint8Array([1, 2, 3]);
    h.controller.acceptPlaybackMetadata(where, metadata('played early'), bytes);
    h.publish(); assert.equal(h.context.library.rows.length, 0);
    assert.deepEqual(h.notifications.at(-1).change.metadataUpdates, []);
    assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 1);
    h.controller.attachComments(new Map([[sha, { instrumentNames: 'old sidecar' }]]));
    h.controller.attachComments(new Map([[sha, { instrumentNames: 'new sidecar' }]]));
    h.digests[0].resolve(sha); await tick(); h.publish();
    assert.deepEqual([...bytes], [1, 2, 3]); assert.equal(bytes.byteLength, 3);
    assert.equal(h.notifications.findLast(item => item.change.metadataUpdates.length).change.metadataUpdates[0].patch.sha256, sha);
    incoming.push(file('outer.zip')); incoming.end(); await tick();
    h.jobs('catalog')[0].resolve({ entries: [entry('music.mod', { containerMember: 'inner.zip' })], issues: [] }); await tick();
    const row = h.context.library.rows[0]; assert.equal(row.title, 'played early'); assert.equal(row.metadataState, 'ready'); assert.equal(row.sha256, sha);
    assert.equal(h.context.library.commentsFor(0).instrumentNames, 'new sidecar');
    assert.deepEqual(h.context.discoveryStatus.sidecarCounts, { joined: 1, unmatched: 0, awaitingHashes: 0, failedHashes: 0, excludedHashes: 0 });
    assert.equal(search(h.context.library, 'text:new').length, 1);
    h.controller.attachComments(new Map([[secondSha, { instrumentNames: 'replacement' }]]));
    assert.equal(h.context.library.commentsFor(0).instrumentNames, 'inline names');
    assert.deepEqual(h.context.discoveryStatus.sidecarCounts, { joined: 0, unmatched: 1, awaitingHashes: 0, failedHashes: 0, excludedHashes: 0 });
});

test('A/B/C successful replays retain coherent metadata/SHA/sidecars until each atomic replacement', async t => {
    const h = harness({ records: ['a.mod', 'b.mod', 'c.mod'].map(path => file(path)) }); t.after(() => h.controller.dispose()); await tick();
    const names = ['a.mod', 'b.mod', 'c.mod'];
    h.controller.scanMetadata();
    for (const [index, job] of h.jobs('inspect').entries()) job.resolve({ metadata: metadata(`seed ${index}`), sha256: sha });
    await tick(); h.publish();
    h.controller.attachComments(new Map([[sha, { instrumentNames: 'seed sidecar' }]])); h.publish();
    const mutable = new Uint8Array([8, 0, 1, 255, 9]), view = mutable.subarray(1, 4);
    const where = locator('a.mod'), details = metadata('played A');
    h.controller.acceptPlaybackMetadata(where, details, view);
    const firstDigest = createHash('sha256').update(view).digest('hex');
    mutable.fill(44); details.title = 'mutated'; where.sourcePath = 'wrong.mod';
    structuredClone(mutable.buffer, { transfer: [mutable.buffer] });
    h.controller.acceptPlaybackMetadata(locator('b.mod'), metadata('played B'), new Uint8Array([2]));
    h.controller.acceptPlaybackMetadata(locator('c.mod'), metadata('played C'), new Uint8Array([3]));
    assert.equal(h.jobs('hash').length, 0);
    assert.deepEqual(h.context.library.rows.map(row => [row.title, row.sha256]), names.map((_, index) => [`seed ${index}`, sha]));
    assert.equal(h.context.discoveryStatus.sidecarCounts.joined, 3);
    assert.equal(h.context.discoveryStatus.sidecarCounts.awaitingHashes, 0);
    assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 3);
    assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 3);
    assert.equal(h.context.discoveryStatus.metadataIncomplete, true);
    h.publish(); assert.deepEqual(h.notifications.at(-1).change.rowChanges, []);
    h.digests[2].resolve(); h.digests[0].resolve(); await tick();
    assert.equal(h.context.library.rows[0].title, 'played A'); assert.equal(h.context.library.rows[0].sha256, firstDigest);
    assert.equal(h.context.library.rows[1].title, 'seed 1'); assert.equal(h.context.library.rows[1].sha256, sha);
    h.digests[1].resolve(); await tick();
    assert.deepEqual(h.context.library.rows.map(row => row.title), ['played A', 'played B', 'played C']);
    assert.ok(h.context.library.rows.every(row => /^[a-f0-9]{64}$/.test(row.sha256)));
    assert.equal(h.context.discoveryStatus.metadataIncomplete, false);
    assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 0);
    assert.equal(h.metrics.filter(metric => metric.event === 'start').at(-1).pendingInputBytes, 5);
    assert.equal(h.metrics.filter(metric => metric.event === 'settle').at(-1).pendingInputBytes, 0);
    assert.ok(h.metrics.every(metric => Object.values(metric).every(value => ['number', 'string'].includes(typeof value))));
});

test('first play stays unknown; playback invalidates older inspection, then publishes complete tuple', async t => {
    const h = harness({ records: [file('a.mod')] }); t.after(() => h.controller.dispose()); await tick();
    h.controller.scanMetadata(); const stale = h.jobs('inspect')[0];
    h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('playback'), new Uint8Array([1]));
    stale.resolve({ metadata: metadata('stale'), sha256: sha }); await tick();
    assert.equal(h.context.library.rows[0].title, ''); assert.equal(h.context.library.rows[0].metadataState, 'unknown');
    assert.equal(h.context.library.rows[0].sha256, ''); assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 0);
    h.digests[0].resolve('00'.repeat(31) + '0f'); await tick();
    assert.equal(h.context.library.rows[0].title, 'playback'); assert.equal(h.context.library.rows[0].sha256, '00'.repeat(31) + '0f');
    assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
});

test('latest version wins while superseded success/failure promises remain physically counted', async t => {
    const h = harness({ records: [file('a.mod')] }); t.after(() => h.controller.dispose()); await tick();
    for (const title of ['stale success', 'stale failure', 'latest']) h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata(title), new Uint8Array([title.length]));
    assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 3); assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 1);
    h.digests[2].resolve(sha); await tick();
    assert.equal(h.context.library.rows[0].title, 'latest'); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 2);
    assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 0); assert.equal(h.context.discoveryStatus.metadataIncomplete, false);
    h.digests[1].reject(new Error('stale failure')); h.digests[0].resolve(secondSha); await tick();
    assert.equal(h.context.library.rows[0].title, 'latest'); assert.equal(h.context.library.rows[0].sha256, sha);
    assert.equal(h.context.discoveryStatus.issueCount, 0); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 0);
    assert.equal(h.metrics.filter(metric => metric.event === 'settle').length, 3);
});

test('stale-only physical digest does not block Scan for latest failed refresh', async t => {
    const h = harness({ records: [file('a.mod')] }); t.after(() => h.controller.dispose()); await tick();
    h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('stale'), new Uint8Array([1]));
    h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('latest'), new Uint8Array([2]));
    h.digests[1].reject(new Error('digest failed')); await tick();
    assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 1); assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 1);
    h.controller.scanMetadata(); assert.equal(h.jobs('inspect').length, 1);
    h.jobs('inspect')[0].resolve({ metadata: metadata('scanned'), sha256: sha }); await tick();
    assert.equal(h.context.discoveryStatus.metadataIncomplete, false);
    h.digests[0].reject(new Error('obsolete')); await tick();
    assert.equal(h.context.library.rows[0].title, 'scanned'); assert.equal(h.context.discoveryStatus.issueCount, 1);
});

test('sync throw, async rejection and malformed digest share failure patch and joined issue path', async t => {
    for (const mode of ['throw', 'reject', 'short', 'view']) {
        let fail = false;
        const h = harness({ records: [file('outer.zip')], digest: () => {
            if (!fail) return Promise.resolve(new Uint8Array(32).fill(0xaa).buffer);
            if (mode === 'throw') throw new Error('digest failed');
            if (mode === 'reject') return Promise.reject(new Error('digest failed'));
            return Promise.resolve(mode === 'short' ? new ArrayBuffer(31) : new Uint8Array(32));
        } }); t.after(() => h.controller.dispose()); await tick();
        const where = { sourcePath: 'outer.zip', containerMember: 'inner.zip', memberPath: 'song.mod' };
        h.controller.acceptPlaybackMetadata(where, metadata('seed'), new Uint8Array([0])); await tick();
        h.jobs('catalog')[0].resolve({ entries: [entry('song.mod', { containerMember: 'inner.zip' })], issues: [] }); await tick();
        h.controller.attachComments(new Map([[sha, { instrumentNames: 'sidecar' }]]));
        fail = true; const details = metadata('failed refresh');
        h.controller.acceptPlaybackMetadata(where, details, new Uint8Array([1])); details.title = 'mutated'; await tick();
        assert.equal(h.context.library.rows[0].sha256, ''); assert.equal(h.context.library.rows[0].title, 'failed refresh');
        assert.equal(h.context.library.rows[0].metadataState, 'ready'); assert.equal(h.context.library.rows[0].playable, true);
        assert.equal(h.context.library.commentsFor(0).instrumentNames, 'inline names');
        assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 0); assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 0);
        assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 1);
        assert.equal(h.context.discoveryStatus.issues[0].path, 'outer.zip/inner.zip/song.mod');
        assert.equal(h.context.discoveryStatus.sidecarCounts.awaitingHashes, 1);
        h.controller.scanMetadata(); assert.equal(h.jobs('inspect').length, 1);
    }
});

test('page digest survives worker trap, explicit Retry, scan cancellation and subsequent worker work', async t => {
    const h = harness({ records: [file('a.mod'), file('pending.zip')] }); t.after(() => h.controller.dispose()); await tick();
    h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('played'), new Uint8Array([1]));
    h.controller.scanMetadata(); h.controller.cancelMetadata();
    h.jobs('catalog')[0].reject(failure('worker-failed')); await tick();
    assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 1); assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 1);
    h.controller.retry(); assert.equal(h.jobs('hash').length, 0); assert.equal(h.jobs('catalog').length, 1);
    h.jobs('catalog')[0].resolve({ entries: [], issues: [] }); await tick();
    h.digests[0].resolve(sha); await tick();
    assert.equal(h.context.library.rows[0].title, 'played'); assert.equal(h.context.library.rows[0].sha256, sha);
    assert.equal(h.context.discoveryStatus.metadataIncomplete, false);
    // Successful worker recycling is internal to the runtime and never changes controller epochs.
    h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('after recycle'), new Uint8Array([2]));
    h.runtimes.at(-1).enqueue({ generation: 17, kind: 'catalog', sourcePath: 'recycled.zip' }).resolve({ entries: [], issues: [] });
    h.digests[1].resolve(secondSha); await tick(); assert.equal(h.context.library.rows[0].sha256, secondSha);
});

test('before-row latest versions transition to excluded without double-counting; valid SHA still joins', async t => {
    const h = harness({ records: [file('outer.zip')] }); t.after(() => h.controller.dispose()); await tick();
    const where = { sourcePath: 'outer.zip', memberPath: 'locked.mod', containerMember: '' };
    h.controller.attachComments(new Map([[sha, { instrumentNames: 'sidecar' }]]));
    h.controller.acceptPlaybackMetadata(where, metadata('old'), new Uint8Array([1]));
    h.controller.acceptPlaybackMetadata(where, metadata('latest'), new Uint8Array([2]));
    assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 2); assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 1);
    h.jobs('catalog')[0].resolve({ entries: [entry('locked.mod', { playable: false, error: 'encrypted member' })], issues: [] }); await tick();
    assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 0); assert.equal(h.context.discoveryStatus.metadataIncomplete, false);
    h.digests[1].resolve(sha); h.digests[0].reject(new Error('stale')); await tick();
    assert.equal(h.context.library.rows[0].playable, false); assert.equal(h.context.library.rows[0].error, 'encrypted member');
    assert.equal(h.context.library.rows[0].title, 'latest'); assert.equal(h.context.discoveryStatus.sidecarCounts.joined, 1);
    assert.deepEqual(h.context.discoveryStatus.metadataCounts, { ready: 0, unknown: 0, error: 0, excluded: 1, hashesPending: 0, refreshPending: 0, scanActionable: 0 });
});

test('eligible counters and useful Scan exclude encrypted rows and metadata errors', async t => {
    for (const eligible of [0, 1, 2]) {
        const h = harness({ records: [file('outer.zip')] }); t.after(() => h.controller.dispose()); await tick();
        h.jobs('catalog')[0].resolve({ entries: [entry('locked.mod', { playable: false, error: 'encrypted' }), ...Array.from({ length: eligible }, (_, index) => entry(`${index}.mod`))], issues: [] }); await tick();
        h.controller.attachComments(new Map([[sha, { instrumentNames: 'sidecar' }]]));
        assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, eligible);
        assert.equal(h.context.discoveryStatus.metadataIncomplete, eligible > 0);
        h.controller.scanMetadata(); h.controller.scanMetadata(); assert.equal(h.jobs('inspect').length, eligible);
        assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 0);
        for (const [index, job] of h.jobs('inspect').entries()) {
            if (!index) job.reject(failure('job-failed')); else job.resolve({ metadata: metadata('ready'), sha256: sha });
        }
        await tick(); h.controller.scanMetadata(); assert.equal(h.jobs('inspect').length, eligible);
        assert.equal(h.context.discoveryStatus.metadataPhase, 'complete');
        assert.equal(h.context.discoveryStatus.metadataIncomplete, eligible > 0);
        assert.deepEqual(h.context.discoveryStatus.sidecarCounts, { joined: eligible > 1 ? 1 : 0, unmatched: eligible > 1 ? 0 : 1, awaitingHashes: 0, failedHashes: eligible ? 1 : 0, excludedHashes: 1 });
        if (eligible) { h.controller.retryMetadata({ sourcePath: 'outer.zip', memberPath: '0.mod' }); assert.equal(h.jobs('inspect').length, eligible + 1); }
    }
});

test('catalog and inspection errors retain distinct paths and row playability', async t => {
    const h = harness({ records: [file('bad.zip'), file('known.mod')] }); t.after(() => h.controller.dispose()); await tick();
    h.jobs('catalog')[0].reject(failure('read-failed')); h.controller.scanMetadata(); h.jobs('inspect')[0].reject(failure('source-too-large')); await tick();
    assert.equal(h.context.discoveryStatus.phase, 'complete'); assert.equal(h.context.library.rows[0].playable, true);
    assert.equal(h.context.library.rows[0].metadataState, 'error');
    assert.deepEqual(h.context.discoveryStatus.issues.map(issue => issue.path), ['bad.zip']);
});

test('worker failure after complete discovery remains visible and retry resumes metadata', async t => {
    const h = harness({ records: [file('known.mod')] }); t.after(() => h.controller.dispose()); await tick();
    h.controller.scanMetadata(); h.jobs('inspect')[0].reject(failure('worker-failed')); await tick();
    assert.equal(h.context.discoveryStatus.phase, 'complete'); assert.equal(h.context.discoveryStatus.metadataPhase, 'suspended');
    assert.match(h.context.discoveryStatus.runtimeError, /worker-failed/); assert.equal(h.context.library.rows[0].error, '');
    h.controller.retry(); assert.equal(h.jobs('inspect').length, 1);
    h.jobs('inspect')[0].resolve({ metadata: metadata('retry'), sha256: sha }); await tick();
    assert.equal(h.context.discoveryStatus.metadataPhase, 'complete'); assert.equal(h.context.discoveryStatus.runtimeError, '');
});

test('generation mismatch and disposal suppress updates while outstanding digests still settle', async () => {
    const incoming = stream(), h = harness({ source: { label: 'root', enumerate: () => incoming } });
    incoming.push(file('pending.zip')); incoming.push(file('a.mod')); await tick();
    h.controller.scanMetadata(); h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('current'), new Uint8Array([1]));
    h.controller.acceptPlaybackMetadata(locator('before.mod'), metadata('before'), new Uint8Array([1, 2]));
    h.jobs('catalog')[0].resolve({ entries: [entry('stale.mod')], issues: [] }, 999); await tick();
    assert.equal(h.context.library.rows.length, 1); assert.equal(h.context.library.rows[0].sha256, '');
    incoming.return = () => { throw new Error('cleanup failed'); };
    h.controller.dispose(); const before = h.notifications.length;
    incoming.push(file('stale.mod')); h.digests[0].resolve(sha); h.digests[1].reject(new Error('disposed failure')); await tick(); h.publish();
    assert.equal(h.notifications.length, before); assert.equal(h.context.library.rows.length, 1); assert.equal(h.runtimes[0].disposed, true);
    assert.equal(h.context.library.rows[0].error, ''); assert.equal(h.context.library.rows[0].sha256, '');
    assert.equal(h.context.discoveryStatus.issueCount, 0);
    assert.equal(h.metrics.at(-1).pending, 0); assert.equal(h.metrics.at(-1).pendingInputBytes, 0);
});

test('controller coalesces typed deltas and revisions while status updates carry no row work', async t => {
    const incoming = stream(), h = harness({ source: { label: 'root', enumerate: () => incoming } }); t.after(() => h.controller.dispose());
    incoming.push(file('a.mod')); await tick(); h.publish();
    assert.deepEqual(h.notifications.at(-1).change.rowChanges, [{ id: 0, added: true, fields: ['*'] }]);
    assert.equal(h.notifications.at(-1).change.libraryRevision, 1);
    h.controller.pause(); h.publish(); assert.deepEqual(h.notifications.at(-1).change.rowChanges, []);
    h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('ready'), new Uint8Array([1])); h.digests[0].resolve(sha); await tick(); h.publish();
    const patch = h.notifications.findLast(item => item.change.rowChanges.length).change;
    assert.deepEqual(new Set(patch.rowChanges[0].fields), new Set(['title', 'channels', 'metadataState', 'sha256', 'comments']));
    assert.equal(patch.rowChanges[0].added, false); assert.equal(patch.libraryRevision, 2);
    h.controller.attachComments(new Map([[sha, { instrumentNames: 'replacement' }]])); h.publish();
    assert.deepEqual(h.notifications.at(-1).change.rowChanges, [{ id: 0, added: false, fields: ['comments'] }]);
    h.controller.resume(); incoming.push({ kind: 'issue', path: 'a', reason: 'bad' }); incoming.push({ kind: 'issue', path: 'a', reason: 'bad' });
    incoming.push({ kind: 'issue', path: 'b', reason: 'bad' }); incoming.end(); await tick(); h.publish();
    assert.deepEqual(h.context.discoveryStatus.issues.map(issue => issue.id), [0, 1]);
    assert.equal(h.notifications.at(-1).change.issuesRevision, 2);
    assert.deepEqual(h.notifications.at(-1).change.rowChanges, []);
});


test('falsy sync and async failures clear a seeded SHA and create issues; stale falsy failures stay silent', async t => {
    for (const mode of ['sync', 'async']) {
        let invoked = 0;
        const stale = deferred();
        const h = harness({ records: [file('a.mod')], digest: () => {
            invoked += 1;
            if (invoked === 1) return Promise.resolve(new Uint8Array(32).fill(0xaa).buffer);
            if (invoked === 2) return stale.promise;
            if (mode === 'sync') throw undefined;
            return Promise.reject(null);
        } }); t.after(() => h.controller.dispose()); await tick();
        h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('seed'), new Uint8Array([1])); await tick();
        assert.equal(h.context.library.rows[0].sha256, sha);
        h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('stale'), new Uint8Array([2]));
        h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('failure'), new Uint8Array([3])); await tick();
        assert.equal(h.context.library.rows[0].title, 'failure'); assert.equal(h.context.library.rows[0].sha256, '');
        assert.equal(h.context.discoveryStatus.issueCount, 1); assert.equal(h.context.discoveryStatus.issues[0].path, 'a.mod');
        assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 0); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 1);
        stale.reject(false); await tick();
        assert.equal(h.context.discoveryStatus.issueCount, 1); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 0);
    }
});

test('native default digest snapshots a passed byte range before mutation and detachment', async t => {
    const h = harness({ records: [file('a.mod')], nativeDigest: true }); t.after(() => h.controller.dispose()); await tick();
    const bytes = new Uint8Array([99, 0, 1, 2, 88]), view = bytes.subarray(1, 4);
    const expected = createHash('sha256').update(view).digest('hex');
    h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('native'), view);
    bytes.fill(0xff); structuredClone(bytes.buffer, { transfer: [bytes.buffer] });
    await until(() => h.context.discoveryStatus.metadataCounts.hashesPending === 0);
    assert.equal(h.context.library.rows[0].sha256, expected); assert.equal(h.context.discoveryStatus.metadataIncomplete, false);
});

test('scalar status and digest settlement never walk existing library rows', async t => {
    const h = harness({ records: Array.from({ length: 100 }, (_, index) => file(`${index}.mod`)) }); t.after(() => h.controller.dispose());
    await until(() => h.context.discoveryStatus.phase === 'complete');
    const rows = h.context.library.rows;
    h.context.library.rows = new Proxy(rows, { get(target, key, receiver) {
        assert.notEqual(key, Symbol.iterator, 'status must not walk existing rows');
        if (typeof key === 'string') assert.ok(!['map', 'forEach', 'filter', 'reduce', 'slice'].includes(key));
        return Reflect.get(target, key, receiver);
    } });
    h.controller.pause(); h.publish(); h.controller.resume();
    h.controller.acceptPlaybackMetadata(locator('0.mod'), metadata('played'), new Uint8Array([1])); h.digests[0].resolve(sha); await tick(); h.publish();
    assert.equal(h.context.discoveryStatus.metadataCounts.scanActionable, 99);
    h.context.library.rows = rows;
});


test('test-only metric callback failures cannot suppress digest publication or settlement', async t => {
    const originalError = console.error, logged = [];
    console.error = (...message) => logged.push(message);
    t.after(() => { console.error = originalError; });
    const h = harness({ records: [file('a.mod')], onDigestMetric() { throw new Error('metric hook failed'); } }); t.after(() => h.controller.dispose()); await tick();
    h.controller.acceptPlaybackMetadata(locator('a.mod'), metadata('ready'), new Uint8Array([1]));
    h.digests[0].resolve(sha); await tick();
    assert.equal(h.context.library.rows[0].sha256, sha); assert.equal(h.context.discoveryStatus.metadataCounts.hashesPending, 0);
    assert.equal(h.context.discoveryStatus.metadataCounts.refreshPending, 0); assert.equal(logged.length, 2);
});
