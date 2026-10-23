import test from 'node:test';
import assert from 'node:assert/strict';
import { createFileListSource, createDirectorySource, createHttpSource, openDirectorySource, reconnectDirectorySource } from '../www/library-source.js';

const file = (path, contents = 'source_path\na.mod') => ({ webkitRelativePath: path, size: contents.length, arrayBuffer: async () => new TextEncoder().encode(contents).buffer, slice: (start, end) => new Blob([contents]).slice(start, end) });

test('FileList strips exactly one component without reading and discovers root CSVs', async () => {
    const source = await createFileListSource([file('root/INDEX.CSV'), file('root/root/deep/song.mod'), { ...file('root/unread.mod'), arrayBuffer() { throw new Error('must not read during mapping'); } }]);
    assert.equal(source.label, 'root');
    assert.equal(source.indexPath, 'INDEX.CSV');
    assert.equal(await source.size('root/deep/song.mod'), 17);
    assert.equal(new TextDecoder().decode(await source.readFile('root/deep/song.mod')), 'source_path\na.mod');
    await assert.rejects(source.readFile('deep/song.mod'), /deep\/song.mod.*file missing/);
    assert.equal((await createFileListSource([file('root/other.csv'), file('root/nested/index.csv')])).indexPath, 'other.csv');
    const multiple = await createFileListSource([file('root/a.csv'), file('root/b.CSV')]);
    assert.equal(multiple.indexPath, null);
    assert.deepEqual(multiple.candidates, ['a.csv', 'b.CSV']);
    assert.deepEqual((await createFileListSource([file('root/nested/index.csv')])).candidates, []);
});

test('directory walks read-only handles and returns file size', async () => {
    const calls = [];
    const leaf = { getFileHandle: async name => { calls.push(name); return { getFile: async () => file('', 'module') }; } };
    const handle = { name: 'root', async *entries() { yield ['Index.CSV', { kind: 'file' }]; yield ['nested', { kind: 'directory' }]; }, getFileHandle: async () => ({ getFile: async () => file('', 'source_path\na.mod') }), getDirectoryHandle: async name => { calls.push(name); return leaf; } };
    const source = await createDirectorySource(handle);
    assert.equal(source.indexPath, 'Index.CSV');
    assert.equal(await source.size('nested/song.mod'), 6);
    assert.deepEqual(calls, ['nested', 'song.mod']);
    await assert.rejects(reconnectDirectorySource({ name: 'root', requestPermission: async () => 'denied' }), /permission denied/);
});

test('picker is invoked immediately before any other async work', async () => {
    let invoked = false;
    globalThis.window = { showDirectoryPicker(options) { invoked = true; assert.deepEqual(options, { mode: 'read' }); return Promise.reject(new DOMException('cancel', 'AbortError')); } };
    const opening = openDirectorySource();
    assert.equal(invoked, true);
    await assert.rejects(opening, { name: 'AbortError' });
    delete globalThis.window;
});

test('HTTP encodes each segment, forwards signal, and uses HEAD length', async () => {
    const calls = [];
    const source = createHttpSource({ fetch: async (url, options) => { calls.push([url, options]); return new Response('abc', { headers: { 'Content-Length': '3' } }); } });
    const controller = new AbortController();
    assert.equal((await source.readFile('a b/#%?.zip', { signal: controller.signal })).byteLength, 3);
    assert.equal(await source.size('a b/#%?.zip'), 3);
    assert.equal(calls[0][0], '/library/a%20b/%23%25%3F.zip');
    assert.equal(calls[0][1].signal, controller.signal);
    assert.equal(calls[1][1].method, 'HEAD');
    await assert.rejects(source.readFile('../bad'), /invalid library path/);
});

test('HTTP size preserves path in network errors and allows unknown lengths', async () => {
    const failing = createHttpSource({ fetch: async () => { throw new TypeError('network failure'); } });
    await assert.rejects(failing.size('packs/songs.zip'), /packs\/songs.zip.*network failure/);
    const unknown = createHttpSource({ fetch: async () => new Response('') });
    assert.equal(await unknown.size('songs.zip'), null);
});

