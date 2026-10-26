// Pure calculations shared by the progressive view and its Node tests.
import { search } from './library-index.js';
import { libraryLocator } from './playlist.js';

export function captureAnchor(order, scrollTop, rowHeight = 32) {
    const index = Math.max(0, Math.min(order.length - 1, Math.floor(scrollTop / rowHeight)));
    return { id: order[index] ?? null, index, offset: Math.max(0, scrollTop) - index * rowHeight };
}

export function restoreAnchor(anchor, previousOrder, nextOrder, viewportHeight, rowHeight = 32) {
    const maximum = Math.max(0, nextOrder.length * rowHeight - viewportHeight);
    if (!nextOrder.length) return 0;
    const positions = new Map(Array.from(nextOrder, (id, index) => [id, index]));
    let index = positions.get(anchor.id);
    // Prefer the next row when equally near, then the previous row.
    for (let distance = 1; index === undefined && distance < previousOrder.length; distance += 1) {
        index = positions.get(previousOrder[anchor.index + distance]) ?? positions.get(previousOrder[anchor.index - distance]);
    }
    return Math.max(0, Math.min(maximum, (index ?? Math.min(anchor.index, nextOrder.length - 1)) * rowHeight + anchor.offset));
}

export function formatOptions(library, selected = '') {
    const formats = new Set(library.formats ?? library.rows.map(row => row.format));
    if (selected) formats.add(selected); // A selected format with zero results stays selected.
    return [...formats].filter(Boolean).sort().map(value => ({ value, label: value.toUpperCase() }));
}

export function unplayableCount(library) {
    return library.rows.length - (library.folderCounts.get('') ?? 0);
}

export function folderAddLabel(path, status) {
    return `${status && status.phase !== 'complete' ? 'Add discovered tracks in' : 'Add folder'} ${path || 'All folders'}`;
}

export function playlistSnapshot(library, ids, identity) {
    return Array.from(ids, id => library.rows[id]).filter(row => row?.playable).map(row => ({
        locator: libraryLocator(row, identity),
        display: { name: row.name, title: row.title, format: row.format, folder: row.folder,
            ...(row.metadataState ? { metadataState: row.metadataState } : {}) },
    }));
}

export function selectedSnapshot(library, order, selected, identity) {
    return playlistSnapshot(library, order.filter(id => selected.has(id)), identity);
}

export function folderSnapshot(library, path, identity) {
    return playlistSnapshot(library, search(library, '', { scopeFolder: path, sort: { column: 'folder', descending: false } }), identity);
}

export function progressText(status) {
    const count = value => (value ?? 0).toLocaleString('en-US');
    const state = { running: 'Discovering tracks', paused: 'Discovery paused — partial results', failed: 'Discovery failed — partial results', complete: status.tracksFound ? 'Discovery complete' : 'Discovery complete — no supported tracks found' }[status.phase];
    const metadata = { idle: 'Metadata not scanned', running: 'Scanning metadata', waiting: 'Metadata scan waiting for discovery', suspended: 'Metadata scan suspended', complete: 'Metadata scan complete', cancelled: 'Metadata scan cancelled' }[status.metadataPhase];
    return `${state}. ${count(status.tracksFound)} tracks found; ${count(status.filesExamined)} files examined; ${count(status.archivesPending)} archives pending; ${count(status.issueCount)} issues. ${metadata}.${status.runtimeError ? ` Background worker error: ${status.runtimeError} Use Retry discovery to resume background work.` : ''}`;
}

export function metadataHint(status) {
    const counts = status.metadataCounts ?? {};
    const sidecars = status.sidecarCounts ?? {};
    const parts = [];
    if (status.metadataIncomplete) {
        const reason = counts.unknown || counts.error
            ? `${counts.unknown ?? 0} tracks await metadata, ${counts.error ?? 0} have metadata errors.`
            : 'Some tracks still need checksums for comments matching.';
        parts.push(`Metadata searches are incomplete: ${reason} ${counts.hashesPending ?? 0} checksum jobs pending. Play a track or use Scan metadata; failed tracks have Retry metadata in their details.`);
    }
    if (sidecars.awaitingHashes) parts.push(`Comments: ${sidecars.joined ?? 0} joined, ${sidecars.unmatched ?? 0} currently unmatched, ${sidecars.awaitingHashes} awaiting metadata/hashes.`);
    return parts.join(' ');
}
