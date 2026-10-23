import test from 'node:test';
import assert from 'node:assert/strict';
import { wireLibraryControls } from '../www/library-controls.js';

const sha256 = 'a'.repeat(64);
const indexCsv = `source_path,sha256\nsong.mod,${sha256}`;
const commentsCsv = text => `sha256,instrument_names\n${sha256},${text}`;
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
async function until(predicate) {
    for (let index = 0; index < 200; index += 1) {
        if (predicate()) return;
        await new Promise(resolve => setTimeout(resolve, 1));
    }
    assert.fail('controls did not reach expected state');
}

// These tests exercise the asynchronous controls without depending on a browser picker.
test('sidecar loading never blocks the index, ignores stale loads, and manual choices replace it', async () => {
    const original = { document: globalThis.document, fetch: globalThis.fetch, requestAnimationFrame: globalThis.requestAnimationFrame };
    const elements = new Map();
    const element = name => {
        if (!elements.has(name)) {
            const target = new EventTarget();
            Object.assign(target, { textContent: '', hidden: false, disabled: name === 'open-comments', value: '', files: [] });
            elements.set(name, target);
        }
        return elements.get(name);
    };
    globalThis.document = { getElementById: id => element(id.slice('library-'.length)) };
    globalThis.requestAnimationFrame = callback => setTimeout(callback, 0);
    const pending = [];
    let painted = 0, updated = 0;
    const state = {};
    globalThis.fetch = async (url, options) => {
        if (url === '/library-index.csv') return new Response(indexCsv);
        assert.equal(url, '/library-comments.csv');
        if (options?.method === 'HEAD') return new Response('');
        assert.ok(painted > 0, 'index notification precedes comments GET');
        const response = deferred();
        pending.push(response);
        return response.promise;
    };
    try {
        await wireLibraryControls({ state, loader: {}, onLibrary() { painted += 1; }, onComments() { updated += 1; } });
        assert.equal(element('open-comments').disabled, true);
        element('server').dispatchEvent(new Event('click'));
        await until(() => pending.length === 1);
        assert.equal(state.library.library.rows.length, 1);
        assert.equal(element('open-comments').disabled, false);
        const previous = state.library;
        element('server').dispatchEvent(new Event('click'));
        await until(() => pending.length === 2);
        const identity = state.library.identity;
        pending[1].resolve(new Response(commentsCsv('Current')));
        await until(() => updated === 1);
        pending[0].resolve(new Response(commentsCsv('Stale')));
        await new Promise(resolve => setTimeout(resolve, 10));
        assert.equal(updated, 1);
        assert.equal(previous.library.hasComments, false);
        assert.equal(state.library.library.commentsFor(0).instrumentNames, 'Current');
        element('comments').files = [{ name: 'manual.csv', arrayBuffer: async () => new TextEncoder().encode(commentsCsv('Manual')).buffer }];
        element('comments').dispatchEvent(new Event('change'));
        await until(() => updated === 2);
        assert.equal(state.library.library.commentsFor(0).instrumentNames, 'Manual');
        assert.deepEqual(state.library.identity, identity);
        assert.match(element('summary').textContent, /manual.csv.*1 tracks matched.*0 issues.*until the page reloads/);
        element('comments').files = [{ name: 'broken.csv', arrayBuffer: async () => new TextEncoder().encode('wrong\nheader').buffer }];
        element('comments').dispatchEvent(new Event('change'));
        await until(() => element('summary').textContent.includes('not loaded'));
        assert.equal(state.library.library.commentsFor(0).instrumentNames, 'Manual', 'failed replacement leaves the working attachment intact');
        element('server').dispatchEvent(new Event('click'));
        await until(() => pending.length === 3);
        pending[2].resolve(new Response('', { status: 500 }));
        await until(() => element('summary').textContent.includes('HTTP 500'));
        assert.equal(state.library.library.rows.length, 1);
        assert.equal(state.library.library.hasComments, false);
    } finally {
        for (const [key, value] of Object.entries(original)) {
            if (value === undefined) delete globalThis[key]; else globalThis[key] = value;
        }
    }
});