test('root discovery separates sidecars, ignores failed reads, and reads only a slice', async () => {
    const index = name => file(`root/${name}`, 'source_path\nsong.mod');
    const comments = name => file(`root/${name}`, '"SHA256",instrument_names\n' + 'a'.repeat(64) + ',Names');
    for (const [indexName, commentsName] of [['index.csv', 'comments.csv'], ['hornet-modules.csv', 'hornet-comments.csv']]) {
        const source = await createFileListSource([index(indexName), comments(commentsName)]);
        assert.equal(source.indexPath, indexName);
        assert.equal(source.commentsPath, commentsName);
        assert.deepEqual(source.candidates, [indexName]);
        assert.equal(await source.readHead(indexName, 6), 'source');
    }
    const source = await createFileListSource([index('index.csv'), comments('one.csv'), comments('two.csv'), {
        ...index('unreadable.csv'), slice() { throw new Error('unreadable'); },
    }]);
    assert.equal(source.indexPath, 'index.csv');
    assert.equal(source.commentsPath, null);
    assert.deepEqual(source.commentsCandidates, ['one.csv', 'two.csv']);
    assert.deepEqual(source.candidates, ['index.csv']);
});

test('custom HTTP base and remote URL derivation, matching, proxy changes and errors', async () => {
    const { createRemoteSource, normalizeProxy } = await import('../www/library-source.js');
    const { parseLibraryCsv } = await import('../www/library-index.js');
    const calls = [];
    const fetch = async url => { calls.push(url); return new Response('abc'); };
    await createHttpSource({ base: 'https://archive.test/base/', fetch }).readFile('a b/#.mod');
    assert.equal(calls.pop(), 'https://archive.test/base/a%20b/%23.mod');
    const library = parseLibraryCsv('source_path,member_path,source_url\na b/#.mod,, https://archive.test/base/a%20b/%23.mod \np.zip,a.mod,https://archive.test/base/p.zip\np.zip,b.mod,https://archive.test/base/p.zip\nother.mod,,https://other.test/other.mod\nmissing.mod,,');
    assert.equal(library.rows[0].sourceUrl, 'https://archive.test/base/a%20b/%23.mod');
    const source = createRemoteSource({ fetch });
    await assert.rejects(source.readFile('a b/#.mod'), /attach/);
    assert.deepEqual(source.attach(library), { base: 'https://archive.test/base/', streamable: 3, unmatched: 2 });
    assert.equal(source.kind, 'remote');
    assert.equal(source.indexPath, null);
    assert.equal(source.label, 'archive.test');
    assert.equal(source.size('p.zip'), null);
    await source.readFile('a b/#.mod');
    assert.equal(calls.pop(), 'https://archive.test/base/a%20b/%23.mod');
    await assert.rejects(source.readFile('other.mod'), /other.mod: source_url does not follow https:\/\/archive.test\/base\//);
    source.setProxy('https://proxy.test/files');
    await source.readFile('p.zip');
    assert.equal(calls.pop(), 'https://proxy.test/files/p.zip');
    assert.equal(source.label, 'archive.test via proxy.test');
    source.setProxy('');
    await source.readFile('p.zip');
    assert.equal(calls.pop(), 'https://archive.test/base/p.zip');
    assert.throws(() => source.attach(parseLibraryCsv('source_path\nsong.mod')), /No usable source_url/);
    assert.throws(() => source.attach(parseLibraryCsv('source_path,source_url\nsong.mod,https://archive.test/unrelated.mod')), /consistent base/);
    const raw = createRemoteSource({ fetch });
    assert.equal(raw.attach(parseLibraryCsv('source_path,source_url\na b/#.mod,https://archive.test/base/a b/#.mod')).streamable, 1);
    assert.equal(raw.attach(parseLibraryCsv('source_path,source_url\nsong.mod,https://ARCHIVE.test:443/music/song.mod')).base, 'https://ARCHIVE.test:443/music/');
    const failing = createRemoteSource({ fetch: async () => { throw new TypeError('network failure'); } });
    failing.attach(library);
    await assert.rejects(failing.readFile('p.zip'), /p.zip: could not fetch from archive.test; it may not allow cross-origin requests — set a Proxy URL/);
    const aborted = new DOMException('cancel', 'AbortError');
    const cancelling = createRemoteSource({ fetch: async () => { throw aborted; } });
    cancelling.attach(library);
    await assert.rejects(cancelling.readFile('p.zip'), error => error === aborted);
    for (const value of ['relative', 'https:/proxy.test', 'http:proxy.test', 'ftp://proxy.test/', 'https://proxy.test/?q=x', 'https://user:password@proxy.test/']) assert.throws(() => normalizeProxy(value), /Proxy URL/);
    assert.equal(normalizeProxy(' http://localhost:8080 '), 'http://localhost:8080/');
});
