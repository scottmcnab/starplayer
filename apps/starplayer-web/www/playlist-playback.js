// Navigation owns no audio state: all loads, including skips, claim controller intent.
export function createPlaylistPlayback({ playlist, controller, resolver, loop = () => false, report = () => {}, changed = () => {}, available = () => true, clock = globalThis }) {
    const failures = new Map();
    let revision = 0;
    let pendingId = null;
    let previousCurrent = playlist.current?.id;
    let orderedEntries = playlist.entries;
    let positions = new Map(orderedEntries.map((entry, index) => [entry.id, index]));
    function adjacent(id, direction) {
        const index = positions.get(id);
        if (index === undefined) return null;
        return orderedEntries[index + direction] ?? (loop() ? orderedEntries[(index + direction + orderedEntries.length) % orderedEntries.length] : null);
    }
    const unsubscribe = playlist.subscribe(() => {
        orderedEntries = playlist.entries;
        positions = new Map(orderedEntries.map((entry, index) => [entry.id, index]));
        if ((previousCurrent && !positions.has(previousCurrent)) || (pendingId && !positions.has(pendingId))) {
            navigation.cancel();
            controller.cancel();
        }
        previousCurrent = playlist.current?.id;
    });
    const navigation = {
        failures,
        get pending() { return pendingId; },
        cancel() { revision += 1; pendingId = null; },
        async playEntry(entry) {
            if (!entry) return;
            const token = ++revision;
            pendingId = null;
            controller.cancel();
            let consecutiveFailures = 0;
            const skipped = new Set();
            while (entry && token === revision) {
                if (!available(entry)) {
                    if (skipped.has(entry.id)) break;
                    skipped.add(entry.id);
                    entry = adjacent(entry.id, 1);
                    continue;
                }
                pendingId = entry.id;
                const result = await controller.play(entry.locator, { origin: 'playlist', entryId: entry.id, label: entry.display.name });
                if (token !== revision || result.status === 'abandoned') return;
                if (result.status === 'played') {
                    pendingId = null;
                    failures.delete(entry.id);
                    playlist.setCurrent(entry.id);
                    const next = adjacent(entry.id, 1);
                    if (next) Promise.resolve(resolver.prefetch(next.locator)).catch(() => {});
                    changed();
                    return;
                }
                failures.set(entry.id, result.error.message || String(result.error));
                report(`Could not play ${entry.display.name}: ${failures.get(entry.id)}`);
                changed();
                consecutiveFailures += 1;
                if (consecutiveFailures === 3) { pendingId = null; report('Playlist stopped after three consecutive failures.'); return; }
                // Let the diagnostic paint before trying the next entry. Cancellation
                // covers this delay as well as resolve/activation.
                await new Promise(resolve => clock.setTimeout(resolve, 200));
                entry = adjacent(entry.id, 1);
            }
            pendingId = null;
        },
        ended(id) {
            if (controller.playlistMode && playlist.current?.id === id) return navigation.playEntry(adjacent(id, 1));
        },
        next() { return navigation.playEntry(playlist.current ? adjacent(playlist.current.id, 1) : playlist.entries[0]); },
        previous() { return navigation.playEntry(playlist.current ? adjacent(playlist.current.id, -1) : playlist.entries[0]); },
        dispose() { navigation.cancel(); unsubscribe(); },
    };
    return navigation;
}
