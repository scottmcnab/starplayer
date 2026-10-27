import test from 'node:test';
import assert from 'node:assert/strict';
import { appendDiscoveredRows, updateDiscoveredRows, attachComments } from '../www/library-index.js';
import { wireLibraryControls } from '../www/library-controls.js';

const sha256 = 'a'.repeat(64);
const indexCsv = `source_path,sha256\nsong.mod,${sha256}`;
const commentsCsv = text => `sha256,instrument_names\n${sha256},${text}`;
const deferred = () => { let resolve, reject; const promise = new Promise((done, fail) => { resolve = done; reject = fail; }); return { promise, resolve, reject }; };
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
        await until(() => element('selection-status').textContent.includes('selected'));
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
        assert.match(element('selection-status').textContent, /Proxy URL/);
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

function fakeFile(path, text = 'module bytes') {
    const bytes = new TextEncoder().encode(text);
    return { name: path.split('/').at(-1), webkitRelativePath: path, size: bytes.length, lastModified: 0, reads: 0,
        slice() { return { text: async () => text }; },
        async arrayBuffer() { this.reads += 1; return bytes.slice().buffer; } };
}
async function controlHarness({ missingMarkup = false, identityForFolder, directoryPicker, discoveryFactory } = {}) {
    const original = Object.fromEntries(['document', 'fetch', 'requestAnimationFrame', 'localStorage', 'Option', 'window', 'isSecureContext'].map(key => [key, globalThis[key]]));
    const elements = new Map(), contexts = [], updates = [], runtimes = [];
    const extra = new Set(['browse-folder', 'stop-discovery', 'resume-discovery', 'retry-discovery', 'scan-metadata', 'cancel-metadata']);
    const element = name => {
        if (missingMarkup && extra.has(name)) return null;
        if (!elements.has(name)) {
            const target = new EventTarget();
            Object.assign(target, { textContent: '', hidden: true, disabled: false, value: '', files: [], options: [],
                replaceChildren(...options) { target.options = options; }, click() { target.dispatchEvent(new Event('click')); } });
            elements.set(name, target);
        }
        return elements.get(name);
    };
    globalThis.document = { getElementById: id => element(id.slice('library-'.length)) };
    globalThis.Option = class { constructor(text, value) { this.text = text; this.value = value; } };
    globalThis.localStorage = { getItem: () => null, setItem() {} };
    globalThis.fetch = async () => new Response(null, { status: 404 });
    globalThis.requestAnimationFrame = callback => setTimeout(callback, 0);
    if (directoryPicker) { globalThis.window = { showDirectoryPicker: directoryPicker }; globalThis.isSecureContext = true; }
    else { delete globalThis.window; }
    const { createDiscoveryController } = await import('../www/library-discovery.js');
    const runtimeFactory = () => {
        const runtime = { jobs: [], disposed: false, paused: false,
            enqueue(request) {
                let resolve, reject;
                const result = new Promise((done, fail) => { resolve = done; reject = fail; });
                const job = { ...request, result, resolve: payload => resolve({ generation: request.generation, kind: request.kind, payload }), reject,
                    cancel: () => reject(Object.assign(new Error('cancelled'), { code: 'aborted' })) };
                runtime.jobs.push(job); return job;
            },
            dispose() { runtime.disposed = true; for (const job of runtime.jobs) job.reject(Object.assign(new Error('disposed'), { code: 'disposed' })); },
            pauseCatalog() { runtime.paused = true; }, resumeCatalog() { runtime.paused = false; },
            cancelMetadata() { for (const job of runtime.jobs.filter(job => job.kind === 'inspect')) job.cancel(); },
        };
        runtimes.push(runtime); return runtime;
    };
    const state = {};
    await wireLibraryControls({ state, loader: {}, identityForFolder: identityForFolder || (async source => ({ identity: { name: source.label, sha256: sha256 }, storageFailed: false })),
        createDiscovery: options => discoveryFactory ? discoveryFactory(options) : createDiscoveryController({ ...options, runtimeFactory, digest: async () => new Uint8Array(32).fill(0xaa).buffer }),
        onLibrary(context) { contexts.push(context); }, onLibraryUpdate(context, change) { updates.push({ context, change }); } });
    return { state, element, contexts, updates, runtimes,
        chooseFolder(files) { element('files').files = files; element('files').dispatchEvent(new Event('change')); },
        chooseIndex(file) { element('index').files = [file]; element('index').dispatchEvent(new Event('change')); },
        close() {
            for (const context of contexts) context.discovery?.dispose();
            for (const [key, value] of Object.entries(original)) { if (value === undefined) delete globalThis[key]; else globalThis[key] = value; }
        } };
}

