import { openPlaylistStore } from './playlist-store.js';
import { createResolver } from './track-resolver.js';
import { exportPlaylist, importPlaylist, keysForLibrary, locatorKey, rebindCandidates, rebindEntries } from './playlist-transfer.js';

export async function createPlaylistControls({ playlist, state, loader, ready = () => {}, openStore = openPlaylistStore }) {
    const element = name => document.getElementById(`playlist-${name}`);
    const report = message => { element('status').textContent = message; };
    const store = await openStore({ report });
    playlist.restore(store.state);
    element('loop').checked = store.state.loop;
    const snapshot = () => ({ entries: playlist.entries, currentId: playlist.current?.id ?? null, loop: element('loop').checked });
    let suppressSave = false;
    let render = () => {};
    const save = () => store.save(snapshot()).catch(error => report(error.message));
    playlist.subscribe(() => { if (!suppressSave) save(); updateRebind(); });
    element('loop').addEventListener('change', save);
    const resolver = createResolver({ loader, blobStore: store });
    function availability(entry) {
        if (entry.locator.kind === 'upload') return store.hasBlob(entry.locator.blobId) ? '' : 'missing — re-add';
        const library = state.library;
        if (library?.identity.sha256 !== entry.locator.libraryIdentity?.sha256) return `library not connected — ${entry.locator.libraryIdentity?.name || 'unknown library'}`;
        if (keysForLibrary(library.library).has(locatorKey(entry.locator))) return '';
        if (library.mode === 'discovered') return library.discoveryStatus?.phase === 'complete' ? 'not found in scan' : 'not yet discovered';
        return 'track not in library';
    }
    function canPlay(entry) {
        if (entry.locator.kind === 'upload') return store.hasBlob(entry.locator.blobId);
        const library = state.library;
        if (!library || library.identity.sha256 !== entry.locator.libraryIdentity?.sha256) return false;
        return library.mode === 'discovered' || keysForLibrary(library.library).has(locatorKey(entry.locator));
    }
    function updateRebind() {
        const candidates = rebindCandidates(playlist.entries, state.library);
        const pending = state.library?.mode === 'discovered' && state.library.discoveryStatus?.phase !== 'complete'
            && playlist.entries.some(entry => entry.locator.kind === 'library' && entry.locator.libraryIdentity?.sha256 !== state.library.identity.sha256);
        element('rebind').hidden = !candidates.length && !pending;
        element('rebind').disabled = Boolean(pending);
        element('rebind').title = pending ? `Rebind waits for discovery to complete (currently ${state.library.discoveryStatus?.phase || 'running'}). Resume or retry discovery to finish.` : '';
        element('rebind').textContent = pending ? `Rebind pending — discovery ${state.library.discoveryStatus?.phase || 'running'}` : `Rebind ${candidates.length} entries to ${state.library?.identity.name || ''}`;
    }
    element('rebind').addEventListener('click', () => {
        if (!state.library || (state.library.mode === 'discovered' && state.library.discoveryStatus?.phase !== 'complete')) return;
        playlist.restore({ ...snapshot(), entries: rebindEntries(playlist.entries, rebindCandidates(playlist.entries, state.library), state.library.identity) });
        enrichDisplay(state.library, state.library.library.rows.filter(row => row.metadataState === 'ready').map(row => ({ locator: row, patch: row })));
        render();
    });
    async function addFiles(files) {
        for (const file of files) {
            try {
                if (file.size > 128 * 1024 * 1024) throw new Error(`${file.name}: too large (128 MiB limit)`);
                await ready();
                const bytes = new Uint8Array(await file.arrayBuffer());
                const blobId = crypto.randomUUID();
                const items = [];
                const add = (name, containerIndex, memberIndex) => items.push({ locator: { kind: 'upload', blobId, containerIndex, memberIndex, memberPath: memberIndex < 0 ? '' : name }, display: { name, title: '', format: name.split('.').at(-1).toLowerCase(), folder: file.name } });
                const records = bytes => loader.archive_modules(bytes).split('\n').filter(Boolean).map(record => {
                    const [index, name] = record.split('\t');
                    return { index: Number(index), name };
                });
                if (loader.is_archive(bytes)) {
                    for (const member of records(bytes)) add(member.name, -1, member.index);
                    for (const containerIndex of loader.archive_containers(bytes)) {
                        const inner = loader.archive_extract_nested(bytes, -1, containerIndex);
                        for (const nested of records(inner)) add(nested.name, containerIndex, nested.index);
                    }
                } else {
                    if (!/\.(mod|s3m|mtm|xm|it)$/i.test(file.name)) throw new Error(`${file.name}: unsupported module`);
                    add(file.name, -1, -1);
                }
                if (!items.length) throw new Error(`${file.name}: no playable modules`);
                // Queue the atomic write before publishing; subsequent edits include these ids.
                // A cap refusal rolls back only the new entries.
                const added = items.map(item => ({ ...item, id: crypto.randomUUID() }));
                const next = { ...snapshot(), entries: [...playlist.entries, ...added] };
                const pending = store.save(next, { uploads: [{ blobId, name: file.name, size: file.size, type: file.type, bytes: file, addedAt: Date.now() }] });
                suppressSave = true;
                playlist.restore(next);
                suppressSave = false;
                try { await pending; }
                catch (error) { playlist.remove(added.map(entry => entry.id)); throw error; }
                report(`Added ${added.length} tracks from ${file.name}.${store.sessionOnly ? ' Session-only storage.' : ''}`);
            } catch (error) { report(error.message); }
        }
        render();
    }
    let uploads = Promise.resolve();
    const enqueueFiles = files => { const selected = [...files]; uploads = uploads.then(() => addFiles(selected)).catch(error => report(error.message)); };
    element('add').addEventListener('click', () => element('files').click());
    element('files').accept = document.getElementById('file-picker').accept;
    element('files').addEventListener('change', () => { enqueueFiles(element('files').files); element('files').value = ''; });
    element('panel').addEventListener('dragover', event => { if (event.dataTransfer.types.includes('Files')) event.preventDefault(); });
    element('panel').addEventListener('drop', event => {
        if (!event.dataTransfer.files.length) return;
        event.preventDefault(); event.stopPropagation(); enqueueFiles(event.dataTransfer.files);
    });
    element('export').addEventListener('click', () => {
        const { csv, skipped } = exportPlaylist(playlist.entries);
        const url = URL.createObjectURL(new Blob([csv], { type: 'text/csv;charset=utf-8' }));
        const link = document.createElement('a'); link.href = url; link.download = 'playlist.csv'; link.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
        report(`Exported playlist; skipped ${skipped} uploaded tracks.`);
    });
    element('import').addEventListener('click', () => element('csv').click());
    element('csv').addEventListener('change', async () => {
        const file = element('csv').files[0]; element('csv').value = '';
        if (!file) return;
        try {
            const result = importPlaylist(await file.text(), state.library);
            playlist.add(result.items);
            report(`Imported ${result.items.length} tracks; ${result.unavailable} unavailable; ${result.pending} pending discovery; ${result.issues.length} CSV issues.`);
        } catch (error) { report(error.message); }
    });
    function enrichDisplay(library, updates) {
        if (library !== state.library || library.mode !== 'discovered') return;
        const patches = new Map();
        for (const { locator, patch } of updates) {
            const key = locatorKey(locator);
            patches.set(key, { ...patches.get(key), ...patch });
        }
        const displays = [];
        for (const entry of playlist.entries) {
            if (entry.locator.kind !== 'library' || entry.locator.libraryIdentity?.sha256 !== library.identity.sha256) continue;
            const patch = patches.get(locatorKey(entry.locator));
            if (!patch) continue;
            const display = {};
            for (const key of ['title', 'format', 'metadataState']) if (Object.hasOwn(patch, key)) display[key] = patch[key];
            if (Object.keys(display).length) displays.push({ id: entry.id, patch: display });
        }
        playlist.updateDisplay(displays);
    }
    return {
        resolver, availability, canPlay,
        setRender(callback) { render = callback; },
        libraryChanged(library) {
            if (library !== state.library) return;
            enrichDisplay(library, library.library.rows.filter(row => row.metadataState === 'ready').map(row => ({ locator: row, patch: row })));
            store.save(snapshot(), { knownLibraries: [{ ...library.identity, sourceKind: library.source.kind, label: library.source.label }] }).catch(error => report(error.message));
            updateRebind(); render();
        },
        libraryUpdated(library, change) {
            if (library !== state.library) return;
            // Row snapshots cover membership arrivals already enriched before discovery.
            // Explicit partial patches also cover playback before the row exists.
            const rows = (change.changedRowIds || []).map(id => library.library.rows[id]).filter(row => row?.metadataState === 'ready');
            enrichDisplay(library, [...rows.map(row => ({ locator: row, patch: row })), ...(change.metadataUpdates || [])]);
            updateRebind(); render();
        },
    };
}
