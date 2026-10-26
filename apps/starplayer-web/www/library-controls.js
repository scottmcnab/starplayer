import { parseCsv, parseLibraryCsv, libraryIdentity, parseCommentsCsv, attachComments, createDiscoveredLibrary } from './library-index.js';
import { defaultProxyFor } from './remote-sources.js';
import { createResolver } from './track-resolver.js';
import { createFileListSource, createHttpSource, createRemoteSource, normalizeProxy, storedRemoteIndex, directoryPickerAvailable, openDirectorySource, reconnectDirectorySource, storedDirectoryHandle, folderIdentity } from './library-source.js';
import { createDiscoveryController } from './library-discovery.js';

// Optional notification keeps source state usable independently of the view.
export async function wireLibraryControls({ state, loader, onLibrary = () => {}, onComments = () => {}, onLibraryUpdate = () => {},
    createDiscovery = createDiscoveryController, identityForFolder = folderIdentity }) {
    const element = name => document.getElementById(`library-${name}`);
    const summary = element('summary');
    const folder = element('folder');
    const files = element('files');
    const reconnect = element('reconnect');
    const server = element('server');
    const indexInput = element('index');
    const choices = element('choices');
    const commentsInput = element('comments');
    const commentsButton = element('open-comments');
    const stream = element('stream');
    const proxyInput = element('proxy');
    const proxyField = element('proxy-field');
    const reopen = element('reopen');
    const savedProxy = () => { try { return localStorage.getItem('starplayer.libraryProxy'); } catch { return null; } };
    proxyInput.value = savedProxy() ?? '';
    const browse = element('browse-folder');
    const local = source => source?.kind === 'directory' || source?.kind === 'filelist';
    let restored = null;
    let storageQueue = Promise.resolve();
    let indexSummary = '';
    let commentsSummary = '';
    function describeComments(context) {
        const info = context.commentsInfo;
        const { joined, unmatched, awaitingHashes } = context.library.commentCounts;
        return `${info.size.toLocaleString('en-US')} modules from ${info.name}, ${joined.toLocaleString('en-US')} tracks matched, ${info.issues} issues, ${unmatched} unmatched${awaitingHashes ? `, ${awaitingHashes} awaiting metadata/hashes` : ''}${info.manual ? '. This choice lasts until the page reloads' : ''}`;
    }
    const renderSummary = () => {
        const context = state.library;
        if (context?.mode === 'discovered') {
            const status = context.discoveryStatus;
            indexSummary = `${status.tracksFound.toLocaleString('en-US')} tracks found, discovery ${status.phase}, ${status.filesExamined.toLocaleString('en-US')} files examined, ${status.archivesPending} archives pending, ${status.issueCount} issues, from ${context.source.label}`;
            if (context.commentsInfo && !context.commentsLoading && !context.commentsError) commentsSummary = describeComments(context);
            if (context.source.kind === 'filelist') indexSummary += '. Choose this folder again after reloading';
        }
        summary.textContent = indexSummary + (commentsSummary ? `. Comments: ${commentsSummary}` : '') + (state.library?.storageFailed ? ". This library won't be offered after a reload (browser storage unavailable)." : ''); };
    const remoteControls = offered => {
        stream.hidden = !offered;
        proxyField.hidden = !offered && state.library?.source.kind !== 'remote';
    };
    function saveRemote(context) {
        if (!context.record) return Promise.resolve();
        const record = { ...context.record, proxy: context.source.proxy };
        storageQueue = storageQueue.then(async () => {
            if (context !== state.library) return;
            try {
                await storedRemoteIndex(record);
                restored = record;
                reopen.textContent = `Reopen ${record.name} (streaming)`;
                reopen.hidden = false;
                context.storageFailed = false;
            } catch { context.storageFailed = true; }
            if (context === state.library) renderSummary();
        });
        return storageQueue;
    }
    let commentsRevision = 0;
    const showComments = message => { commentsSummary = message; renderSummary(); };
    async function loadComments(context, name, read, currentRevision, manual = false) {
        const currentCommentsRevision = ++commentsRevision;
        const current = () => currentRevision === revision && currentCommentsRevision === commentsRevision && context === state.library;
        context.commentsLoading = true; context.commentsError = '';
        showComments(`loading ${name}…`);
        try {
            const bytes = await read();
            if (!current()) return;
            const parsed = parseCommentsCsv(new TextDecoder().decode(bytes));
            const { joined, unmatched } = context.discovery ? context.discovery.attachComments(parsed.comments) : attachComments(context.library, parsed.comments);
            context.commentsLoading = false;
            if (context.discovery) context.commentsInfo = { name, size: parsed.comments.size, issues: parsed.issues.length, manual };
            showComments(`${parsed.comments.size.toLocaleString('en-US')} modules from ${name}, ${joined.toLocaleString('en-US')} tracks matched, ${parsed.issues.length} issues, ${unmatched} unmatched${manual && context.source.kind !== 'remote' ? '. This choice lasts until the page reloads' : ''}`);
            if (context.record) {
                context.record = { ...context.record, commentsName: name, commentsBytes: bytes };
                saveRemote(context);
            }
            if (!context.discovery) onComments(context);
        } catch (error) { if (current()) { context.commentsLoading = false; context.commentsError = error.message; showComments(`not loaded (${error.message})`); } }
    }
    async function autoComments(context, currentRevision) {
        const source = context.source;
        try {
            if (source.kind === 'remote') { showComments('use Open comments CSV…'); return; }
            if (source.kind === 'http') {
                const response = await fetch('/library-comments.csv', { method: 'HEAD' });
                if (currentRevision !== revision || context !== state.library) return;
                // A manual attachment made during the probe takes precedence.
                if (context.manualComments) return;
                if (response.status !== 200) { showComments(response.status === 404 ? 'none on the server (start it with --library-comments FILE)' : `not loaded (HTTP ${response.status})`); return; }
                await loadComments(context, 'library-comments.csv', async () => {
                    const response = await fetch('/library-comments.csv');
                    if (!response.ok) throw new Error(`HTTP ${response.status}`);
                    return response.arrayBuffer();
                }, currentRevision);
            } else if (source.commentsPath) {
                await loadComments(context, source.commentsPath, () => source.readFile(source.commentsPath), currentRevision);
            } else showComments(source.commentsCandidates.length > 1 ? 'not loaded (several comments CSVs; use Open comments CSV…)' : 'not loaded (no comments CSV found)');
        } catch (error) { if (currentRevision === revision && context === state.library && !context.manualComments) showComments(`not loaded (${error.message})`); }
    }
    let storedHandle = null;
    let pendingIndex = null;
    let selectedSource = null;
    let revision = 0;
    let openingRevision = 0;
    let indexRevision = 0;
    const report = error => { if (error.name !== 'AbortError') summary.textContent = error.message; };
    const updateBrowse = source => { if (browse) browse.hidden = !local(source); };
    function disposeCurrent() { state.library?.discovery?.dispose(); state.library?.resolver.clear(); }
    function deferComments(context, currentRevision) {
        requestAnimationFrame(() => setTimeout(() => {
            if (currentRevision === revision && context === state.library && !context.manualComments) autoComments(context, currentRevision);
        }, 0));
    }
    async function discover(source, currentRevision) {
        let folder;
        try { folder = await identityForFolder(source); }
        catch (error) { if (currentRevision === revision) report(error); return; }
        const { identity, storageFailed } = folder;
        if (currentRevision !== revision) return;
        disposeCurrent();
        const context = { mode: 'discovered', source, library: createDiscoveredLibrary({ name: source.label }), identity, storageFailed, resolver: createResolver({ source, loader }) };
        createDiscovery({ context, generation: currentRevision, onUpdate(updated, change) {
            if (currentRevision !== revision || updated !== state.library) return;
            renderSummary(); onLibraryUpdate(updated, change);
        } });
        state.library = context;
        remoteControls(false); choices.hidden = true; updateBrowse(source);
        commentsSummary = 'not loaded (checking for a sidecar)'; renderSummary();
        commentsButton.disabled = false; onLibrary(context);
        context.discovery.start(); deferComments(context, currentRevision);
    }
    function browseFolder() {
        const source = selectedSource || state.library?.source;
        if (!local(source)) return;
        ++openingRevision; ++indexRevision; ++commentsRevision; pendingIndex = null;
        indexInput.value = ''; remoteControls(false); choices.hidden = true;
        disposeCurrent();
        discover(source, ++revision).catch(report);
    }
    function describeIndex({ source, library, streaming }) {
        let text = `${library.rows.length.toLocaleString('en-US')} tracks, ${library.issues.length} issues`;
        if (streaming) text += `; streaming ${streaming.streamable.toLocaleString('en-US')} tracks from ${source.label}${streaming.unmatched ? `; ${streaming.unmatched} rows have no matching source_url` : ''}`;
        else text += `, from ${source.label}`;
        if (library.issues.length) text += ': ' + library.issues.slice(0, 3).map(issue => `line ${issue.line}: ${issue.reason}`).join('; ');
        if (source.kind === 'filelist') text += '. Choose this folder again after reloading.';
        return text.replace(/\.$/, '');
    }
    async function loadIndex(source, index, currentRevision) {
        try {
            const bytes = await index.bytes();
            if (currentRevision !== revision) return;
            const library = parseLibraryCsv(new TextDecoder().decode(bytes), { name: index.name });
            const identity = await libraryIdentity(bytes, index.name);
            if (currentRevision !== revision) return;
            const streaming = source.kind === 'remote' ? source.attach(library) : null;
            if (streaming) {
                if (!source.proxy && !proxyInput.value && savedProxy() === null) source.setProxy(defaultProxyFor(streaming.base));
                proxyInput.value = source.proxy;
            }
            disposeCurrent();
            state.library = { mode: 'indexed', source, library, identity, resolver: createResolver({ source, loader }),
                ...(streaming ? { streaming, record: { kind: 'remote', name: index.name, bytes, proxy: source.proxy } } : {}) };
            if (pendingIndex === index) pendingIndex = null;
            remoteControls(false); updateBrowse(source);
            indexSummary = describeIndex(state.library);
            showComments('not loaded (checking for a sidecar)');
            commentsButton.disabled = false;
            onLibrary(state.library);
            if (streaming) saveRemote(state.library);
            deferComments(state.library, currentRevision);
        } catch (error) {
            if (currentRevision !== revision) return;
            updateBrowse(source);
            summary.textContent = `${error.message}${local(source) ? '. Choose Browse folder instead to continue without this index.' : ''}`;
        }
    }
    async function selectSource(opening) {
        // A picker that is cancelled or overtaken must not disturb the open library or its
        // pending sidecar, so it has its own counter until it yields a source.
        const currentOpening = ++openingRevision;
        let source;
        try { source = await opening; }
        catch (error) { if (currentOpening === openingRevision) report(error); return; }
        if (currentOpening !== openingRevision) return;
        const currentRevision = ++revision;
        selectedSource = source;
        ++commentsRevision;
        disposeCurrent();
        updateBrowse(source);
        choices.hidden = true;
        if (source.handle) {
            storedHandle = source.handle;
            reconnect.textContent = `Reconnect ${source.label}`;
            reconnect.hidden = false;
            // Storage failures must not prevent using the current selection.
            storedDirectoryHandle(source.handle).catch(() => {});
        }
        if (pendingIndex) {
            const index = pendingIndex;
            await loadIndex(source, index, currentRevision);
        } else if (source.indexPath) {
            await loadIndex(source, { name: source.indexPath, bytes: () => source.kind === 'http' ? fetchIndex() : source.readFile(source.indexPath) }, currentRevision);
        } else if (source.candidates?.length) {
            choices.replaceChildren(new Option('Choose index CSV…', ''), ...source.candidates.map(path => new Option(path, path)), new Option('Browse folder without an index', '__browse__'));
            choices.hidden = false;
            summary.textContent = 'Choose a root index CSV.';
        } else if (local(source)) await discover(source, currentRevision);
        else summary.textContent = 'No root CSV found. Use Open index CSV…';
    }
    async function fetchIndex() {
        const response = await fetch('/library-index.csv');
        if (!response.ok) throw new Error(`Library index: HTTP ${response.status}`);
        return response.arrayBuffer();
    }
    browse?.addEventListener('click', browseFolder);
    for (const [name, method] of [['stop-discovery', 'pause'], ['resume-discovery', 'resume'], ['retry-discovery', 'retry'], ['scan-metadata', 'scanMetadata'], ['cancel-metadata', 'cancelMetadata']]) {
        element(name)?.addEventListener('click', () => state.library?.discovery?.[method]());
    }
    folder.addEventListener('click', () => {
        // Do not insert an await before opening the picker.
        if (directoryPickerAvailable()) selectSource(openDirectorySource()).catch(report);
        else files.click();
    });
    reconnect.addEventListener('click', () => selectSource(reconnectDirectorySource(storedHandle)).catch(report));
    files.addEventListener('change', () => {
        if (files.files.length) selectSource(createFileListSource(files.files)).catch(report);
        files.value = '';
    });
    element('open-index').addEventListener('click', () => indexInput.click());
    indexInput.addEventListener('change', async () => {
        const file = indexInput.files[0];
        indexInput.value = '';
        if (!file) return;
        const currentIndexRevision = ++indexRevision;
        pendingIndex = null;
        remoteControls(false);
        try {
            const bytes = await file.arrayBuffer();
            if (currentIndexRevision !== indexRevision) return;
            pendingIndex = { name: file.name, bytes: () => Promise.resolve(bytes) };
            choices.hidden = true;
            const header = new TextDecoder().decode(bytes).split('\n')[0].replace(/\r$/, '');
            const streamable = (parseCsv(header)[0] || []).some(column => column.trim().toLowerCase() === 'source_url');
            remoteControls(streamable);
            updateBrowse(selectedSource || state.library?.source);
            summary.textContent = `${file.name} selected. Click Open library folder to choose its root${streamable ? ', or Stream from source URLs to fetch each track over the network' : ''}.`;
        } catch (error) { if (currentIndexRevision === indexRevision) report(error); }
    });
    stream.addEventListener('click', () => {
        if (pendingIndex) {
            try { selectSource(createRemoteSource({ proxy: proxyInput.value })).catch(report); } catch (error) { report(error); }
        }
    });
    proxyInput.addEventListener('change', () => {
        try {
            const proxy = normalizeProxy(proxyInput.value);
            proxyInput.value = proxy;
            try { localStorage.setItem('starplayer.libraryProxy', proxy); } catch { /* Private browsing may deny storage. */ }
            const context = state.library;
            if (context?.source.kind === 'remote') {
                context.source.setProxy(proxy);
                context.resolver.clear();
                indexSummary = describeIndex(context);
                renderSummary();
                saveRemote(context);
            }
        } catch (error) { report(error); }
    });
    reopen.addEventListener('click', async () => {
        if (!restored) return;
        try {
            const record = restored;
            ++indexRevision;
            proxyInput.value = savedProxy() ?? record.proxy;
            pendingIndex = { name: record.name, bytes: async () => record.bytes };
            const source = createRemoteSource({ proxy: proxyInput.value });
            await selectSource(source);
            const context = state.library;
            if (context?.source === source && record.commentsBytes) {
                context.manualComments = true;
                await loadComments(context, record.commentsName, async () => record.commentsBytes, revision);
            }
        } catch (error) { report(error); }
    });
    commentsButton.addEventListener('click', () => commentsInput.click());
    commentsInput.addEventListener('change', () => {
        const file = commentsInput.files[0];
        commentsInput.value = '';
        if (!file || !state.library) return;
        state.library.manualComments = true;
        loadComments(state.library, file.name, () => file.arrayBuffer(), revision, true);
    });
    choices.addEventListener('change', () => {
        const source = selectedSource;
        const path = choices.value;
        if (path === '__browse__') { browseFolder(); return; }
        if (path) loadIndex(source, { name: path, bytes: () => source.readFile(path) }, ++revision).catch(report);
    });
    server.addEventListener('click', () => { ++indexRevision; pendingIndex = null; selectSource(createHttpSource()).catch(report); });
    await Promise.allSettled([
        storedRemoteIndex().then(record => {
            if (record?.kind === 'remote') {
                restored = record;
                reopen.textContent = `Reopen ${record.name} (streaming)`;
                reopen.hidden = false;
            }
        }),
        fetch('/library-index.csv', { method: 'HEAD' }).then(response => { server.hidden = response.status !== 200; }),
        directoryPickerAvailable() ? storedDirectoryHandle().then(handle => {
            if (handle) { storedHandle = handle; reconnect.textContent = `Reconnect ${handle.name}`; reconnect.hidden = false; }
        }) : Promise.resolve(),
    ]);
}
