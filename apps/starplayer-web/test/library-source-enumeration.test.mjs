import test from 'node:test';
import assert from 'node:assert/strict';
import { createDirectorySource, createFileListSource, discoverIndex } from '../www/library-source.js';

const collect = async iterator => { const records = []; for await (const record of iterator) records.push(record); return records; };
const file = (path, contents = '', lastModified = 0) => ({
    webkitRelativePath: path, size: contents.length, lastModified,
    arrayBuffer() { throw new Error('unexpected content read'); },
    slice: (start, end) => new Blob([contents]).slice(start, end),
});
const directory = (name, entries = []) => ({ name, kind: 'directory', async *entries() { yield* entries; }, getFileHandle() { throw new Error('unexpected getFile'); } });
const moduleEntry = { kind: 'file', getFile() { throw new Error('unexpected file I/O'); } };

test('directory walk includes all regular files, duplicate basenames and empty folders without file I/O', async () => {
    const source = await createDirectorySource(directory('root', [
        ['SONG.MOD', moduleEntry], ['one', directory('one', [['song.mod', moduleEntry]])],
        ['empty', directory('empty')], ['two', directory('two', [['song.mod', moduleEntry]])], ['notes.txt', moduleEntry],
    ]));
    assert.deepEqual(await collect(source.enumerate()), ['SONG.MOD', 'one/song.mod', 'two/song.mod', 'notes.txt'].map(path => ({ kind: 'file', path, size: null, timestamp: '' })));
    assert.deepEqual(await collect((await createDirectorySource(directory('empty'))).enumerate()), []);
});

test('iterative directory walk handles deep trees and yields to browser even without files', async () => {
    let root = directory('empty');
    for (let index = 0; index < 1500; index += 1) root = directory('deep', [['deep', root]]);
    const source = await createDirectorySource(root);
    let browserRan = false;
    setTimeout(() => { browserRan = true; }, 0);
    assert.deepEqual(await collect(source.enumerate()), []);
    assert.equal(browserRan, true);
});

test('pausing requests retains the current directory iterator; abort releases it', async () => {
    const handle = directory('root');
    const source = await createDirectorySource(handle);
    let advances = 0, closed = false;
    handle.entries = async function* () { try { for (const name of ['a.mod', 'b.mod', 'c.mod']) { advances += 1; yield [name, moduleEntry]; } } finally { closed = true; } };
    const controller = new AbortController();
    const iterator = source.enumerate({ signal: controller.signal });
    assert.equal((await iterator.next()).value.path, 'a.mod');
    await new Promise(resolve => setTimeout(resolve, 5));
    assert.equal(advances, 1);
    assert.equal((await iterator.next()).value.path, 'b.mod');
    controller.abort();
    await assert.rejects(iterator.next(), { name: 'AbortError' });
    assert.equal(advances, 2);
    assert.equal(closed, true);
});

test('abort is checked after an awaited directory iterator operation', async () => {
    const handle = directory('root');
    const source = await createDirectorySource(handle);
    let resolveNext;
    handle.entries = () => ({ [Symbol.asyncIterator]() { return this; }, next: () => new Promise(resolve => { resolveNext = resolve; }) });
    const controller = new AbortController();
    const result = source.enumerate({ signal: controller.signal }).next();
    controller.abort();
    resolveNext({ done: false, value: ['late.mod', moduleEntry] });
    await assert.rejects(result, { name: 'AbortError' });
});

test('invalid segments, vanished children, unreadable subdirectories and root failure become path issues', async () => {
    const unreadable = { kind: 'directory', entries() { throw new DOMException('read denied', 'NotAllowedError'); } };
    const handle = directory('root', [['../bad', moduleEntry], ['bad\\name', moduleEntry], ['.', moduleEntry], ['missing', { kind: 'directory' }], ['denied', unreadable], ['good.IT', moduleEntry]]);
    handle.getDirectoryHandle = async () => { throw new DOMException('vanished', 'NotFoundError'); };
    const records = await collect((await createDirectorySource(handle)).enumerate());
    assert.deepEqual(records.map(record => record.path), ['../bad', 'bad\\name', '.', 'missing', 'denied', 'good.IT']);
    assert.equal(records.filter(record => record.kind === 'issue').length, 5);
    assert.equal(records[3].reason, 'file missing');
    assert.equal(records[4].reason, 'read denied');
    const denied = await createDirectorySource({ name: 'denied', entries() { throw new Error('root permission denied'); } });
    assert.deepEqual(await collect(denied.enumerate()), [{ kind: 'issue', path: '', reason: 'root permission denied' }]);
    handle.entries = async function* () { yield ['partial.xm', moduleEntry]; throw new Error('root permission revoked'); };
    assert.deepEqual((await collect((await createDirectorySource(handle)).enumerate())).map(record => record.kind), ['file', 'issue']);
});

