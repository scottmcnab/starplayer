// Observable ordered model. Navigation never wraps; W10 owns repeat and playback policy.
export function createPlaylist() {
    let entries = [];
    let currentId = null;
    const listeners = new Set();
    const notify = () => { for (const listener of listeners) listener(model); };
    const model = {
        restore(snapshot) { entries = snapshot.entries.map(entry => structuredClone(entry)); currentId = snapshot.currentId ?? null; notify(); },
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

export function wirePlaylistList(playlist, { playback, available = () => true, availability = entry => available(entry) ? '' : 'unavailable' } = {}) {
    const list = document.getElementById('playlist-list');
    let draggedId = null;
    const render = () => {
        const focusedItem = document.activeElement?.closest('[data-entry-id]');
        const focusedId = focusedItem?.dataset.entryId;
        const focusedIndex = [...list.children].indexOf(focusedItem);
        const focusedRemove = document.activeElement?.tagName === 'BUTTON';
        const entries = playlist.entries;
        const currentId = playlist.current?.id;
        document.getElementById('playlist-count').textContent = String(entries.length);
        for (const id of ['playlist-previous', 'playlist-next']) document.getElementById(id).disabled = entries.length === 0;
        list.replaceChildren(...entries.map(entry => {
            const item = document.createElement('li');
            item.dataset.entryId = entry.id;
            item.tabIndex = 0;
            item.draggable = true;
            const current = currentId === entry.id;
            item.classList.toggle('current', current);
            if (current) item.setAttribute('aria-current', 'true');
            const failure = playback?.failures.get(entry.id);
            const unavailable = availability(entry);
            const marker = document.createElement('span');
            marker.className = 'playlist-marker';
            marker.textContent = `${current ? '▶ ' : ''}${failure ? '⚠ ' : ''}${unavailable}`;
            const name = document.createElement('strong');
            name.className = 'playlist-name';
            name.textContent = entry.display.name;
            const detail = document.createElement('span');
            detail.className = 'playlist-detail';
            detail.textContent = [entry.display.title, entry.display.format, entry.display.folder || entry.locator.sourcePath].filter(Boolean).join(' · ');
            item.title = [failure, unavailable, detail.textContent].filter(Boolean).join(' · ');
            const remove = document.createElement('button');
            remove.textContent = '×';
            remove.setAttribute('aria-label', `Remove ${entry.display.name}`);
            remove.addEventListener('click', event => { event.stopPropagation(); playlist.remove([entry.id]); });
            item.append(marker, name, detail, remove);
            item.addEventListener('dblclick', event => { if (!event.target.closest('button')) playback?.playEntry(entry); });
            item.addEventListener('keydown', event => {
                if (event.target !== item) return;
                if (event.key === 'Delete') { event.preventDefault(); playlist.remove([entry.id]); }
                else if (event.key === 'Enter') { event.preventDefault(); playback?.playEntry(entry); }
                else if (event.altKey && ['ArrowUp', 'ArrowDown'].includes(event.key)) {
                    event.preventDefault();
                    playlist.move(entry.id, playlist.entries.findIndex(candidate => candidate.id === entry.id) + (event.key === 'ArrowUp' ? -1 : 1));
                }
            });
            item.addEventListener('dragstart', event => { draggedId = entry.id; event.dataTransfer.setData('text/plain', entry.id); event.dataTransfer.effectAllowed = 'move'; });
            item.addEventListener('dragover', event => { if (draggedId) event.preventDefault(); });
            item.addEventListener('drop', event => {
                if (!draggedId) return;
                event.preventDefault();
                playlist.move(draggedId, playlist.entries.findIndex(candidate => candidate.id === entry.id));
                draggedId = null;
            });
            item.addEventListener('dragend', () => { draggedId = null; });
            return item;
        }));
        if (focusedId) {
            const item = [...list.children].find(item => item.dataset.entryId === focusedId) ?? list.children[Math.min(focusedIndex, list.children.length - 1)];
            (focusedRemove ? item?.querySelector('button') : item)?.focus();
            if (!item) document.getElementById('playlist-add').focus();
        }
    };
    document.getElementById('playlist-clear').addEventListener('click', () => playlist.clear());
    document.getElementById('playlist-previous').addEventListener('click', () => playback?.previous());
    document.getElementById('playlist-next').addEventListener('click', () => playback?.next());
    render();
    const unsubscribe = playlist.subscribe(render);
    return { render, dispose: unsubscribe };
}
