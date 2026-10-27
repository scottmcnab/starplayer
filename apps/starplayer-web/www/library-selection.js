// Selection is independent of the rendered window. Hidden ids survive all view changes.
export function createSelection() { return { selected: new Set(), anchor: null, focus: null }; }

export function selectRow(state, order, id, { toggle = false, extend = false } = {}) {
    if (!order.includes(id)) return state;
    const selected = new Set(state.selected);
    let anchor = state.anchor;
    if (extend) {
        let first = order.indexOf(anchor);
        const last = order.indexOf(id);
        if (first < 0) { first = last; anchor = id; }
        // Replace the visible selection, retaining selections outside this result.
        if (!toggle) for (const visible of order) selected.delete(visible);
        for (let index = Math.min(first, last); index <= Math.max(first, last); index += 1) selected.add(order[index]);
    } else {
        if (!toggle) selected.clear();
        if (toggle && selected.has(id)) selected.delete(id);
        else selected.add(id);
        anchor = id;
    }
    return { selected, anchor, focus: id };
}

export function selectAll(state, order) {
    return { ...state, selected: new Set([...state.selected, ...order]), focus: order.includes(state.focus) ? state.focus : order[0] ?? null };
}

export function deselectAll(state, order) {
    const selected = new Set(state.selected);
    for (const id of order) selected.delete(id);
    return { ...state, selected };
}

export function moveFocus(state, order, key, { extend = false, pageSize = 1 } = {}) {
    if (!order.length) return state;
    const current = order.indexOf(state.focus);
    let index = current < 0 ? 0 : current;
    if (key === 'Home') index = 0;
    else if (key === 'End') index = order.length - 1;
    else if (current >= 0) index += ({ ArrowUp: -1, ArrowDown: 1, PageUp: -pageSize, PageDown: pageSize })[key] ?? 0;
    index = Math.max(0, Math.min(order.length - 1, index));
    return selectRow(state, order, order[index], { extend });
}

// Exclusive end, clamped even after a filter shrinks a previously scrolled result.
export function virtualWindow(scrollTop, height, rowCount, rowHeight = 32, overscan = 5) {
    const visible = Math.min(190, Math.max(1, Math.ceil(height / rowHeight)));
    const firstVisible = Math.min(Math.max(0, Math.floor(scrollTop / rowHeight)), Math.max(0, rowCount - visible));
    const padding = Math.min(5, Math.max(0, overscan));
    return { first: Math.max(0, firstVisible - padding), last: Math.min(rowCount, firstVisible + visible + padding) };
}