test('large FileList mapping and enumeration yield, preserve metadata, and resolve paths before scan', async () => {
    const files = Array.from({ length: 1000 }, (_, index) => file(`root/sub/${index}.MtM`, 'abc', 1700000000000));
    let mappedYield = false;
    setTimeout(() => { mappedYield = true; }, 0);
    const source = await createFileListSource(files);
    assert.equal(mappedYield, true);
    // Saved playlist paths can resolve before the first enumerate request.
    assert.equal(await source.size('sub/999.MtM'), 3);
    files[999].arrayBuffer = async () => new Uint8Array([1, 2]).buffer;
    assert.deepEqual(new Uint8Array(await source.readFile('sub/999.MtM')), new Uint8Array([1, 2]));
    let enumerationYield = false;
    setTimeout(() => { enumerationYield = true; }, 0);
    const records = await collect(source.enumerate());
    assert.equal(enumerationYield, true);
    assert.equal(records.length, 1000);
    assert.deepEqual(records[0], { kind: 'file', path: 'sub/0.MtM', size: 3, timestamp: '2023-11-14T22:13:20.000Z' });
});

test('FileList invalid paths and unreadable metadata are issues and use readFile validation', async () => {
    const broken = file('root/unread.mod');
    Object.defineProperty(broken, 'size', { get() { throw new Error('metadata denied'); } });
    const source = await createFileListSource([file('root/good.MOD'), file('root/a/../bad.mod'), file('root/bad\\name.mod'), file('plain.mod'), file('root/good.MOD'), broken]);
    const records = await collect(source.enumerate());
    assert.equal(records.filter(record => record.kind === 'issue').length, 5);
    assert.equal(records.filter(record => record.kind === 'file').length, 1);
    await assert.rejects(source.readFile('a/../bad.mod'), /invalid library path/);
    await assert.rejects(source.readFile('missing.mod'), /file missing/);
    const controller = new AbortController();
    const iterator = source.enumerate({ signal: controller.signal });
    await iterator.next();
    controller.abort();
    await assert.rejects(iterator.next(), { name: 'AbortError' });
    assert.deepEqual(await collect((await createFileListSource([])).enumerate()), []);
});

test('CSV candidate headers require source_path, exclude playlists and preserve malformed-body/manual access', async () => {
    const source = await createFileListSource([
        file('root/INDEX.CSV', '\uFEFF"source_path",title\r\nsong.mod,"unfinished'),
        file('root/index2.csv', 'source_path,title\nsong.mod,Title'),
        file('root/playlist.csv', 'source_path,library\nsong.mod,old'),
        file('root/unrelated.csv', 'name,title\nfoo,bar'),
        file('root/bad.csv', '"source_path,title\nunterminated'),
        file('root/comments.csv', '\uFEFF"SHA256",sample_names\n'),
        file('root/multiline.csv', '"irrelevant\ncolumn",source_path\nx,song.mod'),
    ]);
    assert.equal(source.indexPath, 'INDEX.CSV');
    assert.deepEqual(source.candidates, ['INDEX.CSV', 'index2.csv', 'multiline.csv']);
    assert.deepEqual(source.commentsCandidates, ['comments.csv']);
    assert.equal((await source.readHead('playlist.csv')).startsWith('source_path,library'), true);
    const playlistOnly = await createFileListSource([file('root/index.csv', 'source_path,LIBRARY\nsong.mod,old')]);
    assert.equal(playlistOnly.indexPath, null);
    assert.deepEqual(playlistOnly.candidates, []);
    const preferred = await createFileListSource([file('root/index.csv', 'library,source_path\nold,a.mod'), file('root/real.csv', 'source_path\na.mod')]);
    assert.equal(preferred.indexPath, 'real.csv');
});

test('readFile cancellation brackets individual directory handle awaits', async () => {
    const handle = directory('root');
    const source = await createDirectorySource(handle);
    const controller = new AbortController();
    let openedFile = false;
    handle.getDirectoryHandle = async () => { controller.abort(); return { getFileHandle() { openedFile = true; } }; };
    await assert.rejects(source.readFile('sub/song.mod', { signal: controller.signal }), { name: 'AbortError' });
    assert.equal(openedFile, false);
});


test('FileList mapping and enumeration yield at 128 records with a stationary clock', async () => {
    let mapped = 0, enumerated = 0;
    const boundaries = [];
    const files = Array.from({ length: 257 }, (_, index) => ({
        get webkitRelativePath() { mapped += 1; return `root/${index}.mod`; },
        get size() { enumerated += 1; return 0; },
        arrayBuffer() { throw new Error('unexpected content read'); },
    }));
    const source = await createFileListSource(files, { now: () => 0, yieldTask: async () => boundaries.push([mapped, enumerated]) });
    assert.deepEqual(boundaries.slice(0, 2), [[128, 0], [256, 0]]);
    boundaries.length = 0;
    assert.equal((await collect(source.enumerate())).length, 257);
    assert.deepEqual(boundaries, [[257, 128], [257, 256]]);
});

