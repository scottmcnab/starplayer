import { parsePlaylistCsv, toPlaylistCsv } from './library-index.js';
import { libraryLocator } from './playlist.js';

export const locatorKey = row => JSON.stringify([row.sourcePath || '', row.containerMember || '', row.memberPath || '']);
export function rebindCandidates(entries, library) {
    if (!library) return [];
    const keys = new Set(library.library.rows.map(locatorKey));
    return entries.filter(entry => entry.locator.kind === 'library' && entry.locator.libraryIdentity?.sha256 !== library.identity.sha256 && keys.has(locatorKey(entry.locator))).map(entry => entry.id);
}
export function rebindEntries(entries, ids, identity) {
    const selected = new Set(ids);
    return entries.map(entry => selected.has(entry.id) ? { ...entry, locator: { ...entry.locator, libraryIdentity: { ...identity } } } : entry);
}
export function exportPlaylist(entries) {
    const rows = entries.filter(entry => entry.locator.kind === 'library');
    return { csv: toPlaylistCsv(rows.map(entry => ({ ...entry.locator, title: entry.display.title, format: entry.display.format, library: entry.locator.libraryIdentity.name }))), skipped: entries.length - rows.length };
}
export function importPlaylist(text, library) {
    if (!library) throw new Error('Open a library before importing a playlist.');
    const parsed = parsePlaylistCsv(text);
    const keys = new Set(library.library.rows.map(locatorKey));
    return {
        items: parsed.entries.map(row => ({ locator: libraryLocator(row, library.identity), display: { name: (row.memberPath || row.sourcePath).split(/[\\/]/).at(-1), title: row.title, format: row.format, folder: row.sourcePath } })),
        unavailable: parsed.entries.filter(row => !keys.has(locatorKey(row))).length,
        issues: parsed.issues,
    };
}