test('no-index/playlist-only folders discover immediately, tolerate absent markup and read no modules', async () => {
    const h = await controlHarness({ missingMarkup: true });
    try {
        const music = fakeFile('root/music.mod'), playlist = fakeFile('root/index.csv', 'library,source_path\nold,saved.mod');
        h.chooseFolder([music, playlist]); await until(() => h.state.library?.discoveryStatus?.phase === 'complete');
        assert.equal(h.contexts.length, 1); assert.equal(h.contexts[0].mode, 'discovered');
        assert.equal(h.state.library.library.rows.length, 1); assert.equal(music.reads, 0); assert.equal(playlist.reads, 0);
        assert.match(h.element('summary').textContent, /1 tracks found, discovery complete/);
        assert.equal(h.state.library.library.rows[0].metadataState, 'unknown');
    } finally { h.close(); }
});

test('valid indexed folders can bypass CSV and source replacement disposes old work', async () => {
    const h = await controlHarness();
    try {
        h.chooseFolder([fakeFile('root/index.csv', indexCsv), fakeFile('root/song.mod'), fakeFile('root/extra.it')]);
        await until(() => h.state.library?.mode === 'indexed');
        assert.equal(h.state.library.library.rows.length, 1); assert.equal(h.element('browse-folder').hidden, false);
        h.element('browse-folder').click(); await until(() => h.state.library?.discoveryStatus?.phase === 'complete');
        assert.equal(h.state.library.library.rows.length, 2); assert.equal(h.contexts.length, 2);
        const previous = h.state.library;
        previous.discovery.scanMetadata(); assert.equal(h.runtimes[0].jobs.length, 2);
        h.chooseFolder([fakeFile('new/new.mod')]); await until(() => h.state.library !== previous);
        assert.equal(h.runtimes[0].disposed, true); assert.equal(h.contexts.length, 3);
        const updates = h.updates.length;
        await new Promise(resolve => setTimeout(resolve, 120));
        assert.ok(h.updates.slice(updates).every(update => update.context !== previous));
        assert.equal(previous.library.rows[0].metadataState, 'unknown');
    } finally { h.close(); }
});

test('automatic and manual malformed indexes require explicit browse, clearing pending choices', async () => {
    const h = await controlHarness();
    try {
        h.chooseFolder([fakeFile('auto/index.csv', 'source_path\n"unterminated'), fakeFile('auto/auto.mod')]);
        await until(() => h.element('selection-status').textContent.includes('Unterminated'));
        assert.equal(h.state.library, undefined); assert.match(h.element('selection-status').textContent, /Browse folder instead/);
        h.element('browse-folder').click(); await until(() => h.state.library?.mode === 'discovered');
        h.chooseIndex(fakeFile('manual.csv', 'wrong_header\nvalue'));
        await until(() => h.element('selection-status').textContent.includes('manual.csv selected'));
        const previous = h.state.library;
        h.chooseFolder([fakeFile('next/next.mod')]); await until(() => h.element('selection-status').textContent.includes('Missing CSV header'));
        assert.equal(h.state.library, previous, 'no silent fallback after malformed pending index');
        h.element('browse-folder').click(); await until(() => h.state.library !== previous);
        assert.equal(h.state.library.library.rows[0].sourcePath, 'next.mod');
        const second = h.state.library;
        h.chooseFolder([fakeFile('third/third.mod')]); await until(() => h.state.library !== second);
        assert.equal(h.state.library.mode, 'discovered', 'Browse clears malformed pending index');
    } finally { h.close(); }
});

test('candidate chooser offers browse, manual playlist index still has precedence', async () => {
    const h = await controlHarness();
    try {
        h.chooseFolder([fakeFile('root/a.csv', 'source_path\na.mod'), fakeFile('root/b.csv', 'source_path\nb.mod'), fakeFile('root/song.mod')]);
        await until(() => !h.element('choices').hidden);
        assert.ok(h.element('choices').options.some(option => option.value === '__browse__'));
        h.element('choices').value = '__browse__'; h.element('choices').dispatchEvent(new Event('change'));
        await until(() => h.state.library?.mode === 'discovered');
        h.chooseIndex(fakeFile('playlist.csv', 'library,source_path\nold,from-playlist.mod'));
        await until(() => h.element('selection-status').textContent.includes('playlist.csv selected'));
        h.chooseFolder([fakeFile('other/index.csv', 'source_path\nauto.mod'), fakeFile('other/loose.mod')]);
        await until(() => h.state.library?.mode === 'indexed');
        assert.equal(h.state.library.library.rows[0].sourcePath, 'from-playlist.mod');
    } finally { h.close(); }
});

