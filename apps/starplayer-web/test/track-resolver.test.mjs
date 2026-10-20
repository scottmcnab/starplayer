import test from 'node:test';
import assert from 'node:assert/strict';
import { createResolver } from '../www/track-resolver.js';

const locator = (sourcePath = 'a.zip', memberPath = 'DOS\\song.mod') => ({ kind: 'library', sourcePath, containerMember: 'inner\\pack.zip', memberPath, rawHex: 'abcd' });
function fixture(options = {}) {
    const reads = [];
    const calls = [];
    const source = { size: async () => 4, readFile: async path => { reads.push(path); return new Uint8Array([1, 2, 3, 4]).buffer; } };
    const loader = { archive_extract_path(bytes, ...names) { calls.push(names); return bytes.subarray(1, 3); } };
    return { reads, calls, source, loader, resolver: createResolver({ source, loader, ...options }) };
}

test('archive LRU evicts by total bytes; cache hits promote; copies stay independent', async () => {
    const { resolver, reads, calls } = fixture({ cacheBytes: 8 });
    const first = await resolver.resolve(locator());
    new Uint8Array(first)[0] = 99;
    assert.deepEqual([...new Uint8Array(await resolver.resolve(locator()))], [2, 3]);
    await resolver.resolve(locator('b.zip'));
    await resolver.resolve(locator());
    await resolver.resolve(locator('c.zip'));
    await resolver.resolve(locator('b.zip'));
    assert.deepEqual(reads, ['a.zip', 'b.zip', 'c.zip', 'b.zip']);
    assert.deepEqual(calls[0], ['inner\\pack.zip', 'DOS\\song.mod', 'abcd']);
    resolver.clear();
    await resolver.resolve(locator());
    assert.equal(reads.length, 5);
});

test('oversize archives bypass cache; max file size is checked before reading', async () => {
    const { resolver, reads } = fixture({ cacheBytes: 3 });
    await resolver.resolve(locator()); await resolver.resolve(locator());
    assert.equal(reads.length, 2);
    const limited = fixture({ maxFileBytes: 3 });
    await assert.rejects(limited.resolver.resolve(locator()), /a.zip.*song.mod.*too large/);
    assert.equal(limited.reads.length, 0);
});

test('unknown size still enforces cap after read, and loose modules return fresh copies', async () => {
    const sourceBytes = new Uint8Array([1, 2]);
    const source = { size: async () => null, readFile: async () => sourceBytes.buffer };
    const resolver = createResolver({ source, loader: {}, maxFileBytes: 2 });
    const first = await resolver.resolve(locator('loose.mod', ''));
    new Uint8Array(first)[0] = 9;
    assert.deepEqual([...new Uint8Array(await resolver.resolve(locator('loose.mod', '')))], [1, 2]);
    await assert.rejects(createResolver({ source, loader: {}, maxFileBytes: 1 }).resolve(locator()), /too large/);
});

test('queue serialises read and extract; abort after read rejects and queue recovers', async () => {
    let release;
    const events = [];
    const controller = new AbortController();
    const source = { size: async () => 1, readFile: async path => {
        events.push(path);
        if (path === 'a.zip') await new Promise(resolve => { release = resolve; });
        return new Uint8Array([1]).buffer;
    } };
    const resolver = createResolver({ source, loader: { archive_extract_path: bytes => { events.push('extract'); return bytes; } } });
    const first = resolver.resolve(locator(), { signal: controller.signal });
    const second = resolver.resolve(locator('b.zip'));
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(events, ['a.zip']);
    controller.abort(); release();
    await assert.rejects(first, { name: 'AbortError' });
    await second;
    assert.deepEqual(events, ['a.zip', 'b.zip', 'extract']);
});

test('prefetch warms archives only, swallows errors; diagnostics retain row and cause', async () => {
    const { resolver, reads, loader } = fixture();
    assert.equal(await resolver.prefetch(locator()), undefined);
    await resolver.resolve(locator());
    await resolver.prefetch(locator('loose.mod', ''));
    assert.deepEqual(reads, ['a.zip']);
    for (const cause of ['archive entry does not exist', 'ambiguous member', 'unsupported method', 'file missing']) {
        loader.archive_extract_path = () => { throw new Error(cause); };
        await assert.rejects(resolver.resolve(locator()), error => error.message.includes('a.zip') && error.message.includes('DOS\\song.mod') && error.message.includes(cause));
        await resolver.prefetch(locator());
    }
    await assert.rejects(resolver.resolve({ kind: 'upload' }), /not supported by this resolver/);
});

test('abort after size or extraction is checked; extraction output is a fresh copy', async () => {
    const controller = new AbortController();
    let reads = 0;
    const source = { size: async () => { controller.abort(); return 1; }, readFile: async () => { reads += 1; return new Uint8Array([1]).buffer; } };
    await assert.rejects(createResolver({ source, loader: {} }).resolve(locator(), { signal: controller.signal }), { name: 'AbortError' });
    assert.equal(reads, 0);
    const extractionController = new AbortController();
    const testFixture = fixture();
    testFixture.loader.archive_extract_path = async bytes => { extractionController.abort(); return bytes; };
    await assert.rejects(testFixture.resolver.resolve(locator(), { signal: extractionController.signal }), { name: 'AbortError' });
    const extractedBytes = new Uint8Array([7]);
    testFixture.loader.archive_extract_path = () => extractedBytes;
    const first = await testFixture.resolver.resolve(locator());
    assert.notEqual(first, extractedBytes.buffer);
    new Uint8Array(first)[0] = 99;
    assert.deepEqual([...new Uint8Array(await testFixture.resolver.resolve(locator()))], [7]);
});

test('prefetch swallows source failures and leaves the queue usable', async () => {
    const { source, resolver } = fixture();
    source.readFile = async () => { throw new Error('file missing'); };
    assert.equal(await resolver.prefetch(locator()), undefined);
    await assert.rejects(resolver.resolve(locator()), /a.zip.*song.mod.*file missing/);
    source.readFile = async () => new Uint8Array([1, 2]).buffer;
    assert.deepEqual([...new Uint8Array(await resolver.resolve(locator()))], [2]);
});
