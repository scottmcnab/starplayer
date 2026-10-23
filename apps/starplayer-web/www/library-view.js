import { buildTree, commentMatchLines, commentTokens, compileQuery, matchRanges, search } from './library-index.js';
import { createSelection, moveFocus, selectAll, selectRow, virtualWindow } from './library-selection.js';
import { libraryLocator } from './playlist.js';

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
function node(tag, text, className) {
    const result = document.createElement(tag);
    if (text !== undefined) result.textContent = text;
    if (className) result.className = className;
    return result;
}

export function createLibraryView({ playlist, play }) {
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
    let treeFocus = '';
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
        pane.hidden = !context?.library.hasComments || selection.focus === null;
        if (pane.hidden) { detailsKey = null; return; }
        const key = `${selection.focus}\n${element('search').value}\n${element('search-comments').checked}`;
        if (key === detailsKey) return;
        detailsKey = key;
        const row = context.library.rows[selection.focus];
        if (!row) { pane.hidden = true; return; }
        element('details-title').textContent = [row.name, row.title].filter(Boolean).join(' — ');
        const content = element('details-content');
        content.replaceChildren();
        const highlighted = text => highlight(text, matchTokens.length ? matchRanges(text, matchTokens) : []);
        const comments = context.library.commentsFor(row.id);
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
    }

    function add(ids) {
        if (!context) return;
        playlist.add(Array.from(ids, id => context.library.rows[id]).filter(row => row.playable).map(row => ({
            locator: libraryLocator(row, context.identity),
            display: { name: row.name, title: row.title, format: row.format, folder: row.folder },
        })));
    }
    function addSelected() { add(order.filter(id => selection.selected.has(id))); }
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
    function renderRows() {
        frame = null;
        if (!context) return;
        const window = virtualWindow(viewport.scrollTop, viewport.clientHeight, order.length, ROW_HEIGHT);
        const fragment = document.createDocumentFragment();
        for (let index = window.first; index < window.last; index += 1) {
            const row = context.library.rows[order[index]];
            const line = node('div', undefined, 'library-track library-columns');
            line.id = `library-row-${row.id}`;
            line.dataset.rowId = String(row.id);
            line.setAttribute('role', 'row');
            line.setAttribute('aria-rowindex', String(index + 2));
            line.setAttribute('aria-selected', String(selection.selected.has(row.id)));
            line.classList.toggle('unplayable', !row.playable);
            line.classList.toggle('playing', row.id === playing);
            line.classList.toggle('focused', row.id === selection.focus);
            for (const [column] of COLUMNS) {
                const cell = node('span', row[column] ?? '—');
                cell.setAttribute('role', 'gridcell');
                cell.title = cell.textContent;
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
            const cell = node('span');
            cell.setAttribute('role', 'gridcell');
            const button = node('button', '+');
            button.type = 'button';
            button.tabIndex = -1;
            button.disabled = !row.playable;
            button.setAttribute('aria-label', `Add ${row.name}`);
            button.dataset.add = 'true';
            cell.append(button);
            line.append(cell);
            fragment.append(line);
        }
        rows.style.transform = `translateY(${window.first * ROW_HEIGHT}px)`;
        rows.replaceChildren(fragment);
        updateSelection();
    }
    function updateSelection() {
        for (const line of rows.children) {
            const id = Number(line.dataset.rowId);
            line.setAttribute('aria-selected', String(selection.selected.has(id)));
            line.classList.toggle('focused', id === selection.focus);
        }
        // Never point at an element that has been virtualised away.
        const active = document.getElementById(`library-row-${selection.focus}`);
        if (active) grid.setAttribute('aria-activedescendant', active.id);
        else grid.removeAttribute('aria-activedescendant');
        const selected = order.reduce((count, id) => count + Number(selection.selected.has(id) && context.library.rows[id].playable), 0);
        element('add-selected').disabled = selected === 0;
        renderDetails();
        element('selection-count').textContent = `${selected.toLocaleString('en-US')} playable selected in this result`;
    }
    function scheduleRows() { if (frame === null) frame = requestAnimationFrame(renderRows); }
    function refresh() {
        clearTimeout(timer);
        if (!context) return;
        const tokens = compileQuery(element('search').value);
        const includeComments = element('search-comments').checked;
        order = search(context.library, tokens, {
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
        viewport.scrollTop = 0;
        for (const header of heading.querySelectorAll('[data-column]')) header.setAttribute('aria-sort', header.dataset.column === sort.column ? sort.descending ? 'descending' : 'ascending' : 'none');
        renderRows();
    }
    // A WAI-ARIA tree with one tab stop: only expanded nodes are in the DOM, so document
    // order is visible order and arrow keys walk the rendered items.
    function treeItem(path) { return element('tree').querySelector(`[data-scope="${CSS.escape(path)}"]`); }
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
    function addFolder(path) { add(search(context.library, '', { scopeFolder: path, sort: { column: 'folder', descending: false } })); }
    function toggleFolder(path, open = !expanded.has(path)) {
        if (open) expanded.add(path); else expanded.delete(path);
        renderTree();
    }
    function renderTree() {
        const pane = element('tree');
        const hadFocus = pane.contains(document.activeElement);
        pane.replaceChildren();
        if (!tree) return;
        function append(parent, entry, level) {
            const open = level === 1 || expanded.has(entry.path);
            const item = node('li');
            item.setAttribute('role', 'treeitem');
            item.setAttribute('aria-level', String(level));
            item.setAttribute('aria-selected', String(scopeFolder === entry.path));
            item.setAttribute('aria-label', `${entry.label || 'All folders'}, ${entry.trackCount.toLocaleString('en-US')} tracks`);
            if (entry.children.length && level > 1) item.setAttribute('aria-expanded', String(open));
            item.dataset.scope = entry.path;
            item.tabIndex = entry.path === treeFocus ? 0 : -1;
            const row = node('div', undefined, `library-tree-row ${entry.kind}`);
            row.style.setProperty('--level', String(level - 1));
            const caret = node('span', undefined, 'library-tree-caret');
            caret.dataset.treeToggle = 'true';
            const icon = (entry.kind === 'dir' ? FOLDER_ICON : ARCHIVE_ICON).content.cloneNode(true);
            const label = node('span', entry.label || 'All folders', 'library-tree-label');
            label.title = entry.path || 'All folders';
            const count = node('span', entry.trackCount.toLocaleString('en-US'), 'library-tree-count');
            const addButton = node('button', '+', 'library-tree-add');
            addButton.type = 'button';
            addButton.tabIndex = -1;
            addButton.disabled = entry.trackCount === 0;
            addButton.dataset.treeAdd = 'true';
            addButton.setAttribute('aria-label', `Add folder ${entry.path || 'All folders'}`);
            addButton.title = 'Add this folder to the playlist';
            row.append(caret, icon, label, count, addButton);
            item.append(row);
            if (entry.children.length && open) {
                const group = node('ul');
                group.setAttribute('role', 'group');
                for (const child of entry.children) append(group, child, level + 1);
                item.append(group);
            }
            parent.append(item);
        }
        append(pane, tree, 1);
        if (!treeItem(treeFocus)) treeFocus = scopeFolder && treeItem(scopeFolder) ? scopeFolder : '';
        treeItem(treeFocus).tabIndex = 0;
        if (hadFocus) focusTreeItem(treeItem(treeFocus));
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
    const actionHeader = node('div', 'Add');
    actionHeader.setAttribute('role', 'columnheader');
    heading.append(actionHeader);
    element('search').addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(refresh, 80); });
    element('search-comments').addEventListener('change', refresh);
    element('format').addEventListener('change', refresh);
    element('unplayable').addEventListener('change', refresh);
    element('browse').addEventListener('change', () => {
        element('folders').hidden = !element('browse').checked;
        element('body').classList.toggle('browsing', element('browse').checked);
        scopeFolder = '';
        refresh();
        if (element('browse').checked) { tree ??= buildTree(context.library); renderTree(); }
    });
    element('add-selected').onclick = addSelected;
    // Shift-click would otherwise extend a text selection across the rows (and Edge pops up
    // its Copy/Search mini menu); the click handler still moves focus to the grid.
    rows.addEventListener('mousedown', event => { if (event.shiftKey || event.ctrlKey || event.metaKey || event.detail > 1) event.preventDefault(); });
    rows.addEventListener('click', event => {
        const line = event.target.closest('[data-row-id]');
        if (!line) return;
        const id = Number(line.dataset.rowId);
        if (event.target.closest('[data-add]')) {
            if (!selection.selected.has(id)) selection = selectRow(selection, order, id);
            addSelected();
            updateSelection();
            return;
        }
        selection = selectRow(selection, order, id, { toggle: event.ctrlKey || event.metaKey, extend: event.shiftKey });
        grid.focus({ preventScroll: true });
        updateSelection();
    });
    rows.addEventListener('dblclick', event => {
        const line = event.target.closest('[data-row-id]');
        if (line && !event.target.closest('[data-add]')) playRow(Number(line.dataset.rowId));
    });
    grid.addEventListener('keydown', event => {
        if (event.target !== grid) return;
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
        else renderRows();
    });
    // Mouse-only preview of a row's matching comment lines; the details pane is the
    // keyboard and screen-reader route to the same text.
    rows.addEventListener('mouseover', event => {
        const line = event.target.closest('[data-row-id]');
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
    return {
        setLibrary(next) {
            context = next;
            detailsKey = null;
            element('comments-toggle').hidden = !context.library.hasComments;
            selection = createSelection();
            playing = null;
            scopeFolder = '';
            tree = null;
            treeFocus = '';
            expanded.clear();
            element('browse').checked = false;
            element('folders').hidden = true;
            element('body').classList.remove('browsing');
            element('search').value = '';
            element('format').replaceChildren(new Option('All formats', ''), ...[...new Set(context.library.rows.map(row => row.format))].sort().map(format => new Option(format.toUpperCase(), format)));
            element('browser').hidden = false;
            element('panel').open = true;
            refresh();
        },
        refreshComments() { detailsKey = null; element('comments-toggle').hidden = !context.library.hasComments; refresh(); },
        setPlaying(id) { playing = id; if (id !== null) reveal(id); renderRows(); },
    };
}