test('Browse invalidates a pending manual CSV read and an in-flight automatic CSV read', async () => {
    const h = await controlHarness();
    try {
        h.chooseFolder([fakeFile('root/a.mod')]); await until(() => h.state.library?.discoveryStatus?.phase === 'complete');
        const manualRead = deferred();
        h.chooseIndex({ name: 'slow.csv', arrayBuffer: () => manualRead.promise });
        const previous = h.state.library;
        h.element('browse-folder').click(); await until(() => h.state.library !== previous);
        manualRead.resolve(new TextEncoder().encode('source_path\nwrong.mod').buffer);
        await new Promise(resolve => setTimeout(resolve, 5));
        assert.doesNotMatch(h.element('selection-status').textContent, /slow.csv selected/);
        const automaticRead = deferred(), automatic = fakeFile('automatic/index.csv', 'source_path\nwrong.mod');
        automatic.arrayBuffer = () => automaticRead.promise;
        const accepted = h.state.library;
        h.chooseFolder([automatic, fakeFile('automatic/right.mod')]);
        await until(() => !h.element('browse-folder').hidden);
        // Wait for FileList candidate classification to reach the index content read.
        await new Promise(resolve => setTimeout(resolve, 5));
        h.element('browse-folder').click(); await until(() => h.state.library !== accepted);
        automaticRead.resolve(new TextEncoder().encode('source_path\nwrong.mod').buffer);
        await new Promise(resolve => setTimeout(resolve, 5));
        assert.equal(h.state.library.mode, 'discovered'); assert.equal(h.state.library.library.rows[0].sourcePath, 'right.mod');
        const current = h.state.library;
        h.chooseFolder([fakeFile('last/last.mod')]); await until(() => h.state.library !== current);
        assert.equal(h.state.library.mode, 'discovered', 'late manual CSV cannot restore a cleared pending choice');
    } finally { h.close(); }
});

test('cancelled folder picker preserves current discovery and running metadata', async () => {
    const h = await controlHarness({ directoryPicker: async () => { throw new DOMException('cancel', 'AbortError'); } });
    try {
        h.chooseFolder([fakeFile('root/a.mod')]); await until(() => h.state.library?.discoveryStatus?.phase === 'complete');
        const context = h.state.library;
        h.element('scan-metadata').click(); assert.equal(h.runtimes[0].jobs.length, 1);
        h.element('folder').click(); await new Promise(resolve => setTimeout(resolve, 5));
        assert.equal(h.state.library, context); assert.equal(h.runtimes[0].disposed, false);
        assert.equal(context.discoveryStatus.metadataPhase, 'running'); assert.equal(h.contexts.length, 1);
        h.element('stop-discovery').click(); assert.equal(context.discoveryStatus.phase, 'paused');
        h.element('resume-discovery').click(); assert.equal(context.discoveryStatus.phase, 'complete');
        h.element('cancel-metadata').click(); assert.equal(context.discoveryStatus.metadataPhase, 'cancelled');
    } finally { h.close(); }
});

test('discovered sidecar summaries update after late hashes and retain latest replacement', async () => {
    const h = await controlHarness();
    try {
        h.chooseFolder([fakeFile('root/a.mod')]); await until(() => h.state.library?.discoveryStatus?.phase === 'complete');
        const context = h.state.library;
        h.element('comments').files = [fakeFile('first.csv', commentsCsv('first'))];
        h.element('comments').dispatchEvent(new Event('change'));
        await until(() => h.element('summary').textContent.includes('awaiting metadata/hashes'));
        assert.match(h.element('summary').textContent, /0 tracks matched.*1 awaiting metadata\/hashes/);
        h.element('comments').files = [fakeFile('latest.csv', commentsCsv('latest'))];
        h.element('comments').dispatchEvent(new Event('change'));
        await until(() => h.element('summary').textContent.includes('latest.csv'));
        context.discovery.acceptPlaybackMetadata({ sourcePath: 'a.mod' }, { format: 'mod', title: 'played', channels: 4, instrumentNames: '', sampleNames: '', songMessage: '' }, new Uint8Array([1]));
        await until(() => h.element('summary').textContent.includes('1 tracks matched'));
        assert.doesNotMatch(h.element('summary').textContent, /awaiting metadata\/hashes/);
        assert.equal(context.library.commentsFor(0).instrumentNames, 'latest');
        assert.equal(h.contexts.length, 1, 'metadata/comments changes never replace the library');
        assert.ok(h.updates.some(update => update.change.metadataUpdates.some(update => update.patch.sha256 === sha256)));
    } finally { h.close(); }
});