test('remote controls sniff only the header, update proxy and cache, and survive denied storage', async () => {
    const original = { document: globalThis.document, fetch: globalThis.fetch, requestAnimationFrame: globalThis.requestAnimationFrame, localStorage: globalThis.localStorage };
    const elements = new Map();
    const element = name => {
        if (!elements.has(name)) {
            const target = new EventTarget();
            Object.assign(target, { textContent: '', hidden: true, disabled: true, value: '', files: [] });
            elements.set(name, target);
        }
        return elements.get(name);
    };
    globalThis.document = { getElementById: id => element(id.slice('library-'.length)) };
    globalThis.requestAnimationFrame = callback => setTimeout(callback, 0);
    const preferences = new Map([['starplayer.libraryProxy', 'https://proxy.test/']]);
    globalThis.localStorage = { getItem: key => preferences.get(key) ?? null, setItem: (key, value) => preferences.set(key, value) };
    const calls = [];
    globalThis.fetch = async (url, options) => {
        calls.push([url, options]);
        if (url === '/library-index.csv') return new Response(null, { status: 404 });
        if (url === 'https://proxy.test/song.mod' || url === 'https://archive.test/music/song.mod') return new Response('bytes');
        assert.fail(`unexpected request ${url}`);
    };
    const state = {};
    const chooseIndex = text => {
        element('index').files = [{ name: 'synthetic.csv', arrayBuffer: async () => new TextEncoder().encode(text).buffer }];
        element('index').dispatchEvent(new Event('change'));
    };
    try {
        await wireLibraryControls({ state, loader: {} });
        assert.equal(element('proxy').value, 'https://proxy.test/');
        // Malformed later records must not be parsed just to offer the button.
        chooseIndex('source_path," SOURCE_URL "\nsong.mod,"unterminated');
        await until(() => !element('stream').hidden);
        assert.equal(element('proxy-field').hidden, false);
        chooseIndex('source_path\nsong.mod');
        await until(() => element('summary').textContent.includes('selected'));
        assert.equal(element('stream').hidden, true);
        chooseIndex('source_path,source_url,sha256\nsong.mod,https://archive.test/music/song.mod,' + sha256 + '\nunmatched.mod,,');
        await until(() => !element('stream').hidden);
        element('stream').dispatchEvent(new Event('click'));
        await until(() => state.library?.source.kind === 'remote');
        await until(() => element('summary').textContent.includes("won't be offered after a reload"));
        assert.match(element('summary').textContent, /streaming 1 tracks from archive.test via proxy.test; 1 rows have no matching source_url/);
        assert.equal(element('stream').hidden, true);
        assert.equal(element('proxy-field').hidden, false);
        const identity = state.library.identity;
        await state.library.source.readFile('song.mod');
        assert.equal(calls.at(-1)[0], 'https://proxy.test/song.mod');
        assert.equal(calls.at(-1)[1].credentials, 'omit');
        let cleared = 0;
        state.library.resolver.clear = () => cleared++;
        element('proxy').value = 'relative';
        element('proxy').dispatchEvent(new Event('change'));
        assert.match(element('summary').textContent, /Proxy URL/);
        assert.equal(cleared, 0);
        assert.equal(state.library.source.proxy, 'https://proxy.test/');
        element('proxy').value = '';
        element('proxy').dispatchEvent(new Event('change'));
        assert.equal(cleared, 1);
        assert.equal(preferences.get('starplayer.libraryProxy'), '');
        await state.library.source.readFile('song.mod');
        assert.equal(calls.at(-1)[0], 'https://archive.test/music/song.mod');
        globalThis.localStorage = { getItem() { throw new Error('blocked'); }, setItem() { throw new Error('blocked'); } };
        element('proxy').value = 'https://proxy.test';
        element('proxy').dispatchEvent(new Event('change'));
        assert.equal(cleared, 2);
        assert.equal(element('proxy').value, 'https://proxy.test/');
        element('comments').files = [{ name: 'remote-comments.csv', arrayBuffer: async () => new TextEncoder().encode(commentsCsv('Remote names')).buffer }];
        element('comments').dispatchEvent(new Event('change'));
        await until(() => state.library.library.commentsFor(0).instrumentNames === 'Remote names');
        assert.deepEqual(state.library.identity, identity);
        assert.equal(state.library.record.commentsName, 'remote-comments.csv');
        await new Promise(resolve => setTimeout(resolve, 10));
        assert.ok(!calls.some(([url]) => url === '/library-comments.csv'), 'remote does not discover comments');
        assert.match(element('summary').textContent, /remote-comments.csv/);
        assert.doesNotMatch(element('summary').textContent, /choice lasts until/);
    } finally {
        for (const [key, value] of Object.entries(original)) {
            if (value === undefined) delete globalThis[key]; else globalThis[key] = value;
        }
    }
});
