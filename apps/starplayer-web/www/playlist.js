// In-memory W7 model. Navigation never wraps; W10 owns repeat and playback policy.
export function createPlaylist() {
    let entries = [];
    let currentId = null;
    const listeners = new Set();
    const notify = () => { for (const listener of listeners) listener(model); };
    const model = {
        get entries() { return entries.slice(); },
        get current() { return entries.find(entry => entry.id === currentId) ?? null; },
        add(items, { at = entries.length } = {}) {
            const added = items.map(item => ({ id: crypto.randomUUID(), locator: { ...item.locator }, display: { ...item.display } }));
            entries.splice(Math.max(0, Math.min(entries.length, at)), 0, ...added);
            notify();
            return added;
        },
        remove(ids) {
            const removed = new Set(ids);
            entries = entries.filter(entry => !removed.has(entry.id));
            if (removed.has(currentId)) currentId = null;
            notify();
        },
        move(id, toIndex) {
            const from = entries.findIndex(entry => entry.id === id);
            if (from < 0) return;
            const [entry] = entries.splice(from, 1);
            entries.splice(Math.max(0, Math.min(entries.length, toIndex)), 0, entry);
            notify();
        },
        clear() { entries = []; currentId = null; notify(); },
        setCurrent(id) {
            if (id !== null && !entries.some(entry => entry.id === id)) return;
            currentId = id;
            notify();
        },
        next() {
            const index = entries.findIndex(entry => entry.id === currentId);
            const entry = entries[index + 1] ?? null;
            if (entry) model.setCurrent(entry.id);
            return entry;
        },
        previous() {
            const index = entries.findIndex(entry => entry.id === currentId);
            const entry = entries[index - 1] ?? null;
            if (entry) model.setCurrent(entry.id);
            return entry;
        },
        subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    };
    return model;
}

export function libraryLocator(row, libraryIdentity) {
    return { kind: 'library', libraryIdentity: { ...libraryIdentity }, sourcePath: row.sourcePath, containerMember: row.containerMember, memberPath: row.memberPath, rawHex: row.rawHex };
}

export function wirePlaylistList(playlist) {
    const list = document.getElementById('playlist-list');
    const render = () => {
        document.getElementById('playlist-count').textContent = String(playlist.entries.length);
        list.replaceChildren(...playlist.entries.map(entry => {
            const item = document.createElement('li');
            item.textContent = entry.display.name;
            item.title = `${entry.display.title} (${entry.display.folder})`;
            return item;
        }));
    };
    render();
    return playlist.subscribe(render);
}
