// Pure calculations shared by the progressive view and its Node tests.
import { search, compileQuery, createRowPredicate, createRowComparator } from './library-index.js';
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

export function scanExplanation(status) {
    const counts = status.metadataCounts ?? {};
    if (counts.scanActionable > 0) return 'Scan metadata for eligible tracks that still need inspection or checksums.';
    if (counts.error) return 'No tracks can be scanned now. Focus a failed track and use Retry metadata in its details.';
    if (counts.excluded) return 'No eligible tracks need scanning. Excluded tracks cannot be inspected; Show unplayable explains why.';
    if (counts.refreshPending) return 'Current track metadata is refreshing; Scan metadata has no additional work.';
    return 'No eligible tracks need metadata scanning.';
}

export function metadataHint(status) {
    const counts = status.metadataCounts ?? {};
    const sidecars = status.sidecarCounts ?? {};
    const parts = [];
    if (status.metadataIncomplete) {
        const reason = counts.unknown || counts.error
            ? `${counts.unknown ?? 0} tracks await metadata, ${counts.error ?? 0} have metadata errors.`
            : counts.refreshPending ? 'A previously verified track is awaiting its current-version refresh.' : 'Some tracks still need checksums for comments matching.';
        parts.push(`Metadata searches are incomplete: ${reason}`);
        if (counts.scanActionable > 0) parts.push('Use Scan metadata or play a track.');
        if (counts.error) parts.push('Focus a failed track and use Retry metadata in its details.');
    }
    if (counts.excluded) parts.push(`${counts.excluded} excluded tracks cannot be scanned; Show unplayable explains why.`);
    if (counts.hashesPending) parts.push(`${counts.hashesPending} playback digests pending (including superseded playback work).`);
    if (counts.refreshPending) parts.push(`${counts.refreshPending} current-version refreshes pending; previously verified metadata remains available until replacement.`);
    if (sidecars.awaitingHashes || sidecars.failedHashes || sidecars.excludedHashes) {
        parts.push(`Comments: ${sidecars.joined ?? 0} joined, ${sidecars.unmatched ?? 0} currently unmatched, ${sidecars.awaitingHashes ?? 0} awaiting metadata/hashes; ${sidecars.failedHashes ?? 0} failed hashes need row Retry metadata, ${sidecars.excludedHashes ?? 0} excluded hashes cannot be scanned.`);
    }
    if (!(counts.scanActionable > 0) && (counts.error || counts.excluded)) parts.push(scanExplanation(status));
    return parts.join(' ');
}