test('FileList mapping and enumeration yield after 8 ms before reaching 128 records', async () => {
    let elapsed = 0, mapped = 0, enumerated = 0;
    const boundaries = [];
    const files = Array.from({ length: 11 }, (_, index) => ({
        get webkitRelativePath() { elapsed += 2; mapped += 1; return `root/${index}.mod`; },
        get size() { elapsed += 2; enumerated += 1; return 0; },
        arrayBuffer() { throw new Error('unexpected content read'); },
    }));
    const source = await createFileListSource(files, { now: () => elapsed, yieldTask: async () => boundaries.push([mapped, enumerated]) });
    assert.deepEqual(boundaries, [[4, 0], [8, 0]]);
    boundaries.length = 0;
    await collect(source.enumerate());
    assert.deepEqual(boundaries, [[11, 4], [11, 8]]);
});

test('invalid FileList records share the same bounded enumeration budget', async () => {
    let yields = 0;
    const source = await createFileListSource(Array.from({ length: 257 }, () => file('bad.mod')), { now: () => 0, yieldTask: async () => { yields += 1; } });
    yields = 0;
    const records = await collect(source.enumerate());
    assert.equal(records.length, 257);
    assert.equal(records.every(record => record.kind === 'issue'), true);
    assert.equal(yields, 2);
});

for (const [budget, cost, expected] of [['records', 0, [128, 256]], ['time', 2, [4, 8]]]) {
    test(`directory root candidates and recursive iteration honor the ${budget} budget without file reads`, async () => {
        let elapsed = 0, advanced = 0;
        const boundaries = [];
        const count = cost ? 11 : 257;
        const handle = {
            name: 'root',
            async *entries() {
                for (let index = 0; index < count; index += 1) {
                    elapsed += cost;
                    advanced += 1;
                    yield [`${index}.mod`, moduleEntry];
                }
            },
            getFileHandle() { throw new Error('unexpected getFile'); },
        };
        const source = await createDirectorySource(handle, { now: () => elapsed, yieldTask: async () => boundaries.push(advanced) });
        assert.deepEqual(boundaries.slice(0, expected.length), expected);
        boundaries.length = 0;
        advanced = 0;
        assert.equal((await collect(source.enumerate())).length, count);
        assert.deepEqual(boundaries, expected);
    });
}

test('deep directory descent and empty-directory unwind both obey the record budget', async () => {
    let advances = 0;
    const boundaries = [];
    const countedDirectory = child => ({
        name: 'deep', kind: 'directory', entries() {
            let done = false;
            return { [Symbol.asyncIterator]() { return this; }, async next() {
                advances += 1;
                if (done || !child) return { done: true };
                done = true;
                return { done: false, value: ['deep', child] };
            } };
        },
    });
    let root = countedDirectory(null);
    for (let index = 0; index < 300; index += 1) root = countedDirectory(root);
    const source = await createDirectorySource(root, { now: () => 0, yieldTask: async () => boundaries.push(advances) });
    advances = 0;
    boundaries.length = 0;
    assert.deepEqual(await collect(source.enumerate()), []);
    assert.deepEqual(boundaries, [128, 256, 384, 512]);
});

test('candidate filtering yields on both budget triggers even when every path is ineligible', async () => {
    for (const cost of [0, 2]) {
        let elapsed = 0, examined = 0;
        const boundaries = [];
        function* paths() {
            for (let index = 0; index < (cost ? 11 : 257); index += 1) {
                examined += 1;
                elapsed += cost;
                yield `sub/${index}.csv`;
            }
        }
        const result = await discoverIndex(paths(), () => { throw new Error('unexpected header read'); }, { now: () => elapsed, yieldTask: async () => boundaries.push(examined) });
        assert.deepEqual(result.candidates, []);
        assert.deepEqual(boundaries, cost ? [4, 8] : [128, 256]);
    }
});

test('candidate sorting and header processing are sliced and retain deterministic ordering', async () => {
    const paths = Array.from({ length: 300 }, (_, index) => `${String(299 - index).padStart(3, '0')}.csv`);
    const reads = [], boundaries = [];
    const result = await discoverIndex(paths, async path => { reads.push(path); return 'source_path'; }, { now: () => 0, yieldTask: async () => boundaries.push(reads.length) });
    assert.deepEqual(result.candidates, [...paths].sort());
    assert.deepEqual(reads, result.candidates);
    // Filtering + nine merge passes must yield before any header is read.
    assert.equal(boundaries.filter(count => count === 0).length, Math.floor((300 + 9 * 300) / 128));
    const headerBoundaries = boundaries.filter(count => count > 0);
    assert.equal(headerBoundaries.length, 2);
    assert.equal(headerBoundaries[0] <= 128, true);
    assert.equal(headerBoundaries[1] - headerBoundaries[0], 128);
});