test('stale folder identity failures cannot overwrite a newer accepted library', async () => {
    const pending = deferred();
    const h = await controlHarness({ identityForFolder: source => source.label === 'old' ? pending.promise : Promise.resolve({ identity: { name: source.label, sha256 }, storageFailed: false }) });
    try {
        h.chooseFolder([fakeFile('old/old.mod')]); await new Promise(resolve => setTimeout(resolve, 5));
        h.chooseFolder([fakeFile('new/new.mod')]); await until(() => h.state.library?.source.label === 'new');
        pending.reject(new Error('stale identity error')); await new Promise(resolve => setTimeout(resolve, 5));
        assert.doesNotMatch(h.element('summary').textContent, /stale identity/); assert.match(h.element('summary').textContent, /from new/);
    } finally { h.close(); }
});

test('a late rejected folder picker cannot overwrite a newer accepted source', async () => {
    const pending = deferred();
    const h = await controlHarness({ directoryPicker: () => pending.promise });
    try {
        h.element('folder').click();
        h.chooseFolder([fakeFile('new/new.mod')]); await until(() => h.state.library?.discoveryStatus?.phase === 'complete');
        pending.reject(new Error('stale picker error')); await new Promise(resolve => setTimeout(resolve, 5));
        assert.doesNotMatch(h.element('summary').textContent, /stale picker/); assert.match(h.element('summary').textContent, /from new/);
    } finally { h.close(); }
});

// Keep emitting through the controls' real callback closure, even after disposal,
// so acceptance tests detect silenced current callbacks AND leaked stale callbacks.
function emittingDiscovery({ context, onUpdate }) {
    const emit = () => onUpdate(context, { changedRowIds: [0], metadataUpdates: [] });
    const controller = { disposed: 0, starts: 0, comments: 0,
        emit,
        start() { this.starts += 1; appendDiscoveredRows(context.library, [{ sourcePath: 'live.mod', playable: true, sha256 }]); emit(); },
        dispose() { this.disposed += 1; },
        pause() { context.discoveryStatus.phase = 'paused'; emit(); },
        resume() { context.discoveryStatus.phase = 'running'; emit(); },
        acceptPlaybackMetadata(locator, metadata) { updateDiscoveredRows(context.library, [{ locator, patch: metadata }]); emit(); },
        attachComments(comments) { this.comments += 1; const counts = attachComments(context.library, comments); emit(); return counts; },
    };
    context.discovery = controller;
    context.discoveryStatus = { phase: 'running', tracksFound: 1, filesExamined: 1, archivesPending: 0, issueCount: 0 };
    return controller;
}
function assertStillLive(h, context, title) {
    const before = h.updates.length;
    context.discovery.acceptPlaybackMetadata({ sourcePath: 'live.mod' }, { title });
    assert.equal(context.library.rows[0].title, title);
    assert.equal(h.updates.length, before + 1, 'accepted row updates remain observable');
    assert.equal(h.updates.at(-1).context, context);
    assert.equal(h.state.library, context);
    assert.equal(context.discovery.disposed, 0);
    h.element('stop-discovery').click(); assert.equal(context.discoveryStatus.phase, 'paused');
    h.element('resume-discovery').click(); assert.equal(context.discoveryStatus.phase, 'running');
}

