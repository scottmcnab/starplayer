import { commentMatchLines, commentTokens, compileQuery, matchRanges } from './library-index.js';
import { createSelection, deselectAll, moveFocus, selectAll, selectRow, virtualWindow } from './library-selection.js';
import { captureAnchor, restoreAnchor, formatOptions, unplayableCount, folderAddLabel, selectedSnapshot, folderSnapshot, progressText, metadataHint, scanExplanation, createIncrementalViewState, createIssuePageState } from './library-view-state.js';

const ROW_HEIGHT = 32;
const COLUMNS = [['name', 'Name'], ['title', 'Title'], ['format', 'Format'], ['channels', 'Ch'], ['size', 'Size'], ['timestamp', 'Date'], ['folder', 'Folder']];
const element = name => document.getElementById(`library-${name}`);
function icon(markup) {
    const template = document.createElement('template');
    template.innerHTML = `<svg class="library-tree-icon" viewBox="0 0 16 16" aria-hidden="true">${markup}</svg>`;
    return template;
}
let FOLDER_ICON, ARCHIVE_ICON;
const SECTION_LABELS = { instrumentNames: ['#', 'Instrument'], sampleNames: ['smp ', 'Sample'], songMessage: ['msg ', 'Message line'] };
// Module text only ever reaches the page as text nodes; `ranges` come from `matchRanges`.
function highlight(text, ranges, separator = null) {
    const fragment = document.createDocumentFragment();
    const piece = value => document.createTextNode(separator === null ? value : value.replaceAll('\n', separator));
    let offset = 0;
    for (const [start, end] of ranges) {
        fragment.append(piece(text.slice(offset, start)));
        const mark = document.createElement('mark');
        mark.append(piece(text.slice(start, end)));
        fragment.append(mark);
        offset = end;
    }
    fragment.append(piece(text.slice(offset)));
    return fragment;
}
function lineLabel(hit, long = false) {
    const [short, title] = SECTION_LABELS[hit.section];
    const number = value => String(value + 1).padStart(2, '0');
    const lines = hit.first === hit.last ? number(hit.first) : `${number(hit.first)}–${number(hit.last)}`;
    return long ? `${title} ${lines}` : `${short}${lines}`;
}
// Starts a long line near its first match, so the cell's ellipsis cannot hide it.
function excerpt(hit) {
    const cut = hit.ranges.length && hit.ranges[0][0] > 16 ? hit.ranges[0][0] - 8 : 0;
    if (!cut) return hit;
    return { ...hit, text: `…${hit.text.slice(cut)}`, ranges: hit.ranges.map(([start, end]) => [start - cut + 1, end - cut + 1]) };
}
function setText(target, text) { if (target.textContent !== text) target.textContent = text; }
function node(tag, text, className) {
    const result = document.createElement(tag);
    if (text !== undefined) result.textContent = text;
    if (className) result.className = className;
    return result;
}

