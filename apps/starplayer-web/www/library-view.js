import { buildTree, compileQuery, search } from './library-index.js';
import { createSelection, moveFocus, selectAll, selectRow, virtualWindow } from './library-selection.js';
import { libraryLocator } from './playlist.js';

const ROW_HEIGHT = 32;
const COLUMNS = [['name', 'Name'], ['title', 'Title'], ['format', 'Format'], ['channels', 'Ch'], ['size', 'Size'], ['timestamp', 'Date'], ['folder', 'Folder']];
const element = name => document.getElementById(`library-${name}`);
function node(tag, text, className) {
    const result = document.createElement(tag);
    if (text !== undefined) result.textContent = text;
    if (className) result.className = className;
    return result;
}

export function createLibraryView({ playlist, play }) {
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
    let timer = null;
    let frame = null;
    const expanded = new Set();

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
        element('selection-count').textContent = `${selected.toLocaleString('en-US')} playable selected in this result`;
    }
    function scheduleRows() { if (frame === null) frame = requestAnimationFrame(renderRows); }
    function refresh() {
        clearTimeout(timer);
        if (!context) return;
        order = search(context.library, compileQuery(element('search').value), {
            scopeFolder, showUnplayable: element('unplayable').checked, format: element('format').value, sort,
        });
        element('count').textContent = `${order.length.toLocaleString('en-US')} of ${context.library.rows.length.toLocaleString('en-US')}`;
        element('scope').textContent = scopeFolder || 'All folders';
        grid.setAttribute('aria-rowcount', String(order.length + 1));
        spacer.style.height = `${order.length * ROW_HEIGHT}px`;
        viewport.scrollTop = 0;
        for (const header of heading.querySelectorAll('[data-column]')) header.setAttribute('aria-sort', header.dataset.column === sort.column ? sort.descending ? 'descending' : 'ascending' : 'none');
        renderRows();
    }
    function renderTree() {
        const pane = element('tree');
        const focused = pane.contains(document.activeElement) ? document.activeElement : null;
        const focusPath = focused?.dataset.treePath;
        const focusAction = focused?.dataset.treeAction;
        pane.replaceChildren();
        function append(parent, children) {
            for (const child of children) {
                const item = node('li');
                const bar = node('div', undefined, `library-folder-node ${child.kind}`);
                const expand = node('button', expanded.has(child.path) ? '−' : '+');
                expand.type = 'button';
                expand.disabled = !child.children.length;
                expand.setAttribute('aria-label', `Expand ${child.label}`);
                if (child.children.length) expand.setAttribute('aria-expanded', String(expanded.has(child.path)));
                expand.onclick = () => { if (expanded.has(child.path)) expanded.delete(child.path); else expanded.add(child.path); renderTree(); };
                const choose = node('button', `${child.kind === 'nested' ? 'Nested ZIP' : child.kind === 'zip' ? 'ZIP' : 'Folder'}: ${child.label} (${child.trackCount})`);
                choose.type = 'button';
                choose.dataset.scope = child.path;
                choose.setAttribute('aria-pressed', String(scopeFolder === child.path));
                choose.onclick = () => { scopeFolder = child.path; refresh(); renderTree(); };
                const addFolder = node('button', 'Add folder');
                addFolder.type = 'button';
                addFolder.disabled = child.trackCount === 0;
                addFolder.setAttribute('aria-label', `Add folder ${child.path}`);
                addFolder.onclick = () => add(search(context.library, '', { scopeFolder: child.path, sort: { column: 'folder', descending: false } }));
                for (const [action, button] of [['expand', expand], ['choose', choose], ['add', addFolder]]) {
                    button.dataset.treePath = child.path;
                    button.dataset.treeAction = action;
                }
                bar.append(expand, choose, addFolder);
                item.append(bar);
                if (expanded.has(child.path)) {
                    const list = node('ul');
                    append(list, child.children);
                    item.append(list);
                }
                parent.append(item);
            }
        }
        if (tree) append(pane, tree.children);
        if (focused) [...pane.querySelectorAll('button')].find(button => button.dataset.treePath === focusPath && button.dataset.treeAction === focusAction)?.focus({ preventScroll: true });
    }
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
    const actionHeader = node('div', 'Add');
    actionHeader.setAttribute('role', 'columnheader');
    heading.append(actionHeader);
    element('search').addEventListener('input', () => { clearTimeout(timer); timer = setTimeout(refresh, 80); });
    element('format').addEventListener('change', refresh);
    element('unplayable').addEventListener('change', refresh);
    element('browse').addEventListener('change', () => {
        element('folders').hidden = !element('browse').checked;
        scopeFolder = '';
        if (element('browse').checked) { tree ??= buildTree(context.library); renderTree(); }
        refresh();
    });
    element('all').onclick = () => { scopeFolder = ''; refresh(); renderTree(); };
    element('add-selected').onclick = addSelected;
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
        renderRows();
    });
    viewport.addEventListener('scroll', scheduleRows, { passive: true });
    new ResizeObserver(scheduleRows).observe(viewport);
    return {
        setLibrary(next) {
            context = next;
            selection = createSelection();
            playing = null;
            scopeFolder = '';
            tree = null;
            expanded.clear();
            element('browse').checked = false;
            element('folders').hidden = true;
            element('search').value = '';
            element('format').replaceChildren(new Option('All formats', ''), ...[...new Set(context.library.rows.map(row => row.format))].sort().map(format => new Option(format.toUpperCase(), format)));
            element('browser').hidden = false;
            element('panel').open = true;
            refresh();
        },
        setPlaying(id) { playing = id; if (id !== null) reveal(id); renderRows(); },
    };
}