test('pending chooser, delayed/invalid index and failed remote attachment retain accepted callbacks and caches', async () => {
    const h = await controlHarness({ discoveryFactory: emittingDiscovery });
    try {
        h.chooseFolder([fakeFile('accepted/live.mod')]); await until(() => h.state.library);
        const context = h.state.library, generation = context.acceptedGeneration;
        let clears = 0; context.resolver.clear = () => clears++;
        h.chooseFolder([fakeFile('candidate/a.csv', indexCsv), fakeFile('candidate/b.csv', indexCsv)]);
        await until(() => !h.element('choices').hidden);
        assertStillLive(h, context, 'during chooser');
        assert.equal(h.element('selection-status').textContent, 'Choose a root index CSV.');
        assert.match(h.element('summary').textContent, /from accepted/);
        h.element('comments').files = [fakeFile('manual.csv', commentsCsv('chooser comments'))];
        h.element('comments').dispatchEvent(new Event('change'));
        await until(() => context.library.commentsFor(0).instrumentNames === 'chooser comments');
        const commentsRevision = context.commentsRevision;
        const read = deferred(), index = fakeFile('slow/index.csv', indexCsv);
        index.arrayBuffer = () => read.promise;
        h.chooseFolder([index]); await new Promise(resolve => setTimeout(resolve, 5));
        assertStillLive(h, context, 'during delayed index');
        assert.equal(context.commentsRevision, commentsRevision);
        read.resolve(new TextEncoder().encode('source_path\n"unterminated').buffer);
        await until(() => h.element('selection-status').textContent.includes('Unterminated'));
        assertStillLive(h, context, 'after invalid index');
        h.chooseIndex(fakeFile('remote.csv', 'source_path,source_url\nlive.mod,not-a-url'));
        await until(() => !h.element('stream').hidden); h.element('stream').click();
        await until(() => h.element('selection-status').textContent.includes('No usable source_url'));
        assertStillLive(h, context, 'after remote attach failure');
        assert.equal(context.library.commentsFor(0).instrumentNames, 'chooser comments');
        assert.equal(context.acceptedGeneration, generation); assert.equal(clears, 0);
        h.element('browse-folder').click(); await until(() => h.state.library !== context);
        assert.equal(h.contexts.length, 2); assert.equal(context.discovery.disposed, 1); assert.equal(clears, 1);
        assert.equal(h.state.library.acceptedGeneration, generation + 1);
        const updates = h.updates.length; context.discovery.emit(); assert.equal(h.updates.length, updates);
    } finally { h.close(); }
});

test('Browse identity rejection and candidate construction failure preserve the accepted context', async () => {
    let failIdentity = false, failConstruction = false, rejectedCandidate;
    const h = await controlHarness({
        identityForFolder: async source => { if (failIdentity) throw new Error('identity denied'); return { identity: { name: source.label, sha256 }, storageFailed: false }; },
        discoveryFactory: options => {
            const controller = emittingDiscovery(options);
            if (failConstruction) { rejectedCandidate = options.context; throw new Error('controller construction failed'); }
            return controller;
        },
    });
    try {
        h.chooseFolder([fakeFile('accepted/live.mod')]); await until(() => h.state.library);
        const context = h.state.library; let clears = 0; context.resolver.clear = () => clears++;
        failIdentity = true; h.element('browse-folder').click();
        await until(() => h.element('selection-status').textContent === 'identity denied');
        assertStillLive(h, context, 'identity failed');
        failIdentity = false; failConstruction = true; h.element('browse-folder').click();
        await until(() => h.element('selection-status').textContent === 'controller construction failed');
        assertStillLive(h, context, 'constructor failed');
        assert.equal(clears, 0); assert.equal(h.contexts.length, 1);
        assert.equal(rejectedCandidate.discovery.starts, 0); assert.equal(rejectedCandidate.discovery.disposed, 1);
    } finally { h.close(); }
});

test('a newer index selection overtakes old reads without silencing accepted updates', async () => {
    const h = await controlHarness({ discoveryFactory: emittingDiscovery });
    try {
        h.chooseFolder([fakeFile('accepted/live.mod')]); await until(() => h.state.library);
        const context = h.state.library, read = deferred(), index = fakeFile('old/index.csv', indexCsv);
        index.arrayBuffer = () => read.promise;
        h.chooseFolder([index]); await new Promise(resolve => setTimeout(resolve, 5));
        h.chooseIndex(fakeFile('new.csv', indexCsv));
        await until(() => h.element('selection-status').textContent.includes('new.csv selected'));
        assertStillLive(h, context, 'new pending index');
        read.resolve(new TextEncoder().encode(indexCsv).buffer);
        await new Promise(resolve => setTimeout(resolve, 5));
        assertStillLive(h, context, 'old read ignored');
        assert.equal(h.contexts.length, 1); assert.match(h.element('selection-status').textContent, /new.csv selected/);
        h.chooseFolder([fakeFile('next/live.mod')]); await until(() => h.state.library !== context);
        assert.equal(h.state.library.library.rows[0].sourcePath, 'song.mod');
    } finally { h.close(); }
});

