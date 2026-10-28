import test from 'node:test';
import assert from 'node:assert/strict';
import { createPlaylist, wirePlaylistList } from '../www/playlist.js';
import { createPlaylistPlayback } from '../www/playlist-playback.js';
import { createPlaylistControls } from '../www/playlist-controls.js';
import { openPlaylistStore } from '../www/playlist-store.js';

// A bubbling DOM fixture exercises the real view handlers. Native pointer/Space
// activation and layout are covered separately by the packaged Chromium harness.
class Element {
    constructor(tag, document) {
        this.tagName = tag.toUpperCase(); this.document = document;
        this.children = []; this.parent = null; this.dataset = {}; this.attributes = {};
        this.listeners = new Map(); this.classes = new Set(); this.value = ''; this.accept = '.mod';
        this.checked = false; this.disabled = false; this.clientHeight = 192; this.scrollTop = 0;
        this.classList = { toggle: (name, value) => value ? this.classes.add(name) : this.classes.delete(name), contains: name => this.classes.has(name) };
    }
    set className(value) { this.classes = new Set(value.split(' ')); }
    get className() { return [...this.classes].join(' '); }
    set textContent(value) { this.text = String(value); this.children = []; }
    get textContent() { return (this.text ?? '') + this.children.map(child => child.textContent).join(''); }
    append(...children) { for (const child of children) { child.parent = this; this.children.push(child); } }
    replaceChildren(...children) { for (const child of this.children) child.parent = null; this.children = []; this.append(...children); }
    setAttribute(name, value) { this.attributes[name] = String(value); }
    matches(selector) {
        if (selector === '[data-entry-id]') return Boolean(this.dataset.entryId);
        if (selector.startsWith('.')) return this.classes.has(selector.slice(1));
        if (selector === 'input[type="checkbox"]') return this.tagName === 'INPUT' && this.type === 'checkbox';
        return this.tagName === selector.toUpperCase();
    }
    closest(selector) { return this.matches(selector) ? this : this.parent?.closest(selector) ?? null; }
    contains(element) { return Boolean(element && (element === this || this.children.some(child => child.contains(element)))); }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
    addEventListener(type, listener) { if (!this.listeners.has(type)) this.listeners.set(type, new Set()); this.listeners.get(type).add(listener); }
    removeEventListener(type, listener) { this.listeners.get(type)?.delete(listener); }
    emit(type, options = {}) {
        const event = { target: this, key: '', preventDefault() { this.defaultPrevented = true; }, stopPropagation() { this.stopped = true; }, ...options };
        for (let current = this; current && !event.stopped; current = current.parent) for (const listener of current.listeners.get(type) ?? []) listener(event);
        return event;
    }
    focus() { if (this.document.activeElement === this) return; this.document.activeElement = this; this.emit('focusin'); }
    click(options) { return this.emit('click', options); }
    toggle() { this.checked = !this.checked; this.indeterminate = false; this.click(); this.emit('change'); }
}
const items = names => names.map(name => ({ locator: { kind: 'library', sourcePath: 'disk.zip', memberPath: name }, display: { name, title: `Title ${name}`, format: 'mod', folder: 'Folder' } }));
function fixture(t, { count = 12, playlist = createPlaylist(), playback } = {}) {
    const previous = globalThis.document;
    const nodes = new Map();
    const document = { activeElement: null, createElement: tag => new Element(tag, document), getElementById: id => { if (!nodes.has(id)) nodes.set(id, new Element('div', document)); return nodes.get(id); } };
    globalThis.document = document;
    const element = name => document.getElementById(`playlist-${name}`);
    const panel = element('panel'), viewport = element('viewport'), table = element('table'), list = element('list'), all = element('select-all');
    all.tagName = 'INPUT'; all.type = 'checkbox';
    panel.append(viewport); viewport.append(table); table.append(all, list);
    for (const name of ['remove-selected', 'clear', 'add', 'previous', 'next']) panel.append(element(name));
    const entries = count ? playlist.add(items(Array.from({ length: count }, (_, index) => index === 1 ? 'song0.mod' : `song${index}.mod`))) : playlist.entries;
    const played = [];
    playback ??= { failures: new Map(), playEntry: entry => played.push(entry.id), previous() { played.push('previous'); }, next() { played.push('next'); } };
    const view = wirePlaylistList(playlist, { playback, availability: entry => entry.id === entries[1]?.id ? 'library not connected' : '' });
    const row = index => list.children[index];
    const selected = () => list.children.filter(row => row.attributes['aria-selected'] === 'true').map(row => row.dataset.entryId);
    t.after(() => { view.dispose(); globalThis.document = previous; });
    return { playlist, entries, row, selected, element, view, document, played, playback };
}
const key = (target, key, options) => target.emit('keydown', { key, ...options });

