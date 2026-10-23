import { parseLibraryCsv, libraryIdentity, parseCommentsCsv, attachComments } from './library-index.js';
import { createResolver } from './track-resolver.js';
import { createFileListSource, createHttpSource, directoryPickerAvailable, openDirectorySource, reconnectDirectorySource, storedDirectoryHandle } from './library-source.js';

// Optional notification keeps source state usable independently of the view.
export async function wireLibraryControls({ state, loader, onLibrary = () => {}, onComments = () => {} }) {
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
    let indexSummary = '';
    let commentsRevision = 0;
    const showComments = message => { summary.textContent = `${indexSummary}. Comments: ${message}`; };
    async function loadComments(context, name, read, currentRevision, manual = false) {
        const currentCommentsRevision = ++commentsRevision;
        const current = () => currentRevision === revision && currentCommentsRevision === commentsRevision && context === state.library;
        showComments(`loading ${name}…`);
        try {
            const bytes = await read();
            if (!current()) return;
            const parsed = parseCommentsCsv(new TextDecoder().decode(bytes));
            const { joined, unmatched } = attachComments(context.library, parsed.comments);
            showComments(`${parsed.comments.size.toLocaleString('en-US')} modules from ${name}, ${joined.toLocaleString('en-US')} tracks matched, ${parsed.issues.length} issues, ${unmatched} unmatched${manual ? '. This choice lasts until the page reloads' : ''}`);
            onComments(context);
        } catch (error) { if (current()) showComments(`not loaded (${error.message})`); }
    }
    async function autoComments(context, currentRevision) {
        const source = context.source;
        try {
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
    const report = error => { summary.textContent = error.name === 'AbortError' ? 'Library selection cancelled.' : error.message; };
    async function loadIndex(source, index, currentRevision) {
        const bytes = await index.bytes();
        const library = parseLibraryCsv(new TextDecoder().decode(bytes), { name: index.name });
        const identity = await libraryIdentity(bytes, index.name);
        if (currentRevision !== revision) return;
        state.library?.resolver.clear();
        state.library = { source, library, identity, resolver: createResolver({ source, loader }) };
        summary.textContent = `${library.rows.length.toLocaleString('en-US')} tracks, ${library.issues.length} issues, from ${source.label}`;
        if (library.issues.length) summary.textContent += ': ' + library.issues.slice(0, 3).map(issue => `line ${issue.line}: ${issue.reason}`).join('; ');
        if (source.kind === 'filelist') summary.textContent += '. Choose this folder again after reloading.';
        indexSummary = summary.textContent.replace(/\.$/, '');
        showComments('not loaded (checking for a sidecar)');
        commentsButton.disabled = false;
        onLibrary(state.library);
        // Let the table paint before even starting the optional sidecar read.
        const context = state.library;
        requestAnimationFrame(() => setTimeout(() => {
            if (currentRevision === revision && context === state.library && !context.manualComments) autoComments(context, currentRevision);
        }, 0));
    }
    async function selectSource(opening) {
        // A picker that is cancelled or overtaken must not disturb the open library or its
        // pending sidecar, so it has its own counter until it yields a source.
        const currentOpening = ++openingRevision;
        const source = await opening;
        if (currentOpening !== openingRevision) return;
        const currentRevision = ++revision;
        selectedSource = source;
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
            pendingIndex = null;
            await loadIndex(source, index, currentRevision);
        } else if (source.indexPath) {
            await loadIndex(source, { name: source.indexPath, bytes: () => source.kind === 'http' ? fetchIndex() : source.readFile(source.indexPath) }, currentRevision);
        } else if (source.candidates.length) {
            choices.replaceChildren(new Option('Choose index CSV…', ''), ...source.candidates.map(path => new Option(path, path)));
            choices.hidden = false;
            summary.textContent = 'Choose a root index CSV.';
        } else summary.textContent = 'No root CSV found. Use Open index CSV…, then choose the library folder again.';
    }
    async function fetchIndex() {
        const response = await fetch('/library-index.csv');
        if (!response.ok) throw new Error(`Library index: HTTP ${response.status}`);
        return response.arrayBuffer();
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
        try {
            const bytes = await file.arrayBuffer();
            if (currentIndexRevision !== indexRevision) return;
            pendingIndex = { name: file.name, bytes: () => Promise.resolve(bytes) };
            choices.hidden = true;
            summary.textContent = `${file.name} selected. Click Open library folder to choose its root.`;
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
        if (path) loadIndex(source, { name: path, bytes: () => source.readFile(path) }, ++revision).catch(report);
    });
    server.addEventListener('click', () => { pendingIndex = null; selectSource(createHttpSource()).catch(report); });
    await Promise.allSettled([
        fetch('/library-index.csv', { method: 'HEAD' }).then(response => { server.hidden = response.status !== 200; }),
        directoryPickerAvailable() ? storedDirectoryHandle().then(handle => {
            if (handle) { storedHandle = handle; reconnect.textContent = `Reconnect ${handle.name}`; reconnect.hidden = false; }
        }) : Promise.resolve(),
    ]);
}