test('candidate headers yield at the time budget even when reads resolve as microtasks', async () => {
    let elapsed = 0, reads = 0;
    const boundaries = [];
    await discoverIndex(Array.from({ length: 11 }, (_, index) => `${index}.csv`), async () => { elapsed += 2; reads += 1; return 'source_path'; }, {
        now: () => elapsed, yieldTask: async () => boundaries.push(reads),
    });
    assert.deepEqual(boundaries, [4, 8]);
});

test('creation aborts before mapping or root iteration and after a candidate header await', async () => {
    const controller = new AbortController();
    controller.abort();
    await assert.rejects(createFileListSource({ *[Symbol.iterator]() { throw new Error('unexpected map'); } }, { signal: controller.signal }), { name: 'AbortError' });
    await assert.rejects(createDirectorySource({ entries() { throw new Error('unexpected root walk'); } }, { signal: controller.signal }), { name: 'AbortError' });
    const reading = new AbortController();
    await assert.rejects(discoverIndex(['index.csv'], async () => { reading.abort(); return 'source_path'; }, { signal: reading.signal }), { name: 'AbortError' });
});

test('abort during a cooperative source yield closes the root iterator and preserves cancellation', async () => {
    for (const stage of ['root', 'recursive']) {
        const controller = new AbortController();
        let closed = 0, advances = 0, abortYield = stage === 'root';
        const handle = { name: 'root', async *entries() {
            try { for (let index = 0; index < 300; index += 1) { advances += 1; yield [`${index}.mod`, moduleEntry]; } }
            finally { closed += 1; }
        } };
        const options = { now: () => 0, signal: stage === 'root' ? controller.signal : undefined, yieldTask: async ({ signal }) => {
            if (abortYield) { assert.equal(signal, controller.signal); controller.abort(); }
        } };
        if (stage === 'root') await assert.rejects(createDirectorySource(handle, options), { name: 'AbortError' });
        else {
            const source = await createDirectorySource(handle, options);
            advances = 0;
            closed = 0;
            abortYield = true;
            await assert.rejects(collect(source.enumerate({ signal: controller.signal })), { name: 'AbortError' });
        }
        assert.equal(advances, 128);
        assert.equal(closed, 1);
    }
});


test('map and root discovery carry partial slice budgets into candidate processing', async () => {
    for (const kind of ['filelist', 'directory']) {
        let yields = 0;
        const options = { now: () => 0, yieldTask: async () => { yields += 1; } };
        if (kind === 'filelist') await createFileListSource(Array.from({ length: 127 }, (_, index) => file(`root/${index}.mod`)), options);
        else await createDirectorySource(directory('root', Array.from({ length: 127 }, (_, index) => [`${index}.mod`, moduleEntry])), options);
        assert.equal(yields, 1);
    }
});

test('candidate sorting honors the time budget before any headers are read', async () => {
    let elapsed = 0, reads = 0;
    const boundaries = [];
    const paths = Array.from({ length: 11 }, (_, index) => `${10 - index}.csv`);
    const result = await discoverIndex(paths, async () => { reads += 1; return 'source_path'; }, {
        now: () => { elapsed += 2; return elapsed; }, yieldTask: async () => boundaries.push(reads),
    });
    assert.deepEqual(result.candidates, [...paths].sort());
    assert.ok(boundaries.filter(count => count === 0).length > 2);
    assert.ok(boundaries.some(count => count > 0));
});

test('FileList mapping and candidate header yields propagate abort instead of returning partial sources', async () => {
    for (const stage of ['mapping', 'headers']) {
        const controller = new AbortController();
        let elapsed = 0, mapped = 0, headers = 0;
        const files = Array.from({ length: stage === 'mapping' ? 300 : 3 }, (_, index) => ({
            get webkitRelativePath() { mapped += 1; return `root/${index}.${stage === 'mapping' ? 'mod' : 'csv'}`; },
            slice() { headers += 1; elapsed += 8; return new Blob(['source_path']); },
            arrayBuffer() { throw new Error('unexpected content read'); },
        }));
        await assert.rejects(createFileListSource(files, { signal: controller.signal, now: () => elapsed,
            yieldTask: async ({ signal }) => { assert.equal(signal, controller.signal); controller.abort(); },
        }), { name: 'AbortError' });
        assert.equal(mapped, stage === 'mapping' ? 128 : 3);
        assert.equal(headers, stage === 'mapping' ? 0 : 1);
    }
});