test('pending, denied and cancelled pickers preserve accepted comments and emitted updates', async () => {
    let picker = deferred();
    const h = await controlHarness({ discoveryFactory: emittingDiscovery, directoryPicker: () => picker.promise });
    try {
        h.chooseFolder([fakeFile('accepted/live.mod')]); await until(() => h.state.library);
        const context = h.state.library;
        h.element('folder').click(); assertStillLive(h, context, 'picker pending');
        h.element('comments').files = [fakeFile('picker-comments.csv', commentsCsv('picker comments'))];
        h.element('comments').dispatchEvent(new Event('change'));
        await until(() => context.library.commentsFor(0).instrumentNames === 'picker comments');
        picker.reject(new DOMException('denied', 'NotAllowedError'));
        await until(() => h.element('selection-status').textContent === 'denied');
        assertStillLive(h, context, 'picker denied');
        picker = deferred(); h.element('folder').click(); picker.reject(new DOMException('cancel', 'AbortError'));
        await new Promise(resolve => setTimeout(resolve, 5));
        assertStillLive(h, context, 'picker cancelled');
        assert.equal(context.library.commentsFor(0).instrumentNames, 'picker comments');
        assert.equal(h.element('selection-status').textContent, '');
    } finally { h.close(); }
});

test('manual sidecar reads cannot attach after acceptance and automatic sidecars survive pending selections', async () => {
    const autoRead = deferred(), auto = fakeFile('accepted/comments.csv', commentsCsv('automatic'));
    auto.arrayBuffer = () => autoRead.promise;
    const h = await controlHarness({ discoveryFactory: emittingDiscovery });
    try {
        h.chooseFolder([fakeFile('accepted/live.mod'), auto]); await until(() => h.state.library?.commentsLoading);
        const context = h.state.library;
        h.chooseFolder([fakeFile('candidate/a.csv', indexCsv), fakeFile('candidate/b.csv', indexCsv)]);
        await until(() => !h.element('choices').hidden);
        autoRead.resolve(new TextEncoder().encode(commentsCsv('automatic')).buffer);
        await until(() => context.library.commentsFor(0).instrumentNames === 'automatic');
        assert.equal(h.element('selection-status').textContent, 'Choose a root index CSV.');
        const manual = deferred();
        h.element('comments').files = [{ name: 'late.csv', arrayBuffer: () => manual.promise }];
        h.element('comments').dispatchEvent(new Event('change'));
        h.element('browse-folder').click(); await until(() => h.state.library !== context);
        const replacement = h.state.library;
        manual.resolve(new TextEncoder().encode(commentsCsv('stale manual')).buffer);
        await new Promise(resolve => setTimeout(resolve, 5));
        assert.equal(replacement.library.hasComments, false);
        assert.equal(context.library.commentsFor(0).instrumentNames, 'automatic');
        assert.equal(replacement.discovery.comments, 0);
    } finally { h.close(); }
});

test('a deferred automatic sidecar is neither suppressed by a pending selection nor started after replacement', async () => {
    const frames = [];
    const h = await controlHarness({ discoveryFactory: emittingDiscovery });
    globalThis.requestAnimationFrame = callback => frames.push(callback);
    try {
        const auto = fakeFile('accepted/comments.csv', commentsCsv('deferred automatic'));
        h.chooseFolder([fakeFile('accepted/live.mod'), auto]); await until(() => h.state.library);
        const context = h.state.library;
        h.chooseFolder([fakeFile('candidate/a.csv', indexCsv), fakeFile('candidate/b.csv', indexCsv)]);
        await until(() => !h.element('choices').hidden);
        frames.shift()();
        await until(() => context.library.commentsFor(0).instrumentNames === 'deferred automatic');
        h.element('browse-folder').click(); await until(() => h.state.library !== context);
        const middle = h.state.library;
        h.chooseFolder([fakeFile('last/live.mod')]); await until(() => h.state.library !== middle);
        for (const frame of frames.splice(0)) frame();
        await new Promise(resolve => setTimeout(resolve, 5));
        assert.equal(middle.discovery.comments, 0, 'replaced context cannot start its deferred sidecar');
    } finally { h.close(); }
});

function directoryHandle(name, records = []) {
    return { name, kind: 'directory', permissionRequests: 0,
        async *entries() { for (const file of records) yield [file.name, { kind: 'file', getFile: async () => file }]; },
        async getFileHandle(name) { const file = records.find(file => file.name === name); return { getFile: async () => file }; },
        async requestPermission() { this.permissionRequests += 1; return 'granted'; },
    };
}