test('heading none/mixed/all covers offscreen unavailable duplicate UUIDs; additions stay unselected', t => {
    const f = fixture(t);
    assert.equal(f.element('remove-selected').disabled, true);
    assert.equal(f.element('select-all').checked, false);
    assert.equal(f.element('select-all').indeterminate, false);
    assert.equal(f.row(0).tagName, 'TR');
    assert.equal(f.row(0).children.length, 5);
    assert.match(f.row(1).title, /library not connected/);
    f.row(1).querySelector('input').toggle();
    assert.deepEqual(f.selected(), [f.entries[1].id]);
    assert.equal(f.element('select-all').indeterminate, true);
    f.element('select-all').toggle();
    assert.deepEqual(f.selected(), f.entries.map(entry => entry.id));
    assert.equal(f.element('select-all').checked, true);
    assert.equal(f.element('select-all').indeterminate, false);
    assert.equal(f.element('select-all').attributes['aria-label'], 'Clear Playlist selection');
    const [added] = f.playlist.add(items(['new.mod']));
    assert.equal(f.selected().includes(added.id), false);
    assert.equal(f.element('select-all').indeterminate, true);
    f.element('select-all').checked = true; f.element('select-all').emit('change');
    f.element('select-all').toggle();
    assert.deepEqual(f.selected(), []);
    f.playlist.clear();
    assert.equal(f.element('select-all').disabled, true);
});

test('row modifiers and keyboard movement use ordered UUID ranges and viewport page size', t => {
    const f = fixture(t);
    f.row(1).click();
    f.row(3).click({ ctrlKey: true });
    assert.deepEqual(f.selected(), [f.entries[1].id, f.entries[3].id]);
    f.row(5).click({ shiftKey: true });
    assert.deepEqual(f.selected(), f.entries.slice(3, 6).map(entry => entry.id));
    key(f.row(5), 'ArrowUp', { shiftKey: true });
    assert.deepEqual(f.selected(), f.entries.slice(3, 5).map(entry => entry.id));
    key(f.row(4), 'Home');
    assert.deepEqual(f.selected(), [f.entries[0].id]);
    key(f.row(0), 'PageDown');
    assert.deepEqual(f.selected(), [f.entries[5].id]);
    assert.equal(f.document.activeElement, f.row(5));
    assert.ok(f.element('viewport').scrollTop > 0);
    key(f.row(5), 'PageUp');
    assert.deepEqual(f.selected(), [f.entries[0].id]);
    key(f.row(0), 'End', { shiftKey: true });
    assert.equal(f.selected().length, 12);
    key(f.row(11), 'Escape');
    assert.deepEqual(f.selected(), []);
    key(f.row(11), 'a', { metaKey: true });
    assert.equal(f.selected().length, 12);
    key(f.row(11), 'Escape');
    f.row(2).click({ metaKey: true }); f.row(2).click({ ctrlKey: true });
    assert.deepEqual(f.selected(), []);
});

test('native checkboxes isolate click/Space/Enter/double-click and retain focus without rebuilding', t => {
    const f = fixture(t);
    let escaped = 0;
    for (const type of ['click', 'keydown', 'keyup', 'dblclick']) f.element('panel').addEventListener(type, () => escaped++);
    f.row(0).click();
    escaped = 0;
    const row = f.row(1), checkbox = row.querySelector('input');
    checkbox.focus();
    assert.deepEqual(f.selected(), [f.entries[0].id], 'focus alone leaves selection alone');
    assert.equal(key(checkbox, ' ').defaultPrevented, undefined, 'Space retains native activation');
    checkbox.emit('keyup', { key: ' ' });
    checkbox.toggle();
    checkbox.emit('dblclick');
    key(checkbox, 'Enter');
    assert.equal(escaped, 0);
    assert.deepEqual(f.selected(), f.entries.slice(0, 2).map(entry => entry.id));
    assert.equal(f.document.activeElement, checkbox);
    assert.equal(f.row(1), row);
    assert.equal(f.played.length, 0);
    f.row(1).emit('dblclick'); key(f.row(0), 'Enter');
    assert.deepEqual(f.played, [f.entries[1].id, f.entries[0].id]);
});