export function createLibraryView({ playlist, play, onOperation = () => {} }) {
    FOLDER_ICON ??= icon('<path d="M1.5 3.5h4.2l1.5 1.6h7.3v7.4h-13z"/>');
    ARCHIVE_ICON ??= icon('<path d="M2.5 1.5h11v13h-11z"/><path d="M8 1.5v2M8 4.5v2M8 7.5v1.5M6.8 9h2.4v3H6.8z"/>');
    const grid = element('grid');
    const viewport = element('viewport');
    const spacer = element('spacer');
    const rows = element('rows');
    const heading = element('heading');
    let context = null;
    let order = new Int32Array();
    let selection = createSelection();
    let sort = { column: 'name', descending: false };
    let scopeFolder = '';
    let playing = null;
    let tree = null;
    let viewState = null;
    const treeDOM = new Map();
    const selectedPlayable = new Set();
    const selectedInResult = new Set();
    let issueRevision = null, issuePageIndex = 0, issuePages = createIssuePageState({ onOperation });
    let activeTokens = [];
    let treeFocus = '';
    let treeIncomplete = null;
    let timer = null;
    let frame = null;
    const expanded = new Set();
    let detailsKey = null;
    let matchTokens = [];
    const matchCache = new Map();
    const popover = node('div', undefined, 'library-match-popover');
    popover.id = 'library-match-popover';
    popover.setAttribute('role', 'tooltip');
    popover.hidden = true;
    document.body.append(popover);
    let popoverTimer = null;
    let popoverRow = null;

    function matchesFor(id) {
        if (!matchCache.has(id)) matchCache.set(id, commentMatchLines(context.library.commentsFor(id), matchTokens));
        return matchCache.get(id);
    }
    function hidePopover() {
        clearTimeout(popoverTimer);
        popoverRow = null;
        popover.hidden = true;
    }
    function showPopover(line, pointerX) {
        const hits = matchesFor(Number(line.dataset.rowId));
        if (!hits.length) { hidePopover(); return; }
        const row = context.library.rows[Number(line.dataset.rowId)];
        const limit = 12;
        popover.replaceChildren(node('strong', [row.name, row.title].filter(Boolean).join(' — ')));
        for (const hit of hits.slice(0, limit)) {
            const entry = node('div', undefined, 'library-match-line');
            entry.append(node('span', lineLabel(hit, true), 'library-match-label'), highlight(hit.text, hit.ranges));
            popover.append(entry);
        }
        if (hits.length > limit) popover.append(node('p', `${hits.length - limit} more matching lines; focus the row to see all comment text.`, 'library-hint'));
        popover.hidden = false;
        const bounds = line.getBoundingClientRect();
        const width = popover.offsetWidth, height = popover.offsetHeight;
        popover.style.left = `${Math.max(8, Math.min(pointerX - 24, innerWidth - width - 8))}px`;
        popover.style.top = `${bounds.bottom + 4 + height > innerHeight ? Math.max(8, bounds.top - 4 - height) : bounds.bottom + 4}px`;
    }

    function renderDetails() {
        const pane = element('details');
        const row = selection.selected.has(selection.focus) ? context?.library.rows[selection.focus] : null;
        pane.hidden = !row || (!context.library.hasComments && !row.error);
        if (pane.hidden) { detailsKey = null; return; }
        const comments = context.library.commentsFor(row.id);
        const key = JSON.stringify([row.id, row.error, row.metadataState, row.name, row.title, comments, element('search').value, element('search-comments').checked]);
        if (key === detailsKey) return;
        detailsKey = key;
        element('details-title').textContent = [row.name, row.title].filter(Boolean).join(' — ');
        const content = element('details-content');
        const retryFocused = content.contains(document.activeElement);
        content.replaceChildren();
        const highlighted = text => highlight(text, matchTokens.length ? matchRanges(text, matchTokens) : []);
        if (row.error) {
            content.append(node('h4', row.metadataState === 'error' ? 'Metadata error' : 'Track unavailable'));
            content.append(node('p', row.error, 'library-track-error'));
            if (row.metadataState === 'error' && row.playable && context.discovery) {
                const retry = node('button', 'Retry metadata');
                retry.type = 'button';
                retry.dataset.retryMetadata = String(row.id);
                retry.setAttribute('aria-label', `Retry metadata for ${row.name}`);
                const retryContext = context;
                retry.onclick = () => {
                    if (context === retryContext) retryContext.discovery.retryMetadata({ sourcePath: row.sourcePath, containerMember: row.containerMember, memberPath: row.memberPath });
                };
                content.append(retry);
            }
        }
        for (const [field, title] of [['instrumentNames', 'Instrument names'], ['sampleNames', 'Sample names'], ['songMessage', 'Song message']]) {
            const text = comments[field];
            if (!text) continue;
            content.append(node('h4', title));
            const block = node('pre');
            if (field === 'songMessage') block.append(highlighted(text));
            else {
                const lines = text.split('\n');
                const digits = lines.length > 99 ? 3 : 2;
                lines.forEach((line, index) => {
                    block.append(document.createTextNode(`${String(index + 1).padStart(digits, '0')}  `), highlighted(line), document.createTextNode(index + 1 < lines.length ? '\n' : ''));
                });
            }
            content.append(block);
        }
        if (!content.childNodes.length) content.append(node('p', 'No comment text'));
        if (retryFocused) (content.querySelector('[data-retry-metadata]') ?? grid).focus({ preventScroll: true });
    }

    function addSelected() {
        if (context) playlist.add(selectedSnapshot(context.library, order, selection.selected, context.identity));
    }
    function playRow(id) {
        const row = context?.library.rows[id];
        if (row?.playable && order.includes(id)) play(row);
    }
    function reveal(id) {
        const index = order.indexOf(id);
        if (index < 0) return;
        const top = index * ROW_HEIGHT;
        if (top < viewport.scrollTop) viewport.scrollTop = top;
        else if (top + ROW_HEIGHT > viewport.scrollTop + viewport.clientHeight) viewport.scrollTop = top + ROW_HEIGHT - viewport.clientHeight;
    }
    function rowElement(row, index) {
        onOperation('rowDOMBuilds', 1);
        const line = node('div', undefined, 'library-track library-columns');
        line.id = `library-row-${row.id}`;
        line.dataset.rowId = String(row.id);
        line.setAttribute('role', 'row');
        line.setAttribute('aria-rowindex', String(index + 2));
        line.setAttribute('aria-selected', String(selection.selected.has(row.id)));
        line.classList.toggle('unplayable', !row.playable);
        line.classList.toggle('metadata-error', row.metadataState === 'error');
        if (row.error) line.title = row.error;
        line.classList.toggle('playing', row.id === playing);
        line.classList.toggle('focused', row.id === selection.focus);
        const selectionCell = node('label', undefined, 'library-selection-cell');
        selectionCell.setAttribute('role', 'gridcell');
        const checkbox = node('input');
        checkbox.type = 'checkbox';
        checkbox.dataset.select = 'true';
        checkbox.checked = selection.selected.has(row.id);
        checkbox.setAttribute('aria-label', `Select ${row.name}`);
        selectionCell.append(checkbox);
        line.append(selectionCell);
        for (const [column] of COLUMNS) {
            const cell = node('span', row[column] ?? '—');
            cell.setAttribute('role', 'gridcell');
            cell.title = row.error ? `${cell.textContent}\n${row.error}` : cell.textContent;
            if (column === 'name' && (row.error || !row.playable)) {
                const reason = row.error || `Unsupported format: ${row.format || 'unknown'}`;
                const marker = node('span', '!', 'library-error-marker');
                marker.title = reason;
                marker.setAttribute('role', 'img');
                marker.setAttribute('aria-label', reason);
                const name = node('span', cell.textContent, 'library-track-name');
                cell.className = 'library-name-cell';
                cell.replaceChildren(marker, name);
                cell.title = `${row.name}\n${reason}`;
            }
            line.append(cell);
        }
        if (matchTokens.length) {
            const cell = node('span', undefined, 'library-comment-cell');
            cell.setAttribute('role', 'gridcell');
            const hits = matchesFor(row.id);
            if (hits.length) {
                const first = excerpt(hits[0]);
                cell.append(node('span', lineLabel(first), 'library-match-label'), highlight(first.text, first.ranges, ' / '));
                if (hits.length > 1) cell.append(node('span', ` +${hits.length - 1}`, 'library-match-more'));
            }
            line.append(cell);
        }
        return line;
    }
    function renderRows() {
        if (frame !== null) cancelAnimationFrame(frame);
        frame = null;
        if (!context) return;
        const focusedCheckbox = rows.contains(document.activeElement) ? document.activeElement.closest('[data-row-id]')?.dataset.rowId : null;
        const window = virtualWindow(viewport.scrollTop, viewport.clientHeight, order.length, ROW_HEIGHT);
        const fragment = document.createDocumentFragment();
        for (let index = window.first; index < window.last; index += 1) {
            const row = context.library.rows[order[index]];
            fragment.append(rowElement(row, index));
        }
        rows.style.transform = `translateY(${window.first * ROW_HEIGHT}px)`;
        rows.replaceChildren(fragment);
        if (focusedCheckbox !== null) {
            const replacement = document.getElementById(`library-row-${focusedCheckbox}`)?.querySelector('[data-select]');
            (replacement ?? grid).focus({ preventScroll: true });
        }
        updateSelection(false);
    }
    function updateSelection(recount = true) {
        for (const line of rows.children) {
            const id = Number(line.dataset.rowId);
            line.setAttribute('aria-selected', String(selection.selected.has(id)));
            line.classList.toggle('focused', id === selection.focus);
            line.querySelector('[data-select]').checked = selection.selected.has(id);
        }
        // Never point at an element that has been virtualised away.
        const active = document.getElementById(`library-row-${selection.focus}`);
        if (active) grid.setAttribute('aria-activedescendant', active.id);
        else grid.removeAttribute('aria-activedescendant');
        if (recount) {
            selectedPlayable.clear();
            selectedInResult.clear();
            for (const id of selection.selected) if (viewState.includes(id)) {
                selectedInResult.add(id);
                if (context.library.rows[id].playable) selectedPlayable.add(id);
            }
        }
        selectAllCheckbox.disabled = order.length === 0;
        selectAllCheckbox.checked = order.length > 0 && selectedInResult.size === order.length;
        selectAllCheckbox.indeterminate = selectedInResult.size > 0 && selectedInResult.size < order.length;
        const selected = selectedPlayable.size;
        element('add-selected').disabled = selected === 0;
        renderDetails();
        element('selection-count').textContent = `${selected.toLocaleString('en-US')} playable selected in this result`;
    }
    function scheduleRows() { if (frame === null) frame = requestAnimationFrame(renderRows); }
    function refresh(preserveAnchor = true) {
        clearTimeout(timer);
        if (!context) return;
        const previousOrder = order;
        const anchor = preserveAnchor ? captureAnchor(previousOrder, viewport.scrollTop, ROW_HEIGHT) : null;
        const tokens = compileQuery(element('search').value);
        activeTokens = tokens;
        const includeComments = element('search-comments').checked;
        order = viewState.full(tokens, {
            scopeFolder, includeComments, showUnplayable: element('unplayable').checked, format: element('format').value, sort,
        });
        // The match column appears only while a query can match comment text.
        matchTokens = context.library.hasComments ? commentTokens(tokens, { includeComments }) : [];
        matchCache.clear();
        hidePopover();
        grid.classList.toggle('comment-matches', matchTokens.length > 0);
        grid.setAttribute('aria-colcount', String(matchTokens.length ? 9 : 8));
        element('count').textContent = `${order.length.toLocaleString('en-US')} of ${context.library.rows.length.toLocaleString('en-US')}`;
        element('scope').textContent = element('scope').title = scopeFolder || 'All folders';
        grid.setAttribute('aria-rowcount', String(order.length + 1));
        spacer.style.height = `${order.length * ROW_HEIGHT}px`;
        viewport.scrollTop = anchor ? restoreAnchor(anchor, previousOrder, order, viewport.clientHeight, ROW_HEIGHT) : 0;
        for (const header of heading.querySelectorAll('[data-column]')) header.setAttribute('aria-sort', header.dataset.column === sort.column ? sort.descending ? 'descending' : 'ascending' : 'none');
        updateSelection();
        renderRows();
    }
    // A WAI-ARIA tree with one tab stop: only expanded nodes are in the DOM, so document
    // order is visible order and arrow keys walk the rendered items.
    function treeItem(path) { return treeDOM.get(path); }
    function treeItems() { return [...element('tree').querySelectorAll('[role=treeitem]')]; }
    function focusTreeItem(item) {
        if (!item) return;
        for (const other of treeItems()) other.tabIndex = other === item ? 0 : -1;
        treeFocus = item.dataset.scope;
        item.focus({ preventScroll: true });
        item.firstElementChild.scrollIntoView({ block: 'nearest' });
    }
    function chooseFolder(path) {
        scopeFolder = path;
        for (const item of treeItems()) item.setAttribute('aria-selected', String(item.dataset.scope === path));
        refresh();
    }
    function addFolder(path) { if (context) playlist.add(folderSnapshot(context.library, path, context.identity)); }
    function toggleFolder(path, open = !expanded.has(path)) {
        if (open) expanded.add(path); else expanded.delete(path);
        const item = treeDOM.get(path), entry = viewState.nodes.get(path);
        if (!item || !entry) return;
        if (!open) {
            for (const descendant of item.querySelectorAll('[data-scope]')) treeDOM.delete(descendant.dataset.scope);
            item.querySelector('[role=group]')?.remove();
        }
        updateTreeItem(item, entry);
        if (open && entry.children.size && !item.querySelector(':scope > [role=group]')) {
            const group = node('ul'); group.setAttribute('role', 'group'); item.append(group);
            for (const child of entry.children.values()) appendTreeItem(group, child, Number(item.getAttribute('aria-level')) + 1);
        }
    }
    function treeStatusLabels(item, entry) {
        const incomplete = context.discoveryStatus && context.discoveryStatus.phase !== 'complete';
        item.setAttribute('aria-label', `${entry.label || 'All folders'}, ${entry.trackCount.toLocaleString('en-US')} ${incomplete ? 'discovered ' : ''}tracks`);
        const button = item.querySelector('[data-tree-add]');
        button.title = folderAddLabel(entry.path, context.discoveryStatus);
        button.setAttribute('aria-label', button.title);
    }
    function updateTreeItem(item, entry) {
        const level = Number(item.getAttribute('aria-level'));
        const open = level === 1 || expanded.has(entry.path);
        item.setAttribute('aria-selected', String(scopeFolder === entry.path));
        treeStatusLabels(item, entry);
        if (entry.children.size && level > 1) item.setAttribute('aria-expanded', String(open));
        else item.removeAttribute('aria-expanded');
        if (!entry.children.size) item.querySelector(':scope > [role=group]')?.remove();
        item.dataset.trackCount = String(entry.trackCount);
        item.firstElementChild.className = `library-tree-row ${entry.kind}`;
        setText(item.querySelector('.library-tree-count'), entry.trackCount.toLocaleString('en-US'));
        item.querySelector('[data-tree-add]').disabled = entry.trackCount === 0;
        return open;
    }
    function appendTreeItem(parent, entry, level) {
        const item = node('li');
        item.setAttribute('role', 'treeitem');
        item.setAttribute('aria-level', String(level));
        item.dataset.scope = entry.path;
        item.tabIndex = entry.path === treeFocus ? 0 : -1;
        const row = node('div');
        row.style.setProperty('--level', String(level - 1));
        const caret = node('span', undefined, 'library-tree-caret');
        caret.dataset.treeToggle = 'true';
        const symbol = (entry.kind === 'dir' ? FOLDER_ICON : ARCHIVE_ICON).content.cloneNode(true);
        const label = node('span', entry.label || 'All folders', 'library-tree-label');
        label.title = entry.path || 'All folders';
        const button = node('button', '+', 'library-tree-add');
        button.type = 'button'; button.tabIndex = -1; button.dataset.treeAdd = 'true';
        // The click always takes a fresh snapshot, including while discovery continues.
        row.append(caret, symbol, label, node('span', '', 'library-tree-count'), button);
        item.append(row); parent.append(item); treeDOM.set(entry.path, item);
        if (updateTreeItem(item, entry) && entry.children.size) {
            const group = node('ul'); group.setAttribute('role', 'group'); item.append(group);
            for (const child of entry.children.values()) appendTreeItem(group, child, level + 1);
        }
        return item;
    }
    function syncTree(paths) {
        if (!element('browse').checked) return;
        for (const path of paths) {
            const item = treeDOM.get(path);
            if (!item || viewState.nodes.has(path)) continue;
            const hadFocus = item.contains(document.activeElement);
            for (const descendant of item.querySelectorAll('[data-scope]')) treeDOM.delete(descendant.dataset.scope);
            treeDOM.delete(path); item.remove();
            if (treeFocus === path || treeFocus.startsWith(`${path}/`)) {
                while (treeFocus && !treeDOM.has(treeFocus)) treeFocus = treeFocus.includes('/') ? treeFocus.slice(0, treeFocus.lastIndexOf('/')) : '';
                const fallback = treeDOM.get(treeFocus);
                if (fallback) { fallback.tabIndex = 0; if (hadFocus) fallback.focus({ preventScroll: true }); }
            }
            onOperation('treeDOMUpdates', 1);
        }
        for (const path of paths) {
            const entry = viewState.nodes.get(path);
            let item = treeDOM.get(path);
            if (!entry) continue;
            if (!item && path) {
                const parentPath = path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '';
                const parent = treeDOM.get(parentPath);
                if (parent && (!parentPath || expanded.has(parentPath))) {
                    let group = parent.querySelector(':scope > [role=group]');
                    if (!group) { group = node('ul'); group.setAttribute('role', 'group'); parent.append(group); }
                    item = appendTreeItem(group, entry, Number(parent.getAttribute('aria-level')) + 1);
                }
            }
            if (item) { onOperation('treeDOMUpdates', 1); updateTreeItem(item, entry); }
        }
    }
    function renderTree() {
        element('tree').replaceChildren(); treeDOM.clear();
        if (tree) {
            appendTreeItem(element('tree'), tree, 1);
            if (!treeDOM.has(treeFocus)) treeFocus = treeDOM.has(scopeFolder) ? scopeFolder : '';
            treeDOM.get(treeFocus).tabIndex = 0;
        }
    }
    element('tree').addEventListener('mousedown', event => { if (event.detail > 1 || event.shiftKey) event.preventDefault(); });
    element('tree').addEventListener('click', event => {
        const item = event.target.closest('[role=treeitem]');
        if (!item) return;
        const path = item.dataset.scope;
        focusTreeItem(item);
        if (event.target.closest('[data-tree-add]')) addFolder(path);
        else if (event.target.closest('[data-tree-toggle]')) { if (item.hasAttribute('aria-expanded')) toggleFolder(path); }
        else chooseFolder(path);
    });
    element('tree').addEventListener('dblclick', event => {
        const item = event.target.closest('[role=treeitem]');
        if (item?.hasAttribute('aria-expanded') && !event.target.closest('[data-tree-add], [data-tree-toggle]')) toggleFolder(item.dataset.scope);
    });
    element('tree').addEventListener('keydown', event => {
        const item = event.target.closest('[role=treeitem]');
        if (!item || event.ctrlKey || event.metaKey || event.altKey) return;
        const items = treeItems();
        const index = items.indexOf(item);
        const path = item.dataset.scope;
        const expandable = item.hasAttribute('aria-expanded');
        const open = item.getAttribute('aria-expanded') === 'true';
        if (event.key === 'ArrowDown') focusTreeItem(items[index + 1]);
        else if (event.key === 'ArrowUp') focusTreeItem(items[index - 1]);
        else if (event.key === 'Home') focusTreeItem(items[0]);
        else if (event.key === 'End') focusTreeItem(items.at(-1));
        else if (event.key === 'ArrowRight') { if (expandable && !open) toggleFolder(path, true); else if (open) focusTreeItem(item.querySelector('[role=treeitem]')); }
        else if (event.key === 'ArrowLeft') { if (open) toggleFolder(path, false); else if (path) focusTreeItem(treeItem(path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '')); }
        else if (event.key === 'Enter' || event.key === ' ') chooseFolder(path);
        else if (event.key === '+' || event.key === 'Insert') addFolder(path);
        else return;
        event.preventDefault();
        event.stopPropagation();
    });
    const selectionHeader = node('label', undefined, 'library-selection-cell');
    selectionHeader.setAttribute('role', 'columnheader');
    selectionHeader.title = 'Select or clear all current results';
    const selectAllCheckbox = node('input');
    selectAllCheckbox.type = 'checkbox';
    selectAllCheckbox.id = 'library-select-all';
    selectAllCheckbox.disabled = true;
    selectAllCheckbox.setAttribute('aria-label', 'Select all tracks in current results');
    selectionHeader.append(selectAllCheckbox);
    heading.append(selectionHeader);
    selectAllCheckbox.addEventListener('change', () => {
        selection = selectAllCheckbox.checked ? selectAll(selection, order) : deselectAll(selection, order);
        updateSelection();
    });
    for (const [column, label] of COLUMNS) {
        const header = node('div');
        header.setAttribute('role', 'columnheader');
        header.dataset.column = column;
        const button = node('button', label);
        button.type = 'button';
        button.onclick = () => { sort = { column, descending: sort.column === column && !sort.descending }; refresh(); };
        header.append(button);
        heading.append(header);
    }
    const matchHeader = node('div', 'Comment match', 'library-comment-heading');
    matchHeader.setAttribute('role', 'columnheader');
    heading.append(matchHeader);
    element('search').addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(refresh, 80); });
    element('search-comments').addEventListener('change', () => refresh());
    element('format').addEventListener('change', () => refresh());
    element('unplayable').addEventListener('change', () => refresh());
    element('browse').addEventListener('change', () => {
        element('folders').hidden = !element('browse').checked;
        element('body').classList.toggle('browsing', element('browse').checked);
        scopeFolder = '';
        refresh();
        if (element('browse').checked) renderTree();
        else { element('tree').replaceChildren(); treeDOM.clear(); }
    });
    element('add-selected').onclick = addSelected;
    // Shift-click would otherwise extend a text selection across the rows (and Edge pops up
    // its Copy/Search mini menu); the click handler still moves focus to the grid.
    rows.addEventListener('mousedown', event => { if (!event.target.closest('.library-selection-cell') && (event.shiftKey || event.ctrlKey || event.metaKey || event.detail > 1)) event.preventDefault(); });
    rows.addEventListener('change', event => {
        if (!event.target.matches('[data-select]')) return;
        const id = Number(event.target.closest('[data-row-id]').dataset.rowId);
        selection = selectRow(selection, order, id, { toggle: true });
        updateSelection();
    });
    rows.addEventListener('click', event => {
        const line = event.target.closest('[data-row-id]');
        if (!line) return;
        const id = Number(line.dataset.rowId);
        if (event.target.closest('.library-selection-cell')) return;
        selection = selectRow(selection, order, id, { toggle: event.ctrlKey || event.metaKey, extend: event.shiftKey, clearSole: !event.ctrlKey && !event.metaKey && event.detail < 2 });
        grid.focus({ preventScroll: true });
        updateSelection();
    });
    rows.addEventListener('dblclick', event => {
        const line = event.target.closest('[data-row-id]');
        if (line && !event.target.closest('.library-selection-cell')) playRow(Number(line.dataset.rowId));
    });
    grid.addEventListener('keydown', event => {
        if (event.target !== grid && event.key !== 'Escape') return;
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') selection = selectAll(selection, order);
        else if (event.key === 'Escape') selection = createSelection();
        else if (['ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown'].includes(event.key)) {
            selection = moveFocus(selection, order, event.key, { extend: event.shiftKey, pageSize: Math.max(1, Math.floor(viewport.clientHeight / ROW_HEIGHT)) });
            reveal(selection.focus);
        } else if (event.key === 'Enter') playRow(selection.focus);
        else if (event.key === '+' || event.key === 'Insert') addSelected();
        else return;
        event.preventDefault();
        event.stopPropagation();
        // Focus and details updates keep painted rows intact; only virtualised-away
        // targets require a new window of rows.
        if (selection.focus === null || document.getElementById(`library-row-${selection.focus}`)) updateSelection();
        else { updateSelection(); renderRows(); }
    });
    // Mouse-only preview of a row's matching comment lines; the details pane is the
    // keyboard and screen-reader route to the same text.
    rows.addEventListener('mouseover', event => {
        const line = event.target.closest('[data-row-id]');
        if (event.target.closest('.library-error-marker')) { hidePopover(); return; }
        if (!matchTokens.length || !line || line === popoverRow) return;
        hidePopover();
        popoverRow = line;
        const pointerX = event.clientX;
        popoverTimer = setTimeout(() => { if (popoverRow === line && line.isConnected) showPopover(line, pointerX); }, 250);
    });
    rows.addEventListener('mouseleave', hidePopover);
    viewport.addEventListener('scroll', hidePopover, { passive: true });
    viewport.addEventListener('scroll', scheduleRows, { passive: true });
    new ResizeObserver(scheduleRows).observe(viewport);
    function renderOptions() {
        const select = element('format');
        const selected = select.value;
        const options = formatOptions(context.library, selected);
        const key = options.map(option => option.value).join('|');
        if (select.dataset.options !== key) {
            select.replaceChildren(new Option('All formats', ''), ...options.map(option => new Option(option.label, option.value)));
            select.dataset.options = key;
            select.value = selected;
        }
        element('comments-toggle').hidden = !context.library.hasComments;
        setText(element('unplayable-count'), unplayableCount(context.library).toLocaleString('en-US'));
    }
    function renderProgress() {
        const status = context.discoveryStatus;
        const incomplete = Boolean(status && status.phase !== 'complete');
        if (treeIncomplete !== incomplete) {
            treeIncomplete = incomplete;
            for (const [path, item] of treeDOM) { treeStatusLabels(item, viewState.nodes.get(path)); onOperation('treeStatusUpdates', 1); }
        }
        const discovered = context.mode === 'discovered' && !!status;
        element('progress').hidden = !discovered;
        element('metadata-hint').hidden = !discovered;
        // Source controls own Browse folder visibility, including indexed local sources.
        if (!discovered) for (const name of ['stop-discovery', 'resume-discovery', 'retry-discovery', 'scan-metadata', 'cancel-metadata']) element(name).hidden = true;
        const revision = discovered ? status.issuesRevision : context.library.issues;
        const issueCount = discovered ? status.issueCount : (context.library.issues?.length ?? 0);
        const issuePane = element('issues');
        issuePane.hidden = !issueCount;
        setText(element('issues-summary'), `${issueCount.toLocaleString('en-US')} library issues`);
        issueRevision = revision;
        renderIssues();
        if (!discovered) return;
        setText(element('progress'), progressText(status));
        const hint = metadataHint(status);
        setText(element('metadata-hint'), hint);
        element('metadata-hint').hidden = !hint;
        element('stop-discovery').hidden = status.phase !== 'running';
        element('resume-discovery').hidden = status.phase !== 'paused';
        element('retry-discovery').hidden = status.phase !== 'failed' && !status.runtimeError;
        element('retry-discovery').title = status.runtimeError ? 'Retry failed background worker jobs' : 'Retry discovery';
        const scanning = ['running', 'waiting', 'suspended'].includes(status.metadataPhase);
        element('scan-metadata').hidden = scanning;
        element('scan-metadata').disabled = !(status.metadataCounts?.scanActionable > 0);
        element('scan-metadata').title = scanExplanation(status);
        element('cancel-metadata').hidden = !scanning;
    }
    function renderIssues() {
        if (!context) return;
        const result = issuePages.update({ revision: issueRevision, expanded: element('issues').open, page: issuePageIndex,
            readIssues: () => context.mode === 'discovered' ? context.discoveryStatus.issues : context.library.issues ?? [] });
        if (!result) return;
        issuePageIndex = result.page;
        element('issues-list').replaceChildren(...result.items.map(issue => node('li', `${issue.path ?? `Line ${issue.line}`}: ${issue.reason}`)));
        setText(element('issues-page'), `Page ${result.page + 1} of ${result.pages}; ${result.total.toLocaleString('en-US')} issues`);
        // aria-disabled keeps keyboard focus on navigation when the boundary changes.
        element('issues-previous').setAttribute('aria-disabled', String(!result.page));
        element('issues-next').setAttribute('aria-disabled', String(result.page + 1 === result.pages));
    }
    element('issues').addEventListener('toggle', renderIssues);
    for (const [name, direction] of [['previous', -1], ['next', 1]]) element(`issues-${name}`).onclick = () => {
        if (element(`issues-${name}`).getAttribute('aria-disabled') === 'true') return;
        issuePageIndex += direction; renderIssues();
    };
    function incremental(changes) {
        const previousOrder = order;
        const anchor = captureAnchor(order, viewport.scrollTop, ROW_HEIGHT);
        const result = viewState.update(changes);
        order = result.order;
        for (const id of result.changedRowIds) {
            matchCache.delete(id);
            if (selection.selected.has(id) && viewState.includes(id)) selectedInResult.add(id);
            else selectedInResult.delete(id);
            if (selection.selected.has(id) && viewState.includes(id) && context.library.rows[id].playable) selectedPlayable.add(id);
            else selectedPlayable.delete(id);
        }
        if (changes.some(change => change.added || change.fields.some(field => ['*', 'format', 'playable', 'comments'].includes(field)))) renderOptions();
        const tokens = activeTokens;
        const nextTokens = context.library.hasComments ? commentTokens(tokens, { includeComments: element('search-comments').checked }) : [];
        const columnsChanged = Boolean(nextTokens.length) !== Boolean(matchTokens.length);
        matchTokens = nextTokens;
        grid.classList.toggle('comment-matches', matchTokens.length > 0);
        grid.setAttribute('aria-colcount', String(matchTokens.length ? 9 : 8));
        syncTree(result.affectedTreePaths);
        setText(element('count'), `${order.length.toLocaleString('en-US')} of ${context.library.rows.length.toLocaleString('en-US')}`);
        if (result.orderChanged) {
            grid.setAttribute('aria-rowcount', String(order.length + 1));
            spacer.style.height = `${order.length * ROW_HEIGHT}px`;
            onOperation('anchorRestores', 1);
            viewport.scrollTop = restoreAnchor(anchor, previousOrder, order, viewport.clientHeight, ROW_HEIGHT);
        }
        hidePopover();
        if (result.orderChanged || columnsChanged) renderRows();
        else {
            for (const line of [...rows.children]) if (result.changedRowIds.has(Number(line.dataset.rowId))) {
                const focused = line.contains(document.activeElement);
                const replacement = rowElement(context.library.rows[Number(line.dataset.rowId)], Number(line.getAttribute('aria-rowindex')) - 2);
                line.replaceWith(replacement);
                if (focused) replacement.querySelector('[data-select]').focus({ preventScroll: true });
            }
            updateSelection(false);
        }
    }
    return {
        setLibrary(next) {
            clearTimeout(timer);
            if (frame !== null) cancelAnimationFrame(frame);
            frame = null; hidePopover();
            context = next;
            viewState = createIncrementalViewState(context.library, { onOperation });
            selectedPlayable.clear();
            selectedInResult.clear();
            issueRevision = null; issuePageIndex = 0; issuePages = createIssuePageState({ onOperation });
            detailsKey = null;
            element('comments-toggle').hidden = !context.library.hasComments;
            selection = createSelection();
            playing = null;
            scopeFolder = '';
            tree = viewState.tree;
            element('tree').replaceChildren(); treeDOM.clear();
            treeFocus = ''; treeIncomplete = null;
            expanded.clear();
            element('browse').checked = false;
            element('folders').hidden = true;
            element('body').classList.remove('browsing');
            element('search').value = '';
            element('format').value = '';
            delete element('format').dataset.options;
            renderOptions();
            renderProgress();
            element('browser').hidden = false;
            element('panel').open = true;
            refresh(false);
        },
        updateLibrary(next, change = {}) {
            if (!context || next !== context) return;
            const changes = change.rowChanges ?? (change.changedRowIds ?? []).map(id => ({ id, added: false, fields: ['*'] }));
            if (changes.length) incremental(changes);
            renderProgress();
        },
        refreshComments() { if (context) { renderOptions(); refresh(true); renderProgress(); } },
        setPlaying(id) { playing = id; if (id !== null) reveal(id); renderRows(); },
    };
}
