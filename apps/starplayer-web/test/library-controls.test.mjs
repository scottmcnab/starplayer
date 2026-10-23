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
