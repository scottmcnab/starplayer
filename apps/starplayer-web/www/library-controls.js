import { parseLibraryCsv, libraryIdentity } from './library-index.js';
import { createResolver } from './track-resolver.js';
import { createFileListSource, createHttpSource, directoryPickerAvailable, openDirectorySource, reconnectDirectorySource, storedDirectoryHandle } from './library-source.js';

// W8 source controls only; W9 consumes the resulting page state.
export async function wireLibraryControls({ state, loader }) {
    const element = name => document.getElementById(`library-${name}`);
    const summary = element('summary');
    const folder = element('folder');
    const files = element('files');
    const reconnect = element('reconnect');
    const server = element('server');
    const indexInput = element('index');
    const choices = element('choices');
    let storedHandle = null;
    let pendingIndex = null;
    let selectedSource = null;
    let revision = 0;
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
    }
    async function selectSource(source) {
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
        if (directoryPickerAvailable()) openDirectorySource().then(selectSource).catch(report);
        else files.click();
    });
    reconnect.addEventListener('click', () => reconnectDirectorySource(storedHandle).then(selectSource).catch(report));
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