// Injected counters describe algorithmic work without retaining diagnostics in the UI.
export function createIncrementalViewState(library, { onOperation = () => {} } = {}) {
    const count = (name, amount = 1) => onOperation(name, amount);
    const root = { path: '', label: '', kind: 'dir', children: new Map(), trackCount: 0, references: 0 };
    const nodes = new Map([['', root]]), contributions = new Map(), included = new Set(), keys = new Map();
    let order = new Int32Array(), predicate, compare, dependencies, sortFields, options;
    const intersects = (fields, wanted) => fields.has('*') || [...wanted].some(field => fields.has(field));
    function structural(row, direction, affected) {
        count('treeContributions');
        const zip = row.memberPath || row.containerMember ? row.sourcePath : null;
        const nested = row.containerMember ? `${row.sourcePath}/${row.containerMember.replaceAll('\\', '/')}` : null;
        let parent = root, path = '';
        const apply = entry => { entry.references += direction; entry.trackCount += row.playable ? direction : 0; affected?.add(entry.path); };
        apply(root);
        for (const label of row.folder ? row.folder.split('/') : []) {
            path = path ? `${path}/${label}` : label;
            let entry = nodes.get(path);
            if (!entry) {
                entry = { path, label, kind: 'dir', children: new Map(), trackCount: 0, references: 0 };
                nodes.set(path, entry); parent.children.set(path, entry);
            }
            if (path === nested) entry.kind = 'nested';
            else if (path === zip) entry.kind = 'zip';
            apply(entry); parent = entry;
        }
    }
    const contribution = row => Object.fromEntries(['folder', 'playable', 'sourcePath', 'containerMember', 'memberPath'].map(field => [field, row[field]]));
    count('treeBuilds');
    for (const row of library.rows) { const value = contribution(row); contributions.set(row.id, value); structural(value, 1); }
    function sortKey(row) {
        return [options.sort.column, 'folder', 'name'].map(field => library.sortKeys[row.id]?.[field] ?? row[field]).concat([row.sourcePath, row.containerMember, row.memberPath]);
    }
    function full(query = '', nextOptions = {}) {
        options = { ...nextOptions, sort: nextOptions.sort ?? { column: 'name', descending: false } };
        const tokens = typeof query === 'string' ? compileQuery(query) : query;
        dependencies = new Set();
        if (!options.showUnplayable) dependencies.add('playable');
        if (options.scopeFolder) dependencies.add('folder');
        if (options.format) dependencies.add('format');
        const fieldsByToken = { path: ['sourcePath', 'containerMember', 'memberPath'], ch: ['channels'], text: ['comments'] };
        const freeTextFields = ['name', 'title', 'folder', 'format', 'tracker', ...(options.includeComments !== false ? ['comments'] : [])];
        for (const token of tokens) {
            const fields = token.field ? fieldsByToken[token.field] ?? [token.field] : freeTextFields;
            for (const field of fields) dependencies.add(field);
        }
        sortFields = new Set([options.sort.column, 'folder', 'name', 'sourcePath', 'containerMember', 'memberPath']);
        predicate = createRowPredicate(library, tokens, options);
        const comparator = createRowComparator(library, options.sort);
        compare = (left, right) => { count('comparisons'); return comparator(left, right); };
        included.clear(); keys.clear();
        const result = [];
        for (const row of library.rows) {
            count('rowEvaluations');
            if (predicate(row)) { included.add(row.id); result.push(row.id); keys.set(row.id, sortKey(row)); }
        }
        count('fullSorts'); result.sort(compare); order = Int32Array.from(result);
        return order;
    }
    function update(changes = []) {
        const combined = new Map();
        for (const change of changes) {
            const previous = combined.get(change.id);
            combined.set(change.id, { id: change.id, added: Boolean(previous?.added || change.added), fields: new Set([...(previous?.fields ?? []), ...change.fields]) });
        }
        const affected = new Set(), moved = new Set(), insert = [], display = new Set();
        let orderChanged = false;
        for (const { id, added, fields } of combined.values()) {
            const row = library.rows[id];
            if (!row) continue;
            display.add(id);
            if (added || !contributions.has(id) || intersects(fields, new Set(['folder', 'playable', 'sourcePath', 'containerMember', 'memberPath']))) {
                const previous = contributions.get(id), next = contribution(row);
                if (!previous || Object.keys(next).some(field => next[field] !== previous[field])) {
                    if (previous) structural(previous, -1, affected);
                    structural(next, 1, affected); contributions.set(id, next);
                }
            }
            const wasIncluded = included.has(id);
            let matches = wasIncluded;
            if (added || intersects(fields, dependencies)) { count('rowEvaluations'); matches = predicate(row); }
            let key = keys.get(id), changedKey = false;
            if (matches && (added || !wasIncluded || intersects(fields, sortFields))) {
                const next = sortKey(row);
                changedKey = !key || next.some((value, index) => value !== key[index]); key = next;
            }
            if (matches !== wasIncluded || matches && changedKey) {
                moved.add(id);
                if (matches) { included.add(id); keys.set(id, key); insert.push(id); }
                else { included.delete(id); keys.delete(id); }
            }
        }
        // Ancestors enter affected before their descendants. Retire empty branches from
        // the leaves up; keyed children avoid scanning unchanged siblings on removal.
        for (const path of [...affected].reverse()) {
            const entry = nodes.get(path);
            if (!path || entry.references) continue;
            const parentPath = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
            nodes.get(parentPath).children.delete(path); nodes.delete(path);
        }
        // One traversal/movement of the old result; unchanged rows are never re-sorted.
        if (moved.size) {
            count('batchSorts'); insert.sort(compare);
            const result = [], previous = order;
            const append = id => { if (previous[result.length] !== id) orderChanged = true; result.push(id); };
            let cursor = 0;
            for (const id of previous) {
                count('mergeRows');
                if (moved.has(id)) continue;
                while (cursor < insert.length && compare(insert[cursor], id) < 0) append(insert[cursor++]);
                append(id);
            }
            while (cursor < insert.length) append(insert[cursor++]);
            orderChanged ||= result.length !== previous.length;
            if (orderChanged) order = Int32Array.from(result);
        }
        return { order, orderChanged, affectedTreePaths: affected, changedRowIds: display };
    }
    return { full, update, get order() { return order; }, tree: root, nodes, includes: id => included.has(id) };
}

export function issuePage(issues, page = 0, { onOperation = () => {} } = {}) {
    const pages = Math.max(1, Math.ceil(issues.length / 100));
    const current = Math.max(0, Math.min(pages - 1, page));
    const items = issues.slice(current * 100, (current + 1) * 100);
    onOperation('issueMaterializations', items.length);
    return { items, page: current, pages, total: issues.length };
}

// The reader may snapshot controller issues; never call it on an unchanged status tick.
export function createIssuePageState({ onOperation = () => {} } = {}) {
    let previousRevision, currentPage = 0, rendered = false;
    return {
        update({ revision, expanded, readIssues, page = currentPage }) {
            if (revision !== previousRevision) { previousRevision = revision; rendered = false; }
            if (page !== currentPage) { currentPage = page; rendered = false; }
            if (!expanded || rendered) return null;
            const result = issuePage(readIssues(), currentPage, { onOperation });
            currentPage = result.page; rendered = true;
            return result;
        },
    };
}
