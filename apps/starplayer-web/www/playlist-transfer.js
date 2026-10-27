import { parsePlaylistCsv, toPlaylistCsv, locatorKey } from './library-index.js';
import { libraryLocator } from './playlist.js';

export { locatorKey };
const libraryKeys = new WeakMap();
// Rows and locators are append-only; metadata/sidecar revisions do not change membership.
// Refresh the same Set on access using appended rows; reset for a replaced or shortened array.
export function keysForLibrary(library) {
    const rows = library.rows;
    let cached = libraryKeys.get(library);
    if (!cached || cached.rows !== rows || cached.length > rows.length) {
        cached = { rows, length: 0, keys: new Set() };
        libraryKeys.set(library, cached);
    }
    for (let index = cached.length; index < rows.length; index++) cached.keys.add(locatorKey(rows[index]));
    cached.length = rows.length;
    return cached.keys;
}
export function rebindCandidates(entries, library) {
    if (!library || (library.mode === 'discovered' && library.discoveryStatus?.phase !== 'complete')) return [];
    const keys = keysForLibrary(library.library);
    return entries.filter(entry => entry.locator.kind === 'library' && entry.locator.libraryIdentity?.sha256 !== library.identity.sha256 && keys.has(locatorKey(entry.locator))).map(entry => entry.id);
}
export function rebindEntries(entries, ids, identity) {
    const selected = new Set(ids);
    return entries.map(entry => selected.has(entry.id) ? { ...entry, locator: { ...entry.locator, libraryIdentity: { ...identity } } } : entry);
}
export function exportPlaylist(entries) {
    const rows = entries.filter(entry => entry.locator.kind === 'library');
    return { csv: toPlaylistCsv(rows.map(entry => ({ ...entry.locator, title: entry.display.title, format: entry.display.metadataState === undefined || entry.display.metadataState === 'ready' ? entry.display.format : '', library: entry.locator.libraryIdentity.name }))), skipped: entries.length - rows.length };
}
export function importPlaylist(text, library) {
    if (!library) throw new Error('Open a library before importing a playlist.');
    const parsed = parsePlaylistCsv(text);
    const keys = keysForLibrary(library.library);
    const discovered = library.mode === 'discovered';
    const missing = parsed.entries.filter(row => !keys.has(locatorKey(row))).length;
    const pending = discovered && library.discoveryStatus?.phase !== 'complete' ? missing : 0;
    const rows = new Map(library.library.rows.map(row => [locatorKey(row), row]));
    return {
        items: parsed.entries.map(row => {
            const verified = discovered && rows.get(locatorKey(row))?.metadataState === 'ready' ? rows.get(locatorKey(row)) : null;
            return { locator: libraryLocator(row, library.identity), display: {
                name: (row.memberPath || row.sourcePath).split(/[\\/]/).at(-1), title: verified ? verified.title : row.title,
                format: verified ? verified.format : row.format, folder: row.sourcePath,
                ...(discovered ? { metadataState: verified ? 'ready' : 'unknown' } : {}),
            } };
        }),
        unavailable: missing - pending,
        pending,
        issues: parsed.issues,
    };
}