test('reconnect retains the last accepted handle through pending chooser/identity and constructor failures', async () => {
    const first = directoryHandle('accepted'), pending = directoryHandle('pending', [fakeFile('a.csv', indexCsv), fakeFile('b.csv', indexCsv)]);
    const second = directoryHandle('second'), identity = deferred();
    let chosen = first, rejectConstruction = false;
    const h = await controlHarness({ directoryPicker: async () => chosen,
        identityForFolder: source => source.handle === second ? identity.promise : Promise.resolve({ identity: { name: source.label, sha256 }, storageFailed: false }),
        discoveryFactory: options => { const controller = emittingDiscovery(options); if (rejectConstruction) throw new Error('construction rejected'); return controller; },
    });
    try {
        h.element('folder').click(); await until(() => h.state.library);
        const context = h.state.library;
        assert.equal(h.element('reconnect').textContent, 'Reconnect accepted');
        chosen = pending; h.element('folder').click(); await until(() => !h.element('choices').hidden);
        assert.equal(h.element('reconnect').textContent, 'Reconnect accepted');
        assertStillLive(h, context, 'directory chooser');
        chosen = second; h.element('folder').click(); await new Promise(resolve => setTimeout(resolve, 5));
        assert.equal(h.element('reconnect').textContent, 'Reconnect accepted');
        assertStillLive(h, context, 'directory identity');
        rejectConstruction = true;
        identity.resolve({ identity: { name: 'second', sha256 }, storageFailed: false });
        await until(() => h.element('selection-status').textContent === 'construction rejected');
        assert.equal(h.element('reconnect').textContent, 'Reconnect accepted');
        assertStillLive(h, context, 'directory constructor');
        rejectConstruction = false;
        h.element('reconnect').click(); await until(() => h.state.library !== context);
        assert.equal(first.permissionRequests, 1, 'reconnect uses the accepted handle');
        assert.equal(second.permissionRequests, 0); assert.equal(pending.permissionRequests, 0);
        chosen = second; h.element('folder').click(); await until(() => h.state.library.source.handle === second);
        assert.equal(h.element('reconnect').textContent, 'Reconnect second');
        assert.equal(h.state.library.discovery.starts, 1);
    } finally { h.close(); }
});

test('chooser reads use their captured attempt and commit once only after parsing and identity finish', async () => {
    const h = await controlHarness({ discoveryFactory: emittingDiscovery });
    try {
        h.chooseFolder([fakeFile('accepted/live.mod')]); await until(() => h.state.library);
        const context = h.state.library, read = deferred(), candidate = fakeFile('candidate/a.csv', indexCsv);
        candidate.arrayBuffer = () => read.promise;
        h.chooseFolder([candidate, fakeFile('candidate/b.csv', indexCsv)]);
        await until(() => !h.element('choices').hidden);
        h.element('choices').value = 'a.csv'; h.element('choices').dispatchEvent(new Event('change'));
        assertStillLive(h, context, 'chooser read pending');
        assert.equal(h.contexts.length, 1);
        read.resolve(new TextEncoder().encode(indexCsv).buffer);
        await until(() => h.state.library !== context);
        assert.equal(h.state.library.mode, 'indexed'); assert.equal(h.state.library.library.rows[0].sourcePath, 'song.mod');
        assert.equal(h.contexts.length, 2); assert.equal(context.discovery.disposed, 1);
        assert.equal(h.element('selection-status').hidden, true);
        const updates = h.updates.length; context.discovery.emit(); assert.equal(h.updates.length, updates);
    } finally { h.close(); }
});

test('a prepared candidate overtaken during construction disposes only candidate resources', async () => {
    let overtake = false, candidate, candidateClears = 0;
    const h = await controlHarness({ discoveryFactory: options => {
        const controller = emittingDiscovery(options);
        if (overtake) {
            candidate = options.context;
            candidate.resolver.clear = () => candidateClears++;
            h.chooseIndex(fakeFile('latest.csv', indexCsv));
        }
        return controller;
    } });
    try {
        h.chooseFolder([fakeFile('accepted/live.mod')]); await until(() => h.state.library);
        const context = h.state.library; let acceptedClears = 0; context.resolver.clear = () => acceptedClears++;
        overtake = true; h.element('browse-folder').click();
        await until(() => h.element('selection-status').textContent.includes('latest.csv selected'));
        assertStillLive(h, context, 'candidate lost commit guard');
        assert.equal(candidate.discovery.starts, 0); assert.equal(candidate.discovery.disposed, 1);
        assert.equal(candidateClears, 1); assert.equal(acceptedClears, 0); assert.equal(h.contexts.length, 1);
    } finally { h.close(); }
});

