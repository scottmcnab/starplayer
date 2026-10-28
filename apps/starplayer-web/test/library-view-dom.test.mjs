import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createLibraryView } from '../www/library-view.js';
import { appendDiscoveredRows, createDiscoveredLibrary } from '../www/library-index.js';

// A bubbling DOM fixture exercises selection and rendering together. Native disclosure
// activation and layout use the same scenarios in the packaged Chromium harness.
class Element {
    constructor(tag, document) {
        this.tagName = tag.toUpperCase(); this.document = document; this.children = []; this.parent = null;
        this.dataset = {}; this.attributes = {}; this.style = {}; this.listeners = new Map(); this.classes = new Set();
        this.value = ''; this.checked = false; this.hidden = false; this.clientHeight = 320; this.scrollTop = 0;
        this.classList = { toggle: (name, value) => value ? this.classes.add(name) : this.classes.delete(name), remove: name => this.classes.delete(name) };
    }
    set className(value) { this.classes = new Set(value.split(' ')); }
    set textContent(value) { this.text = String(value); this.children = []; }
    get textContent() { return (this.text ?? '') + this.children.map(child => child.textContent).join(''); }
    get childNodes() { return this.children; }
    append(...children) {
        for (const child of children) {
            if (child.tagName === 'FRAGMENT') { this.append(...child.children); continue; }
            child.parent = this; this.children.push(child);
        }
    }
    replaceChildren(...children) { for (const child of this.children) child.parent = null; this.children = []; this.text = ''; this.append(...children); }
    setAttribute(name, value) { this.attributes[name] = String(value); }
    removeAttribute(name) { delete this.attributes[name]; }
    matches(selector) {
        if (selector.startsWith('.')) return this.classes.has(selector.slice(1));
        if (selector.startsWith('[data-')) return selector.slice(6, -1).replace(/-([a-z])/g, (_, letter) => letter.toUpperCase()) in this.dataset;
        return this.tagName === selector.toUpperCase();
    }
    closest(selector) { return this.matches(selector) ? this : this.parent?.closest(selector) ?? null; }
    contains(element) { return Boolean(element && (element === this || this.children.some(child => child.contains(element)))); }
    querySelectorAll(selector) { return this.children.flatMap(child => [...(child.matches(selector) ? [child] : []), ...child.querySelectorAll(selector)]); }
    querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
    addEventListener(type, listener) { if (!this.listeners.has(type)) this.listeners.set(type, []); this.listeners.get(type).push(listener); }
    emit(type, options = {}) {
        const event = { target: this, detail: 1, key: '', preventDefault() {}, stopPropagation() { this.stopped = true; }, ...options };
        for (let current = this; current && !event.stopped; current = current.parent) for (const listener of current.listeners.get(type) ?? []) listener(event);
    }
    click(options) { this.emit('click', options); }
    focus() { this.document.activeElement = this; }
    toggle() { this.checked = !this.checked; this.emit('change'); }
}
async function fixture(t) {
    const globals = ['document', 'Option', 'ResizeObserver', 'requestAnimationFrame', 'cancelAnimationFrame'];
    const previous = Object.fromEntries(globals.map(name => [name, globalThis[name]]));
    const document = { activeElement: null, createElement: tag => new Element(tag, document) };
    document.body = new Element('body', document);
    document.createDocumentFragment = () => new Element('fragment', document);
    document.createTextNode = text => { const node = new Element('text', document); node.textContent = text; return node; };
    document.getElementById = id => find(document.body, id);
    function find(node, id) { if (node.id === id) return node; for (const child of node.children) { const found = find(child, id); if (found) return found; } return null; }
    const element = name => document.getElementById(`library-${name}`);
    const html = await readFile(new URL('../www/index.html', import.meta.url), 'utf8');
    for (const match of html.matchAll(/<([\w-]+)[^>]*\bid="(library-[^"]+)"[^>]*>/g)) {
        const node = new Element(match[1], document); node.id = match[2]; node.open = /\bopen\b/.test(match[0]); document.body.append(node);
    }
    const reparent = (parent, child) => { child.parent.children = child.parent.children.filter(node => node !== child); parent.append(child); };
    for (const name of ['heading', 'rows']) reparent(element('grid'), element(name));
    reparent(element('details'), element('details-title')); reparent(element('details'), element('details-content'));
    globalThis.document = document;
    globalThis.Option = class extends Element { constructor(text, value) { super('option', document); this.textContent = text; this.value = value; } };
    globalThis.ResizeObserver = class { observe() {} };
    globalThis.requestAnimationFrame = () => 1; globalThis.cancelAnimationFrame = () => {};
    t.after(() => { for (const [name, value] of Object.entries(previous)) { if (value === undefined) delete globalThis[name]; else globalThis[name] = value; } });
    const library = createDiscoveredLibrary({ name: 'UI fixture' });
    appendDiscoveredRows(library, [
        { sourcePath: 'first.mod', format: 'mod', playable: true, metadataState: 'error', error: 'Specific inspection failure', instrumentNames: 'Composer' },
        { sourcePath: 'second.mod', format: 'mod', playable: true, instrumentNames: 'Second composer' },
        { sourcePath: 'third.mod', format: 'mod', playable: false, error: 'Encrypted archive member' },
    ]);
    const added = [], played = [];
    const view = createLibraryView({ playlist: { add: entries => added.push(entries) }, play: row => played.push(row.id) });
    view.setLibrary({ library, identity: { name: 'fixture', sha256: 'a'.repeat(64) } });
    const row = index => element('rows').children.find(node => node.dataset.rowId === String(index));
    return { element, row, view, added, played };
}