test('UUID selection survives metadata/playback/rebind/reorder; deleted focus and anchor are pruned', t => {
    const f = fixture(t, { count: 5 });
    f.row(1).click(); f.row(1).querySelector('input').focus();
    f.playback.failures.set(f.entries[1].id, 'Complete extraction diagnostic');
    f.playlist.updateDisplay([{ id: f.entries[1].id, patch: { title: 'Actual metadata' } }]);
    f.playlist.setCurrent(f.entries[1].id);
    assert.deepEqual(f.selected(), [f.entries[1].id]);
    assert.match(f.row(1).title, /Complete extraction diagnostic/);
    assert.match(f.row(1).title, /Actual metadata/);
    assert.equal(f.row(1).attributes['aria-current'], 'true');
    assert.equal(f.document.activeElement, f.row(1).querySelector('input'));
    f.playlist.restore({ entries: f.playlist.entries, currentId: f.entries[1].id });
    assert.deepEqual(f.selected(), [f.entries[1].id]);
    key(f.row(1), 'ArrowDown', { altKey: true });
    assert.equal(f.row(2).dataset.entryId, f.entries[1].id);
    assert.deepEqual(f.selected(), [f.entries[1].id]);
    f.row(2).focus();
    f.playlist.remove([f.entries[1].id]);
    assert.equal(f.document.activeElement, f.row(2));
    assert.deepEqual(f.selected(), []);
    f.row(3).click({ shiftKey: true });
    assert.deepEqual(f.selected(), [f.entries[4].id], 'missing range anchor cannot retain deleted UUID');
    f.playlist.clear();
    assert.equal(f.document.activeElement, f.element('add'));
    const [added] = f.playlist.add(items(['new.mod']));
    f.row(0).click({ shiftKey: true });
    assert.deepEqual(f.selected(), [added.id]);
});

test('bulk toolbar/Delete publish once, empty Delete is no-op, and focus follows a neighbour', t => {
    const f = fixture(t, { count: 5 });
    let notifications = 0, removes = 0;
    f.playlist.subscribe(() => notifications++);
    const remove = f.playlist.remove;
    f.playlist.remove = ids => { removes++; remove(ids); };
    key(f.row(0), 'Delete');
    assert.equal(notifications, 0);
    f.row(1).click(); f.row(3).click({ ctrlKey: true });
    assert.equal(notifications, 0, 'selection never writes the model or persistence subscribers');
    f.element('remove-selected').focus(); f.element('remove-selected').click();
    assert.equal(notifications, 1); assert.equal(removes, 1);
    assert.deepEqual(f.playlist.entries.map(entry => entry.id), [f.entries[0].id, f.entries[2].id, f.entries[4].id]);
    assert.equal(f.document.activeElement, f.row(2));
    key(f.row(2), 'a', { ctrlKey: true }); key(f.row(2), 'Delete');
    assert.equal(notifications, 2); assert.equal(removes, 2);
    assert.equal(f.document.activeElement, f.element('add'));
    assert.equal(f.element('remove-selected').disabled, true);
});

test('single-item drag moves only the dragged UUID and dispose removes all listeners', t => {
    const f = fixture(t, { count: 5 });
    f.element('select-all').toggle();
    const data = [], dataTransfer = { setData: (...args) => data.push(args) };
    f.row(1).emit('dragstart', { dataTransfer });
    assert.equal(dataTransfer.effectAllowed, 'move');
    assert.deepEqual(data, [['text/plain', f.entries[1].id]]);
    f.row(4).emit('drop');
    assert.deepEqual(f.playlist.entries.map(entry => entry.id), [f.entries[0].id, f.entries[2].id, f.entries[3].id, f.entries[4].id, f.entries[1].id]);
    assert.equal(f.selected().length, 5);
    f.row(0).emit('dragstart', { dataTransfer }); f.row(0).emit('dragend');
    assert.equal(f.row(4).emit('drop').defaultPrevented, undefined);
    assert.equal(f.row(0).querySelector('input').emit('dragstart', { dataTransfer }).defaultPrevented, true);
    f.view.dispose();
    const before = f.row(0);
    f.element('remove-selected').click(); f.element('clear').click(); f.element('previous').click(); f.element('next').click();
    key(f.row(0), 'Enter'); f.row(0).emit('dblclick'); f.row(0).click();
    assert.equal(f.playlist.entries.length, 5); assert.equal(f.played.length, 0);
    f.playlist.clear(); assert.equal(f.row(0), before, 'disposed model subscription no longer renders');
    assert.equal([...f.element('table').listeners.values()].reduce((count, listeners) => count + listeners.size, 0), 0);
});