test('chooser alternatives remain available during pending and invalid choices, with last choice winning', async () => {
    const h = await controlHarness({ discoveryFactory: emittingDiscovery });
    try {
        h.chooseFolder([fakeFile('accepted/live.mod')]); await until(() => h.state.library);
        const context = h.state.library, slowRead = deferred(), slow = fakeFile('candidate/slow.csv', indexCsv);
        slow.arrayBuffer = () => slowRead.promise;
        const choose = value => { h.element('choices').value = value; h.element('choices').dispatchEvent(new Event('change')); };
        h.chooseFolder([slow, fakeFile('candidate/broken.csv', 'source_path\n"unterminated'), fakeFile('candidate/good.csv', indexCsv)]);
        await until(() => !h.element('choices').hidden);
        choose('slow.csv'); await new Promise(resolve => setTimeout(resolve, 5));
        assert.equal(h.element('choices').hidden, false); assertStillLive(h, context, 'first choice pending');
        choose('broken.csv'); await until(() => h.element('selection-status').textContent.includes('Unterminated'));
        assert.equal(h.element('choices').hidden, false); assertStillLive(h, context, 'second choice invalid');
        choose('good.csv'); await until(() => h.state.library !== context);
        const accepted = h.state.library;
        assert.equal(accepted.library.identityName, 'good.csv'); assert.equal(h.contexts.length, 2);
        slowRead.resolve(new TextEncoder().encode('source_path\nstale.mod').buffer);
        await new Promise(resolve => setTimeout(resolve, 5));
        assert.equal(h.state.library, accepted); assert.equal(h.contexts.length, 2);
        assert.equal(context.discovery.disposed, 1); assert.equal(h.element('choices').hidden, true);
    } finally { h.close(); }
});

test('manual invalid index retains a pending chooser folder for Browse before any acceptance', async () => {
    const h = await controlHarness();
    try {
        h.chooseFolder([fakeFile('candidate/a.csv', indexCsv), fakeFile('candidate/b.csv', indexCsv), fakeFile('candidate/right.mod')]);
        await until(() => !h.element('choices').hidden);
        assert.equal(h.state.library, undefined);
        const manualRead = deferred();
        h.chooseIndex({ name: 'invalid.csv', arrayBuffer: () => manualRead.promise });
        assert.equal(h.element('browse-folder').hidden, false);
        manualRead.resolve(new TextEncoder().encode('wrong_header\nvalue').buffer);
        await until(() => h.element('selection-status').textContent.includes('invalid.csv selected'));
        assert.equal(h.element('browse-folder').hidden, false);
        h.element('browse-folder').click(); await until(() => h.state.library?.discoveryStatus?.phase === 'complete');
        assert.equal(h.state.library.mode, 'discovered'); assert.equal(h.state.library.source.label, 'candidate');
        assert.equal(h.state.library.library.rows[0].sourcePath, 'right.mod');
        assert.equal(h.contexts.length, 1);
    } finally { h.close(); }
});

test('manual invalid index retains pending folder B instead of accepted folder A for Browse', async () => {
    const h = await controlHarness();
    try {
        h.chooseFolder([fakeFile('accepted/old.mod')]); await until(() => h.state.library?.discoveryStatus?.phase === 'complete');
        const accepted = h.state.library;
        h.chooseFolder([fakeFile('pending/a.csv', indexCsv), fakeFile('pending/b.csv', indexCsv), fakeFile('pending/right.mod')]);
        await until(() => !h.element('choices').hidden);
        h.chooseIndex(fakeFile('invalid.csv', 'wrong_header\nvalue'));
        await until(() => h.element('selection-status').textContent.includes('invalid.csv selected'));
        assert.equal(h.state.library, accepted); assert.equal(h.element('browse-folder').hidden, false);
        h.element('browse-folder').click(); await until(() => h.state.library !== accepted && h.state.library?.discoveryStatus?.phase === 'complete');
        assert.equal(h.state.library.mode, 'discovered'); assert.equal(h.state.library.source.label, 'pending');
        assert.equal(h.state.library.library.rows[0].sourcePath, 'right.mod');
        assert.equal(h.contexts.length, 2); assert.equal(h.runtimes[0].disposed, true);
    } finally { h.close(); }
});