test('Library details hide after every deselection route and retain disclosure state across selection and repaint', async t => {
    const { element, row, view } = await fixture(t);
    const details = element('details'), grid = element('grid');
    assert.equal(details.hidden, true);
    row(0).click();
    assert.equal(details.hidden, false); assert.equal(details.open, true);
    assert.match(element('details-content').textContent, /Composer/);
    details.open = false;
    row(1).click();
    assert.equal(details.hidden, false); assert.equal(details.open, false);
    assert.match(element('details-title').textContent, /second.mod/);
    row(1).querySelector('[data-select]').toggle();
    assert.equal(details.hidden, true, 'stale focus after checkbox clearing is not selection');
    row(1).click(); row(1).click();
    assert.equal(details.hidden, true, 'repeat ordinary click clears sole selection');
    const heading = element('select-all');
    heading.toggle(); assert.equal(details.hidden, false);
    heading.toggle(); assert.equal(details.hidden, true);
    row(0).click(); row(0).querySelector('[data-select]').emit('keydown', { key: 'Escape' });
    assert.equal(details.hidden, true, 'Escape works from a row checkbox');
    row(0).click(); heading.emit('keydown', { key: 'Escape' });
    assert.equal(details.hidden, true, 'Escape works from the heading checkbox');
    grid.emit('keydown', { key: 'ArrowDown' });
    assert.equal(details.hidden, false); assert.equal(details.open, false);
    view.setPlaying(0);
    assert.equal(details.open, false, 'playback repaint preserves collapse choice');
});

test('Library error icon and filename targets retain full inspection and extraction diagnostics', async t => {
    const { element, row } = await fixture(t);
    element('unplayable').checked = true; element('unplayable').emit('change');
    for (const [index, reason] of [[0, 'Specific inspection failure'], [2, 'Encrypted archive member']]) {
        const marker = row(index).querySelector('.library-error-marker');
        assert.equal(marker.textContent, '!'); assert.equal(marker.title, reason);
        assert.equal(marker.attributes['role'], 'img'); assert.equal(marker.attributes['aria-label'], reason);
        assert.match(row(index).querySelector('.library-name-cell').title, new RegExp(reason));
    }
    assert.equal(row(1).querySelector('.library-error-marker'), null);
});
