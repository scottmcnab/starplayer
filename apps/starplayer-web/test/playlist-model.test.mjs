import test from 'node:test';
import assert from 'node:assert/strict';
import { createPlaylist } from '../www/playlist.js';

test('updateDisplay merges a batch once, preserves identities/order/current and skips no-ops', () => {
    const playlist = createPlaylist();
    const entries = playlist.add(['one', 'two', 'one'].map(name => ({ locator: { kind: 'library', sourcePath: name }, display: { name, title: '', format: 'mod', metadataState: 'unknown' } })));
    playlist.setCurrent(entries[1].id);
    const locators = entries.map(entry => structuredClone(entry.locator));
    let notifications = 0;
    playlist.subscribe(() => notifications++);
    assert.deepEqual(playlist.updateDisplay([{ id: entries[0].id, patch: { title: 'Actual' } }, { id: entries[0].id, patch: { format: 's3m', metadataState: 'ready' } }, { id: entries[2].id, patch: { title: 'Actual' } }, { id: 'removed', patch: { title: 'ignored' } }]), [entries[0].id, entries[2].id]);
    assert.equal(notifications, 1);
    assert.deepEqual(playlist.entries.map(entry => entry.id), entries.map(entry => entry.id));
    assert.deepEqual(playlist.entries.map(entry => entry.locator), locators);
    assert.equal(playlist.current.id, entries[1].id);
    assert.equal(playlist.entries[0].display.name, 'one');
    assert.equal(playlist.entries[0].display.title, 'Actual');
    assert.equal(playlist.entries[0].display.metadataState, 'ready');
    assert.deepEqual(playlist.updateDisplay([{ id: entries[0].id, patch: { title: 'Actual', metadataState: 'ready' } }]), []);
    playlist.updateDisplay([]);
    assert.equal(notifications, 1);
});
