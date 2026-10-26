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
    const selectionStatus = element('selection-status');
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
    const showComments = message => { commentsSummary = message; renderSummary(); };
    async function loadComments(context, name, read, manual = false) {
        if (!accepted(context)) return;
        const currentCommentsRevision = ++context.commentsRevision;
        const current = () => accepted(context) && currentCommentsRevision === context.commentsRevision;
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
    async function autoComments(context) {
        const source = context.source;
        try {
            if (source.kind === 'remote') { showComments('use Open comments CSV…'); return; }
            if (source.kind === 'http') {
                const response = await fetch('/library-comments.csv', { method: 'HEAD' });
                if (!accepted(context)) return;
                // A manual attachment made during the probe takes precedence.
                if (context.manualComments) return;
                if (response.status !== 200) { showComments(response.status === 404 ? 'none on the server (start it with --library-comments FILE)' : `not loaded (HTTP ${response.status})`); return; }
                await loadComments(context, 'library-comments.csv', async () => {
                    const response = await fetch('/library-comments.csv');
                    if (!response.ok) throw new Error(`HTTP ${response.status}`);
                    return response.arrayBuffer();
                });
            } else if (source.commentsPath) {
                await loadComments(context, source.commentsPath, () => source.readFile(source.commentsPath));
            } else showComments(source.commentsCandidates.length > 1 ? 'not loaded (several comments CSVs; use Open comments CSV…)' : 'not loaded (no comments CSV found)');
        } catch (error) { if (accepted(context) && !context.manualComments) showComments(`not loaded (${error.message})`); }
    }
    let storedHandle = null;
    let pendingIndex = null;
    let selectionEpoch = 0;
    let acceptedGeneration = 0;
    let pendingAttempt = null;
    let chooserAttempt = null;
    const accepted = context => context === state.library && context.acceptedGeneration === acceptedGeneration;
    const current = attempt => attempt.epoch === selectionEpoch;
    const showSelection = message => { selectionStatus.textContent = message; selectionStatus.hidden = !message; };
    const report = error => { if (error.name !== 'AbortError') showSelection(error.message); };
    const updateBrowse = source => { if (browse) browse.hidden = !local(source) && !local(state.library?.source); };
    function beginSelection(source = null) {
        const attempt = { epoch: ++selectionEpoch, source };
        pendingAttempt = attempt; chooserAttempt = null;
        choices.hidden = true; remoteControls(false); showSelection('');
        return attempt;
    }
    function disposeContext(context) { context?.discovery?.dispose(); context?.resolver.clear(); }
    function deferComments(context) {
        requestAnimationFrame(() => setTimeout(() => {
            if (accepted(context) && !context.manualComments) autoComments(context);
        }, 0));
    }
    function commitCandidate(attempt, context) {
        if (!current(attempt)) { disposeContext(context); return null; }
        const previous = state.library;
        context.acceptedGeneration = ++acceptedGeneration;
        context.commentsRevision = 0;
        state.library = context;
        disposeContext(previous);
        pendingAttempt = null; chooserAttempt = null;
        remoteControls(false); choices.hidden = true; updateBrowse(context.source); showSelection('');
        if (context.source.kind === 'remote') proxyInput.value = context.source.proxy;
        if (context.source.handle) {
            storedHandle = context.source.handle;
            reconnect.textContent = `Reconnect ${context.source.label}`;
            reconnect.hidden = false;
            // Only an accepted source becomes the reconnect target.
            storedDirectoryHandle(storedHandle).catch(() => {});
        }
        indexSummary = context.mode === 'indexed' ? describeIndex(context) : '';
        commentsSummary = 'not loaded (checking for a sidecar)'; renderSummary();
        commentsButton.disabled = false; onLibrary(context);
        context.discovery?.start();
        if (context.streaming) saveRemote(context);
        deferComments(context);
        return context;
    }
    async function discover(source, attempt) {
        let context;
        try {
            const { identity, storageFailed } = await identityForFolder(source);
            if (!current(attempt)) return;
            const generation = acceptedGeneration + 1;
            context = { mode: 'discovered', source, library: createDiscoveredLibrary({ name: source.label }), identity, storageFailed, resolver: createResolver({ source, loader }) };
            createDiscovery({ context, generation, onUpdate(updated, change) {
                if (updated !== context || !accepted(context) || generation !== acceptedGeneration) return;
                renderSummary(); onLibraryUpdate(updated, change);
            } });
            return commitCandidate(attempt, context);
        } catch (error) {
            if (context && context !== state.library) disposeContext(context);
            if (current(attempt)) report(error);
        }
    }
    function browseFolder() {
        const source = local(pendingAttempt?.source) ? pendingAttempt.source : state.library?.source;
        if (!local(source)) return;
        pendingIndex = null; indexInput.value = '';
        const attempt = beginSelection(source);
        discover(source, attempt);
    }
    function describeIndex({ source, library, streaming }) {
        let text = `${library.rows.length.toLocaleString('en-US')} tracks, ${library.issues.length} issues`;
        if (streaming) text += `; streaming ${streaming.streamable.toLocaleString('en-US')} tracks from ${source.label}${streaming.unmatched ? `; ${streaming.unmatched} rows have no matching source_url` : ''}`;
        else text += `, from ${source.label}`;
        if (library.issues.length) text += ': ' + library.issues.slice(0, 3).map(issue => `line ${issue.line}: ${issue.reason}`).join('; ');
        if (source.kind === 'filelist') text += '. Choose this folder again after reloading.';
        return text.replace(/\.$/, '');
    }
    async function loadIndex(source, index, attempt) {
        let context;
        try {
            const bytes = await index.bytes();
            if (!current(attempt)) return;
            const library = parseLibraryCsv(new TextDecoder().decode(bytes), { name: index.name });
            const identity = await libraryIdentity(bytes, index.name);
            if (!current(attempt)) return;
            const streaming = source.kind === 'remote' ? source.attach(library) : null;
            if (streaming && !source.proxy && !proxyInput.value && savedProxy() === null) source.setProxy(defaultProxyFor(streaming.base));
            context = { mode: 'indexed', source, library, identity, resolver: createResolver({ source, loader }),
                ...(streaming ? { streaming, record: { kind: 'remote', name: index.name, bytes, proxy: source.proxy } } : {}) };
            const committed = commitCandidate(attempt, context);
            if (committed && pendingIndex === index) pendingIndex = null;
            return committed;
        } catch (error) {
            if (context && context !== state.library) disposeContext(context);
            if (!current(attempt)) return;
            updateBrowse(source);
            showSelection(`${error.message}${local(source) ? '. Choose Browse folder instead to continue without this index.' : ''}`);
        }
    }
    async function selectSource(opening) {
        // Opening the picker stays in the click's user activation. Its result belongs
        // only to this pending attempt until the complete candidate can commit.
        const attempt = beginSelection();
        let source;
        try { source = await opening; }
        catch (error) { if (current(attempt)) report(error); return; }
        if (!current(attempt)) return;
        attempt.source = source;
        updateBrowse(source);
        if (pendingIndex) {
            return loadIndex(source, pendingIndex, attempt);
        } else if (source.indexPath) {
            return loadIndex(source, { name: source.indexPath, bytes: () => source.kind === 'http' ? fetchIndex() : source.readFile(source.indexPath) }, attempt);
        } else if (source.candidates?.length) {
            choices.replaceChildren(new Option('Choose index CSV…', ''), ...source.candidates.map(path => new Option(path, path)), new Option('Browse folder without an index', '__browse__'));
            chooserAttempt = attempt; choices.hidden = false;
            showSelection('Choose a root index CSV.');
        } else if (local(source)) return discover(source, attempt);
        else showSelection('No root CSV found. Use Open index CSV…');
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
        const source = local(pendingAttempt?.source) ? pendingAttempt.source : state.library?.source;
        const attempt = beginSelection(source);
        pendingIndex = null;
        try {
            const bytes = await file.arrayBuffer();
            if (!current(attempt)) return;
            pendingIndex = { name: file.name, bytes: () => Promise.resolve(bytes) };
            choices.hidden = true;
            const header = new TextDecoder().decode(bytes).split('\n')[0].replace(/\r$/, '');
            const streamable = (parseCsv(header)[0] || []).some(column => column.trim().toLowerCase() === 'source_url');
            remoteControls(streamable);
            updateBrowse(attempt.source);
            showSelection(`${file.name} selected. Click Open library folder to choose its root${streamable ? ', or Stream from source URLs to fetch each track over the network' : ''}.`);
        } catch (error) { if (current(attempt)) report(error); }
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
            proxyInput.value = savedProxy() ?? record.proxy;
            pendingIndex = { name: record.name, bytes: async () => record.bytes };
            const source = createRemoteSource({ proxy: proxyInput.value });
            const context = await selectSource(source);
            if (context && accepted(context) && record.commentsBytes) {
                context.manualComments = true;
                await loadComments(context, record.commentsName, async () => record.commentsBytes);
            }
        } catch (error) { report(error); }
    });
    commentsButton.addEventListener('click', () => commentsInput.click());
    commentsInput.addEventListener('change', () => {
        const file = commentsInput.files[0];
        commentsInput.value = '';
        if (!file || !state.library) return;
        state.library.manualComments = true;
        loadComments(state.library, file.name, () => file.arrayBuffer(), true);
    });
    choices.addEventListener('change', () => {
        const attempt = chooserAttempt;
        if (!attempt || !current(attempt)) return;
        const source = attempt.source;
        const path = choices.value;
        if (path === '__browse__') { browseFolder(); return; }
        if (path) {
            const nextAttempt = beginSelection(source);
            chooserAttempt = nextAttempt; choices.hidden = false;
            loadIndex(source, { name: path, bytes: () => source.readFile(path) }, nextAttempt);
        }
    });
    server.addEventListener('click', () => { pendingIndex = null; selectSource(createHttpSource()).catch(report); });
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
