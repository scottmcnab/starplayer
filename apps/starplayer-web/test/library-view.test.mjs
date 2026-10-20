import test from 'node:test';
import assert from 'node:assert/strict';
import { createSelection, moveFocus, selectAll, selectRow, virtualWindow } from '../www/library-selection.js';
import { createPlaylist, libraryLocator } from '../www/playlist.js';

test('click, toggle, range and filtered select-all preserve stable ids', () => {
    const order = Int32Array.of(3, 1, 4, 0, 2);
    let state = selectRow(createSelection(), order, 1);
    assert.deepEqual([...state.selected], [1]);
    state = selectRow(state, order, 0, { toggle: true });
    assert.deepEqual([...state.selected], [1, 0]);
    state = selectRow(state, order, 1, { toggle: true });
    assert.deepEqual([...state.selected], [0]);
    state = selectRow(state, order, 2, { extend: true });
    assert.deepEqual([...state.selected], [1, 4, 0, 2]);
    state = selectRow(state, Int32Array.of(4, 1), 4, { extend: true });
    assert.deepEqual([...state.selected], [0, 2, 4, 1]);
    state = selectAll(state, Int32Array.of(3));
    assert.deepEqual([...state.selected], [0, 2, 4, 1, 3]);
    assert.deepEqual([...state.selected].filter(id => [3].includes(id)), [3]);
    const missingAnchor = selectRow(state, Int32Array.of(0, 2), 2, { extend: true });
    assert.deepEqual([...missingAnchor.selected], [4, 1, 3, 2]);
    assert.equal(missingAnchor.anchor, 2);
    assert.deepEqual([...createSelection().selected], []);
});

test('keyboard movement extends and contracts ranges in current sorted order', () => {
    const order = Int32Array.of(7, 3, 9, 1, 5);
    let state = moveFocus(createSelection(), order, 'ArrowDown');
    assert.equal(state.focus, 7);
    state = moveFocus(state, order, 'ArrowDown', { extend: true });
    state = moveFocus(state, order, 'ArrowDown', { extend: true });
    assert.deepEqual([...state.selected], [7, 3, 9]);
    state = moveFocus(state, order, 'ArrowUp', { extend: true });
    assert.deepEqual([...state.selected], [7, 3]);
    state = moveFocus(state, order, 'End', { extend: true });
    assert.deepEqual([...state.selected], [...order]);
    state = moveFocus(state, order, 'PageUp', { pageSize: 2 });
    assert.equal(state.focus, 9);
    state = moveFocus(state, order, 'Home');
    assert.equal(state.focus, 7);
    state = moveFocus(state, order, 'PageDown', { pageSize: 99 });
    assert.equal(state.focus, 5);
    assert.equal(moveFocus(state, [], 'End'), state);
});

test('virtual windows clamp at both ends and never exceed 200 rows', () => {
    assert.deepEqual(virtualWindow(0, 320, 0), { first: 0, last: 0 });
    assert.deepEqual(virtualWindow(0, 320, 37674), { first: 0, last: 15 });
    assert.deepEqual(virtualWindow(3200, 320, 37674), { first: 95, last: 115 });
    assert.deepEqual(virtualWindow(37674 * 32, 320, 37674), { first: 37659, last: 37674 });
    assert.deepEqual(virtualWindow(5000, 320, 3), { first: 0, last: 3 });
    const huge = virtualWindow(32000, 100000, 37674, 32, 1000);
    assert.ok(huge.last - huge.first <= 200);
});

test('playlist preserves duplicate locators, unique ids, order, current and subscriptions', () => {
    const playlist = createPlaylist();
    const locator = libraryLocator({ sourcePath: 'disk.zip', containerMember: '', memberPath: 'REFLEX.S3M', rawHex: '' }, { name: 'index.csv', sha256: 'identity' });
    let notifications = 0;
    const unsubscribe = playlist.subscribe(() => notifications += 1);
    const [first, second] = playlist.add([{ locator, display: { name: 'REFLEX.S3M' } }, { locator, display: { name: 'REFLEX.S3M' } }]);
    assert.notEqual(first.id, second.id);
    assert.equal(first.locator.libraryIdentity.sha256, 'identity');
    assert.equal(playlist.next(), first);
    assert.equal(playlist.next(), second);
    assert.equal(playlist.next(), null);
    assert.equal(playlist.previous(), first);
    playlist.move(second.id, 0);
    assert.deepEqual(playlist.entries.map(entry => entry.id), [second.id, first.id]);
    playlist.remove([first.id]);
    assert.equal(playlist.current, null);
    playlist.setCurrent('unknown');
    assert.equal(playlist.current, null);
    const [inserted] = playlist.add([{ locator, display: { name: 'PETRI.S3M' } }], { at: 0 });
    assert.equal(playlist.entries[0], inserted);
    playlist.clear();
    assert.deepEqual(playlist.entries, []);
    const before = notifications;
    unsubscribe();
    playlist.clear();
    assert.equal(notifications, before);
});
