import { createSelection, deselectAll, moveFocus, selectAll, selectRow } from './library-selection.js';

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
        // One notification (and normal subscriber save) for a batch; unchanged fields
        // do not turn hash/status-only publications into persistence writes.
        updateDisplay(updates) {
            const patches = new Map();
            for (const { id, patch } of updates) patches.set(id, { ...patches.get(id), ...patch });
            const changed = [];
            for (const entry of entries) {
                const patch = patches.get(entry.id);
                if (!patch || !Object.keys(patch).some(key => !Object.is(entry.display[key], patch[key]))) continue;
                entry.display = { ...entry.display, ...patch };
                changed.push(entry.id);
            }
            if (changed.length) notify();
            return changed;
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

// Playlist selection uses entry UUIDs, including duplicate or unavailable locators.
// Unlike the Library's filtered result, its order is complete: deleted IDs are pruned.
export function wirePlaylistList(playlist, { playback, available = () => true, availability = entry => available(entry) ? '' : 'unavailable' } = {}) {
    const element = name => document.getElementById(`playlist-${name}`);
    const list = element('list');
    const table = element('table');
    const viewport = element('viewport');
    const heading = element('select-all');
    const remove = element('remove-selected');
    let selection = createSelection();
    let order = [];
    let draggedId = null;
    let removalFocus = false;
    const listeners = [];
    function listen(target, type, listener) {
        target.addEventListener(type, listener);
        listeners.push(() => target.removeEventListener(type, listener));
    }
    const rowFor = id => [...list.children].find(row => row.dataset.entryId === id);
    const entryFor = id => playlist.entries.find(entry => entry.id === id);
    function prune(nextOrder) {
        const present = new Set(nextOrder);
        selection = {
            selected: new Set([...selection.selected].filter(id => present.has(id))),
            anchor: present.has(selection.anchor) ? selection.anchor : null,
            focus: present.has(selection.focus) ? selection.focus : null,
        };
    }
    function updateSelection() {
        for (const row of list.children) {
            const id = row.dataset.entryId;
            const selected = selection.selected.has(id);
            row.setAttribute('aria-selected', String(selected));
            row.classList.toggle('focused', id === selection.focus);
            row.tabIndex = id === (selection.focus ?? order[0]) ? 0 : -1;
            row.querySelector('input').checked = selected;
        }
        heading.disabled = order.length === 0;
        heading.checked = order.length > 0 && selection.selected.size === order.length;
        heading.indeterminate = selection.selected.size > 0 && selection.selected.size < order.length;
        heading.setAttribute('aria-label', heading.checked ? 'Clear Playlist selection' : 'Select all Playlist entries');
        remove.disabled = selection.selected.size === 0;
        element('selection-count').textContent = `${selection.selected.size} selected`;
    }
    function focusRow(id) {
        rowFor(id)?.focus({ preventScroll: true });
        const top = order.indexOf(id) * 32;
        const height = Math.max(32, viewport.clientHeight - 32);
        if (top < viewport.scrollTop) viewport.scrollTop = top;
        else if (top + 32 > viewport.scrollTop + height) viewport.scrollTop = top + 32 - height;
    }
    function removeSelected() {
        // A single model publication preserves playback cancellation and blob cleanup.
        const ids = order.filter(id => selection.selected.has(id));
        if (!ids.length) return;
        removalFocus = true;
        playlist.remove(ids);
    }
    function cell(text, className) {
        const result = document.createElement('td');
        result.setAttribute('role', 'gridcell');
        result.textContent = text ?? '';
        result.title = result.textContent;
        if (className) result.className = className;
        return result;
    }
    const render = () => {
        const focused = document.activeElement;
        const focusedItem = list.contains(focused) ? focused.closest('[data-entry-id]') : null;
        const focusedId = focusedItem?.dataset.entryId ?? (removalFocus ? selection.focus ?? [...selection.selected][0] : null);
        const focusedIndex = Math.max(0, order.indexOf(focusedId ?? selection.focus));
        const focusedCheckbox = focused?.matches('input[type="checkbox"]') && Boolean(focusedItem);
        removalFocus = false;
        const entries = playlist.entries;
        order = entries.map(entry => entry.id);
        const oldFocus = selection.focus;
        prune(order);
        if (oldFocus && !selection.focus) selection.focus = order[Math.min(focusedIndex, order.length - 1)] ?? null;
        const currentId = playlist.current?.id;
        element('count').textContent = String(entries.length);
        for (const id of ['previous', 'next']) element(id).disabled = entries.length === 0;
        table.setAttribute('aria-rowcount', String(entries.length + 1));
        list.replaceChildren(...entries.map(entry => {
            const item = document.createElement('tr');
            item.dataset.entryId = entry.id;
            item.draggable = true;
            const current = currentId === entry.id;
            item.classList.toggle('current', current);
            if (current) item.setAttribute('aria-current', 'true');
            const failure = playback?.failures.get(entry.id);
            const unavailable = availability(entry);
            const source = [entry.locator.sourcePath, entry.locator.containerMember, entry.locator.memberPath].filter(Boolean).join(' / ');
            const diagnostics = [failure, unavailable, entry.display.name, entry.display.title, entry.display.format, entry.display.folder, source].filter(Boolean).join(' · ');
            item.title = diagnostics;
            item.setAttribute('aria-label', diagnostics);
            const selectionCell = cell('', 'playlist-selection-cell');
            const label = document.createElement('label');
            const checkbox = document.createElement('input');
            checkbox.type = 'checkbox';
            checkbox.setAttribute('aria-label', `Select ${entry.display.name}`);
            label.append(checkbox);
            selectionCell.append(label);
            const filename = cell('', 'playlist-filename');
            const file = document.createElement('span');
            file.className = 'playlist-file';
            const name = document.createElement('span');
            name.className = 'playlist-name';
            name.textContent = entry.display.name;
            name.title = entry.display.name;
            const marker = document.createElement('span');
            marker.className = 'playlist-marker';
            marker.textContent = [current ? '▶' : '', failure ? '⚠' : ''].filter(Boolean).join(' ');
            marker.title = [current ? 'Currently playing' : '', failure].filter(Boolean).join(' · ');
            if (unavailable) {
                const warning = document.createElement('span');
                warning.className = 'playlist-availability-warning';
                warning.textContent = '!';
                warning.title = unavailable;
                warning.setAttribute('role', 'img');
                warning.setAttribute('aria-label', unavailable);
                file.append(warning);
            }
            file.append(name, marker);
            filename.append(file);
            filename.title = diagnostics;
            item.append(selectionCell, filename, cell(entry.display.title), cell(entry.display.format), cell(entry.display.folder || source));
            return item;
        }));
        updateSelection();
        if (focusedId) {
            const item = rowFor(focusedId) ?? list.children[Math.min(focusedIndex, list.children.length - 1)];
            if (item) (focusedCheckbox ? item.querySelector('input') : item).focus({ preventScroll: true });
            else element('add').focus({ preventScroll: true });
        }
    };
    listen(table, 'mousedown', event => {
        if (!event.target.closest('.playlist-selection-cell') && (event.shiftKey || event.ctrlKey || event.metaKey || event.detail > 1)) event.preventDefault();
    });
    listen(table, 'click', event => {
        // Keep native label/checkbox activation; selection is applied once by change.
        if (event.target.closest('.playlist-selection-cell') || event.target === heading) { event.stopPropagation(); return; }
        const row = event.target.closest('[data-entry-id]');
        if (!row) return;
        selection = selectRow(selection, order, row.dataset.entryId, { toggle: event.ctrlKey || event.metaKey, extend: event.shiftKey });
        updateSelection();
        row.focus({ preventScroll: true });
    });
    listen(table, 'change', event => {
        if (event.target === heading) selection = heading.checked ? selectAll(selection, order) : deselectAll(selection, order);
        else {
            const row = event.target.closest('[data-entry-id]');
            if (!row || !event.target.matches('input[type="checkbox"]')) return;
            selection = selectRow(selection, order, row.dataset.entryId, { toggle: true });
        }
        event.stopPropagation();
        updateSelection();
    });
    listen(table, 'dblclick', event => {
        if (event.target.closest('.playlist-selection-cell') || event.target === heading) { event.stopPropagation(); return; }
        const row = event.target.closest('[data-entry-id]');
        if (row) playback?.playEntry(entryFor(row.dataset.entryId));
    });
    listen(table, 'focusin', event => {
        const row = event.target.closest('[data-entry-id]');
        if (!row) return;
        selection = { ...selection, focus: row.dataset.entryId };
        updateSelection();
    });
    listen(table, 'keydown', event => {
        if (event.target.matches('input[type="checkbox"]') && [' ', 'Spacebar', 'Enter'].includes(event.key)) { event.stopPropagation(); return; }
        const row = event.target.closest('[data-entry-id]');
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') selection = selectAll(selection, order);
        else if (event.key === 'Escape') selection = { ...createSelection(), focus: selection.focus };
        else if (event.key === 'Delete') { event.preventDefault(); event.stopPropagation(); removeSelected(); return; }
        else if (event.key === 'Enter' && row) playback?.playEntry(entryFor(row.dataset.entryId));
        else if (event.altKey && ['ArrowUp', 'ArrowDown'].includes(event.key) && row) {
            event.preventDefault(); event.stopPropagation();
            playlist.move(row.dataset.entryId, order.indexOf(row.dataset.entryId) + (event.key === 'ArrowUp' ? -1 : 1));
            return;
        } else if (['ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown'].includes(event.key)) {
            selection = moveFocus(selection, order, event.key, { extend: event.shiftKey, pageSize: Math.max(1, Math.floor((viewport.clientHeight - 32) / 32)) });
            updateSelection();
            focusRow(selection.focus);
        } else return;
        event.preventDefault();
        event.stopPropagation();
        updateSelection();
    });
    listen(table, 'keyup', event => {
        if (event.target.matches('input[type="checkbox"]') && [' ', 'Spacebar', 'Enter'].includes(event.key)) event.stopPropagation();
    });
    listen(list, 'dragstart', event => {
        const row = event.target.closest('[data-entry-id]');
        if (!row || event.target.closest('.playlist-selection-cell')) { event.preventDefault(); return; }
        draggedId = row.dataset.entryId;
        event.dataTransfer.setData('text/plain', draggedId);
        event.dataTransfer.effectAllowed = 'move';
    });
    listen(list, 'dragover', event => { if (draggedId) event.preventDefault(); });
    listen(list, 'drop', event => {
        const row = event.target.closest('[data-entry-id]');
        if (!draggedId || !row) return;
        event.preventDefault();
        playlist.move(draggedId, order.indexOf(row.dataset.entryId));
        draggedId = null;
    });
    listen(list, 'dragend', () => { draggedId = null; });
    listen(remove, 'click', removeSelected);
    listen(element('clear'), 'click', () => playlist.clear());
    listen(element('previous'), 'click', () => playback?.previous());
    listen(element('next'), 'click', () => playback?.next());
    render();
    const unsubscribe = playlist.subscribe(render);
    return { render, dispose() { unsubscribe(); for (const removeListener of listeners) removeListener(); } };
}