test('bulk removal cancels current AND pending navigation while preserving sounding audio', async t => {
    const playlist = createPlaylist();
    const entries = playlist.add(items(['current.mod', 'pending.mod', 'survivor.mod']));
    playlist.setCurrent(entries[0].id);
    let finish, cancellations = 0, stops = 0;
    const controller = { playlistMode: true, cancel() { cancellations++; }, stop() { stops++; }, play: () => new Promise(resolve => { finish = resolve; }) };
    const playback = createPlaylistPlayback({ playlist, controller, resolver: { prefetch() {} } });
    t.after(() => playback.dispose());
    const f = fixture(t, { count: 0, playlist, playback });
    const pending = playback.playEntry(entries[1]);
    f.row(0).click(); f.row(1).click({ ctrlKey: true });
    const before = cancellations;
    key(f.row(1), 'Delete');
    assert.equal(cancellations, before + 1);
    assert.equal(playback.pending, null); assert.equal(playlist.current, null); assert.equal(stops, 0);
    finish({ status: 'played' }); await pending;
    assert.equal(playlist.current, null, 'late completion cannot restore a removed entry');
    assert.equal(f.playlist.entries[0].id, entries[2].id);
});

test('selection never persists; bulk removal retains duplicate upload references until the last one', async t => {
    const playlist = createPlaylist();
    const store = await openPlaylistStore({ indexedDB: null });
    t.after(() => store.close());
    const upload = { id: 'first', locator: { kind: 'upload', blobId: 'shared' }, display: { name: 'upload.mod' } };
    const second = { ...upload, id: 'second' }, other = { ...upload, id: 'other', locator: { kind: 'upload', blobId: 'other' } };
    await store.save({ entries: [upload, second, other], currentId: null, loop: false }, { uploads: [{ blobId: 'shared', size: 4 }, { blobId: 'other', size: 4 }] });
    const f = fixture(t, { count: 0, playlist });
    let writes = 0;
    const save = store.save;
    store.save = (...args) => { writes++; return save(...args); };
    await createPlaylistControls({ playlist, state: {}, loader: {}, openStore: async () => store });
    f.row(0).click();
    assert.equal(writes, 0);
    assert.equal(store.state.entries.some(entry => 'selected' in entry), false);
    key(f.row(0), 'Delete'); await store.settled();
    assert.equal(writes, 1); assert.equal(store.hasBlob('shared'), true);
    f.element('select-all').toggle();
    f.element('remove-selected').click(); await store.settled();
    assert.equal(writes, 2); assert.equal(store.hasBlob('shared'), false); assert.equal(store.hasBlob('other'), false);
    playlist.restore({ entries: [upload, second], currentId: null });
    assert.deepEqual(f.selected(), [], 'reloaded UUIDs cannot resurrect deleted selection');
    f.row(0).click(); f.view.dispose();
    const reloaded = wirePlaylistList(playlist);
    assert.deepEqual(f.selected(), [], 'new page view starts with no selection');
    reloaded.dispose();
});


test('availability uses a compact accessible warning before filename with separate playback markers', t => {
    const f = fixture(t, { count: 3 });
    f.playback.failures.set(f.entries[1].id, 'Full extraction diagnostic');
    f.playlist.setCurrent(f.entries[1].id);
    const row = f.row(1), warning = row.querySelector('.playlist-availability-warning');
    const name = row.querySelector('.playlist-name'), marker = row.querySelector('.playlist-marker');
    assert.equal(warning.textContent, '!');
    assert.equal(warning.title, 'library not connected');
    assert.equal(warning.attributes['role'], 'img');
    assert.equal(warning.attributes['aria-label'], warning.title);
    assert.deepEqual(row.querySelector('.playlist-file').children, [warning, name, marker]);
    assert.equal(name.title, 'song0.mod');
    assert.equal(marker.textContent, '▶ ⚠');
    assert.match(marker.title, /Currently playing.*Full extraction diagnostic/);
    assert.doesNotMatch(row.querySelector('.playlist-file').textContent, /library not connected/);
    assert.match(row.children[1].title, /library not connected.*song0.mod.*disk.zip/);
    assert.equal(f.row(0).querySelector('.playlist-availability-warning'), null);
});
