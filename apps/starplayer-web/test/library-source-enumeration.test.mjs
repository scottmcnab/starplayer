import test from 'node:test';
import assert from 'node:assert/strict';
import { createDirectorySource, createFileListSource } from '../www/library-source.js';

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
