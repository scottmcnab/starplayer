// Pure library/playlist data layer. No filesystem, DOM, or playback ownership.
const knownColumns = new Set(('source_path container_member member_path entry_kind extension format_by_extension size compressed_size crc32 timestamp compression_method encrypted member_path_raw_hex source_url status error sha256 crc_verified format_detected format_variant channels title tracker instrument_slots instruments samples sample_bytes extended_status extended_error library instrument_names sample_names song_message').split(' '));
const playableFormats = new Set(['mod', 's3m', 'mtm', 'xm', 'it']);
const modelStates = new WeakMap();

// This is the existing playlist membership tuple; rawHex selects bytes within a locator,
// but is not part of its identity. Preserve spelling and case, including archive slashes.
export function locatorKey({ sourcePath, containerMember = '', memberPath = '' }) {
    return JSON.stringify([sourcePath, containerMember, memberPath]);
}

function rowPaths(sourcePath, containerMember, memberPath) {
    const innermost = memberPath || containerMember || sourcePath;
    const name = innermost.split(/[\\/]/).at(-1);
    const fullPath = [sourcePath, containerMember, memberPath].filter(Boolean).join('/').replaceAll('\\', '/');
    const folder = fullPath.slice(0, fullPath.length - name.length).replace(/\/$/, '');
    const extension = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1).toLowerCase() : '';
    return { name, folder, extension };
}

export function parseCsv(text) {
    const rows = [];
    const lines = [];
    let fields = [];
    let pieces = [];
    let quoted = false;
    let closed = false;
    let line = 1;
    let rowLine = 1;
    let start = text.charCodeAt(0) === 0xFEFF ? 1 : 0;
    for (let index = start; index < text.length; index += 1) {
        const character = text[index];
        if (quoted) {
            if (character === '"') {
                pieces.push(text.slice(start, index));
                if (text[index + 1] === '"') { pieces.push('"'); index += 1; }
                else { quoted = false; closed = true; }
                start = index + 1;
            } else if (character === '\n' || (character === '\r' && text[index + 1] !== '\n')) { line += 1; }
            continue;
        }
        if (character === '"' && index === start && !closed && pieces.length === 0) { quoted = true; start = index + 1; continue; }
        if (character === ',' || character === '\n' || character === '\r') {
            pieces.push(text.slice(start, index));
            fields.push(pieces.join(''));
            pieces = [];
            closed = false;
            if (character !== ',') {
                rows.push(fields); lines.push(rowLine); fields = [];
                if (character === '\r' && text[index + 1] === '\n') index += 1;
                line += 1; rowLine = line;
            }
            start = index + 1;
        } else if (closed || character === '"') { throw new Error(`Invalid CSV quoting at line ${line}`); }
    }
    if (quoted) throw new Error(`Unterminated CSV field at line ${rowLine}`);
    if (start < text.length || fields.length || pieces.length || closed) {
        pieces.push(text.slice(start)); fields.push(pieces.join('')); rows.push(fields); lines.push(rowLine);
    }
    Object.defineProperty(rows, 'lines', { value: lines });
    return rows;
}

function headers(records) {
    const columns = (records[0] || []).map(column => column.trim().toLowerCase());
    const positions = new Map();
    columns.forEach((column, index) => {
        if (!knownColumns.has(column)) return;
        if (positions.has(column)) throw new Error(`Duplicate CSV header: ${column}`);
        positions.set(column, index);
    });
    if (!positions.has('source_path')) throw new Error('Missing CSV header: source_path');
    return { columns, positions };
}

function invalidPath(path) {
    if (!path.trim()) return 'blank source_path';
    if (path.startsWith('/') || /^[a-z]:/i.test(path) || /[\\\0]/.test(path) || path.split('/').some(part => !part || part === '.' || part === '..')) return `invalid relative path: ${path}`;
    return null;
}

function readRows(text, keepDuplicates) {
    const records = parseCsv(text);
    const { columns, positions } = headers(records);
    const rows = [], issues = [], keys = new Set();
    for (let index = 1; index < records.length; index += 1) {
        const fields = records[index];
        if (fields.length === 1 && fields[0] === '') continue;
        const get = column => fields[positions.get(column)] || '';
        const line = records.lines[index];
        if (get('entry_kind').trim().toLowerCase() === 'directory') continue;
        if (['status', 'extended_status'].some(column => get(column).trim() && get(column).trim().toLowerCase() !== 'ok')) continue;
        const optionalPath = column => get(column).trim() ? get(column) : '';
        const sourcePath = get('source_path'), containerMember = optionalPath('container_member'), memberPath = optionalPath('member_path');
        // Only `source_path` reaches the filesystem or the server. Member names are looked up
        // inside the archive, where DOS-era ZIPs legitimately use `\\` separators.
        const reason = invalidPath(sourcePath);
        if (reason) { issues.push({ line, reason }); continue; }
        const key = locatorKey({ sourcePath, containerMember, memberPath });
        if (!keepDuplicates && keys.has(key)) { issues.push({ line, reason: 'duplicate locator' }); continue; }
        keys.add(key);
        const { name, folder, extension } = rowPaths(sourcePath, containerMember, memberPath);
        const format = (get('format_detected').trim() || get('format_by_extension').trim() || extension).toLowerCase();
        const number = column => get(column).trim() && Number.isFinite(Number(get(column))) ? Number(get(column)) : null;
        rows.push({ id: rows.length, sourcePath, sourceUrl: get('source_url').trim(), containerMember, memberPath, rawHex: get('member_path_raw_hex'), name, folder,
            title: get('title'), format, playable: playableFormats.has(format), channels: number('channels'), size: number('size'),
            timestamp: get('timestamp'), tracker: get('tracker'), sha256: validSha256(get('sha256')),
            instrumentNames: get('instrument_names'), sampleNames: get('sample_names'), songMessage: get('song_message'), line, ...(keepDuplicates ? { library: get('library') } : {}) });
    }
    return { rows, issues, columns };
}

export function foldText(text) {
    return text.toLowerCase().normalize('NFKD').replace(/\p{M}/gu, '');
}

function compactText(text) { return text.replace(/[^\p{L}\p{N}]/gu, ''); }
function validSha256(text) { const value = text.trim().toLowerCase(); return /^[a-f0-9]{64}$/.test(value) ? value : ''; }
const commentColumns = ['instrument_names', 'sample_names', 'song_message'];
const commentKeys = ['instrumentNames', 'sampleNames', 'songMessage'];

export function isCommentsCsvHeader(firstLine) {
    try {
        const columns = (parseCsv(firstLine)[0] || []).map(column => column.trim().toLowerCase());
        return columns.includes('sha256') && commentColumns.some(column => columns.includes(column)) && !columns.includes('source_path');
    } catch { return false; }
}

export function parseCommentsCsv(text) {
    const records = parseCsv(text);
    const positions = new Map();
    (records[0] || []).forEach((column, index) => {
        const name = column.trim().toLowerCase();
        if (positions.has(name)) throw new Error(`Duplicate CSV header: ${name}`);
        positions.set(name, index);
    });
    const missing = [];
    if (!positions.has('sha256')) missing.push('sha256');
    if (!commentColumns.some(column => positions.has(column))) missing.push('instrument_names, sample_names or song_message');
    if (missing.length) throw new Error(`Missing CSV header: ${missing.join('; ')}`);
    const comments = new Map(), issues = [];
    for (let index = 1; index < records.length; index += 1) {
        const fields = records[index];
        if (fields.length === 1 && fields[0] === '') continue;
        const sha256 = validSha256(fields[positions.get('sha256')] || '');
        const reason = !sha256 ? 'invalid sha256' : comments.has(sha256) ? 'duplicate sha256' : '';
        if (reason) { issues.push({ line: records.lines[index], reason }); continue; }
        comments.set(sha256, Object.fromEntries(commentKeys.map((key, column) => [key, fields[positions.get(commentColumns[column])] || ''])));
    }
    return { comments, issues };
}

function prepareRow(library, row) {
    const haystack = foldText([row.name, row.title, row.folder, row.format, row.tracker].join('\n'));
    library.haystacks[row.id] = haystack;
    library.compactHaystacks[row.id] = compactText(haystack);
    library.fieldHaystacks[row.id] = Object.fromEntries(['name', 'title', 'tracker', 'path'].map(field =>
        [field, foldText(field === 'path' ? [row.sourcePath, row.containerMember, row.memberPath].filter(Boolean).join('/') : row[field])]));
    library.sortKeys[row.id] = Object.fromEntries(['name', 'title', 'format', 'timestamp', 'folder'].map(column => [column, row[column].toLowerCase()]));
}

function countFormat(library, format, difference) {
    const counts = modelStates.get(library).formatCounts;
    const count = (counts.get(format) || 0) + difference;
    if (count) { counts.set(format, count); library.formats.add(format); }
    else { counts.delete(format); library.formats.delete(format); }
}

function countFolders(library, row) {
    let path = '';
    if (row.playable) library.folderCounts.set('', library.folderCounts.get('') + 1);
    for (const label of row.folder ? row.folder.split('/') : []) {
        path = path ? `${path}/${label}` : label;
        library.folderCounts.set(path, (library.folderCounts.get(path) || 0) + (row.playable ? 1 : 0));
    }
}

function prepareComments(library, row) {
    const state = modelStates.get(library);
    const previous = state.commentStates[row.id];
    if (previous) {
        if (!previous.sha256) state.unknownHashes -= 1;
        if (previous.hasText) state.commentRows -= 1;
        if (previous.joined) {
            state.joined -= 1;
            const count = state.usedHashes.get(previous.sha256) - 1;
            if (count) state.usedHashes.set(previous.sha256, count);
            else state.usedHashes.delete(previous.sha256);
        }
    }
    const sidecar = library.comments?.get(row.sha256);
    let stored = sidecar && state.shared.get(sidecar);
    if (!stored) {
        const original = Object.fromEntries(commentKeys.map(key => [key, typeof (sidecar || row)[key] === 'string' ? (sidecar || row)[key] : '']));
        const haystack = foldText(commentKeys.map(key => original[key]).filter(Boolean).join('\n'));
        stored = { original, haystack, compact: compactText(haystack) };
        if (sidecar) state.shared.set(sidecar, stored);
    }
    const changed = !state.display[row.id] || commentKeys.some(key => state.display[row.id][key] !== stored.original[key]);
    state.display[row.id] = stored.original;
    library.commentHaystacks[row.id] = stored.haystack;
    library.compactCommentHaystacks[row.id] = stored.compact;
    const hasText = commentKeys.some(key => stored.original[key]);
    state.commentStates[row.id] = { sha256: row.sha256, joined: Boolean(sidecar), hasText };
    if (!row.sha256) state.unknownHashes += 1;
    if (hasText) state.commentRows += 1;
    if (sidecar) {
        state.joined += 1;
        state.usedHashes.set(row.sha256, (state.usedHashes.get(row.sha256) || 0) + 1);
    }
    library.hasComments = state.commentRows > 0;
    library.commentCounts.joined = state.joined;
    library.commentCounts.unmatched = (library.comments?.size || 0) - state.usedHashes.size;
    library.commentCounts.awaitingHashes = library.comments ? state.unknownHashes : 0;
    return changed;
}

function initializeLibrary(library, name) {
    const state = { display: [], commentStates: [], shared: new WeakMap(), usedHashes: new Map(), formatCounts: new Map(), unknownHashes: 0, commentRows: 0, joined: 0 };
    modelStates.set(library, state);
    Object.assign(library, { identityName: name, revision: 0, locatorIds: new Map(), haystacks: [], compactHaystacks: [], fieldHaystacks: [], sortKeys: [],
        commentHaystacks: [], compactCommentHaystacks: [], hasComments: false, comments: null, commentCounts: { joined: 0, unmatched: 0, awaitingHashes: 0 }, commentChangedRowIds: [],
        formats: new Set(), folderCounts: new Map([['', 0]]), commentsFor: rowId => state.display[rowId] });
    for (const row of library.rows) {
        library.locatorIds.set(locatorKey(row), row.id);
        prepareRow(library, row);
        prepareComments(library, row);
        countFormat(library, row.format, 1);
        countFolders(library, row);
    }
    return library;
}

// Keep the original return shape for indexed-library callers. Discovered consumers can
// read live commentCounts (including awaitingHashes) and commentChangedRowIds for a batch
// notification. Sidecar replacement leaves intrinsic row text intact for later fallback.
export function attachComments(library, comments) {
    const replaced = library.comments !== comments;
    library.comments = comments;
    modelStates.get(library).shared = new WeakMap();
    const changed = [];
    for (const row of library.rows) if (prepareComments(library, row)) changed.push(row.id);
    library.commentCounts.unmatched = (comments?.size || 0) - modelStates.get(library).usedHashes.size;
    library.commentCounts.awaitingHashes = comments ? modelStates.get(library).unknownHashes : 0;
    library.commentChangedRowIds = changed;
    if (replaced || changed.length) library.revision += 1;
    return { joined: library.commentCounts.joined, unmatched: library.commentCounts.unmatched };
}

export function parseLibraryCsv(text, { name = '' } = {}) {
    return initializeLibrary(readRows(text, false), name);
}

// Discovered rows keep the CSV shape (line is null, sourceUrl is empty), with no
// guessed title/channels. Each changed append/update batch advances revision once;
// no-op batches and duplicate locators do not. IDs remain array positions forever.
export function createDiscoveredLibrary({ name = '' } = {}) {
    return initializeLibrary({ rows: [], issues: [], columns: [] }, typeof name === 'string' ? name : '');
}

const metadataTextKeys = ['title', 'tracker', 'instrumentNames', 'sampleNames', 'songMessage', 'error'];
const metadataStates = new Set(['unknown', 'ready', 'error']);
const textValue = value => typeof value === 'string' ? value : '';
const channelValue = value => Number.isSafeInteger(value) && value > 0 ? value : null;
const sizeValue = value => Number.isSafeInteger(value) && value >= 0 ? value : null;

export function appendDiscoveredRows(library, records) {
    const changed = [];
    for (const record of records) {
        const sourcePath = textValue(record?.sourcePath);
        const containerMember = record?.containerMember ?? '', memberPath = record?.memberPath ?? '';
        const reason = invalidPath(sourcePath) || (typeof containerMember !== 'string' || typeof memberPath !== 'string' ? 'invalid archive member path' : '');
        if (reason) { library.issues.push({ path: sourcePath, reason }); continue; }
        const key = locatorKey({ sourcePath, containerMember, memberPath });
        if (library.locatorIds.has(key)) continue;
        const { name, folder, extension } = rowPaths(sourcePath, containerMember, memberPath);
        const format = textValue(record.format).trim().toLowerCase() || extension;
        const row = { id: library.rows.length, sourcePath, sourceUrl: '', containerMember, memberPath, rawHex: textValue(record.rawHex), name, folder,
            format, playable: record.playable === true && playableFormats.has(format), channels: channelValue(record.channels), size: sizeValue(record.size),
            timestamp: textValue(record.timestamp), sha256: validSha256(textValue(record.sha256)), line: null,
            metadataState: metadataStates.has(record.metadataState) ? record.metadataState : 'unknown',
            ...Object.fromEntries(metadataTextKeys.map(field => [field, textValue(record[field])])) };
        library.rows.push(row);
        library.locatorIds.set(key, row.id);
        prepareRow(library, row);
        prepareComments(library, row);
        countFormat(library, format, 1);
        countFolders(library, row);
        changed.push(row.id);
    }
    if (changed.length) library.revision += 1;
    return changed;
}

export function updateDiscoveredRows(library, updates) {
    const changed = new Set();
    for (const { locator, patch } of updates) {
        if (!locator || !patch || typeof patch !== 'object') continue;
        const id = library.locatorIds.get(locatorKey(locator));
        if (id === undefined) continue; // The discovery controller owns updates arriving before rows.
        const row = library.rows[id], normalized = {};
        for (const field of metadataTextKeys) if (Object.hasOwn(patch, field)) normalized[field] = textValue(patch[field]);
        if (Object.hasOwn(patch, 'format')) normalized.format = textValue(patch.format).trim().toLowerCase();
        if (Object.hasOwn(patch, 'channels')) normalized.channels = channelValue(patch.channels);
        if (Object.hasOwn(patch, 'sha256')) normalized.sha256 = validSha256(textValue(patch.sha256));
        if (metadataStates.has(patch.metadataState)) normalized.metadataState = patch.metadataState;
        // A successful retry clears an inspection error; structural extraction failures
        // remain attached to unplayable entries and are never repaired by metadata.
        if (normalized.metadataState === 'ready' && row.playable) normalized.error = '';
        if (!row.playable) delete normalized.error;
        if (!Object.keys(normalized).some(field => row[field] !== normalized[field])) continue;
        if (normalized.format !== undefined && normalized.format !== row.format) {
            countFormat(library, row.format, -1);
            countFormat(library, normalized.format, 1);
        }
        Object.assign(row, normalized);
        prepareRow(library, row);
        prepareComments(library, row);
        changed.add(id);
    }
    if (changed.size) library.revision += 1;
    return [...changed];
}

export async function libraryIdentity(bytes, name) {
    const input = bytes instanceof ArrayBuffer ? new Uint8Array(bytes) : new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    const encodedName = new TextEncoder().encode(name);
    const combined = new Uint8Array(input.length + encodedName.length);
    combined.set(input); combined.set(encodedName, input.length);
    const digest = await globalThis.crypto.subtle.digest('SHA-256', combined);
    return { sha256: Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, '0')).join(''), name };
}

export function compileQuery(text) {
    const tokens = [];
    // Quotes can follow a field prefix: title:"a phrase". A leading `-` outside quotes
    // excludes rows the rest of the token matches; `"-=FC=-"` stays a literal search.
    for (const match of text.matchAll(/(?:[^\s"]|"[^"]*")+/g)) {
        const negated = match[0].length > 1 && match[0].startsWith('-');
        const literal = (negated ? match[0].slice(1) : match[0]).replaceAll('"', '');
        const token = foldText(literal);
        const colon = token.indexOf(':');
        const field = colon >= 0 ? token.slice(0, colon) : '';
        const compiled = ['format', 'title', 'name', 'path', 'tracker', 'ch', 'text'].includes(field) ? { field, value: field === 'format' ? literal.slice(colon + 1).toLowerCase() : token.slice(colon + 1), literal: literal.slice(colon + 1), compact: compactText(token.slice(colon + 1)) } : { field: '', value: token, literal, compact: compactText(token) };
        // An empty exclusion (`-text:`) would match, and so hide, every row.
        if (negated && !compiled.value) continue;
        tokens.push({ ...compiled, negated });
    }
    return tokens;
}

// A folded hit implies a compact hit (stripping the same characters from both keeps a
// substring contiguous), so tokens long enough to compact-match need only one scan.
function contains(haystack, compactHaystack, { value, compact }) {
    return compact.length >= 4 ? compactHaystack.includes(compact) : haystack.includes(value);
}

function tokenMatches(library, row, token, includeComments) {
    const { field, value } = token;
    if (field === 'text') return contains(library.commentHaystacks[row.id], library.compactCommentHaystacks[row.id], token);
    if (!field) return contains(library.haystacks[row.id], library.compactHaystacks[row.id], token) || includeComments && contains(library.commentHaystacks[row.id], library.compactCommentHaystacks[row.id], token);
    if (field === 'format') return row.format.includes(value);
    if (field === 'ch') {
        const comparison = /^(<=|>=|<|>|=)?(\d+)$/.exec(value);
        if (!comparison || row.channels === null) return false;
        const expected = Number(comparison[2]);
        switch (comparison[1]) {
            case '<': return row.channels < expected;
            case '>': return row.channels > expected;
            case '<=': return row.channels <= expected;
            case '>=': return row.channels >= expected;
            default: return row.channels === expected;
        }
    }
    return library.fieldHaystacks[row.id][field].includes(value);
}

function compareLocators(left, right) {
    for (const field of ['sourcePath', 'containerMember', 'memberPath']) {
        if (left[field] !== right[field]) return left[field] < right[field] ? -1 : 1;
    }
    return 0;
}

export function search(library, query = '', { scopeFolder = '', showUnplayable = false, format = '', includeComments = true, sort } = {}) {
    // Hand-built tokens may omit `compact`; compute it once, not per row.
    const tokens = (typeof query === 'string' ? compileQuery(query) : query).map(token => token.compact === undefined ? { ...token, compact: compactText(token.value) } : token);
    const matches = [];
    const scope = scopeFolder.replace(/\/$/, '');
    for (const row of library.rows) {
        if (!showUnplayable && !row.playable || format && row.format !== format.toLowerCase()) continue;
        if (scope && row.folder !== scope && !row.folder.startsWith(`${scope}/`)) continue;
        if (!tokens.every(token => tokenMatches(library, row, token, includeComments) !== (token.negated === true))) continue;
        matches.push(row.id);
    }
    if (sort) {
        const compare = (left, right) => left === right ? 0 : left === null ? -1 : right === null ? 1 : left < right ? -1 : 1;
        const column = sort.column;
        if (!['name', 'title', 'format', 'channels', 'size', 'timestamp', 'folder'].includes(column)) throw new Error(`Unknown sort column: ${column}`);
        matches.sort((left, right) => {
            const leftKeys = library.sortKeys[left], rightKeys = library.sortKeys[right];
            const primary = compare(leftKeys[column] ?? library.rows[left][column], rightKeys[column] ?? library.rows[right][column]);
            return primary * (sort.descending ? -1 : 1) || compare(leftKeys.folder, rightKeys.folder) || compare(leftKeys.name, rightKeys.name) || compareLocators(library.rows[left], library.rows[right]);
        });
    }
    return Int32Array.from(matches);
}

// Tokens that can match comment text: `text:` always, free text unless comments are excluded.
export function commentTokens(tokens, { includeComments = true } = {}) {
    return tokens.filter(token => (token.field === 'text' || !token.field && includeComments) && token.value && !token.negated);
}

function lineMatches(text, tokens) {
    const folded = foldText(text);
    const compact = tokens.some(token => token.compact.length >= 4) ? compactText(folded) : '';
    return tokens.some(token => folded.includes(token.value) || token.compact.length >= 4 && compact.includes(token.compact));
}

// Where tokens match, as merged [start, end) ranges of `text`, under the same folded and
// compact rules as `search`, so a highlight never disagrees with why a row matched. Each
// code point is folded on its own and remembers the source span it came from.
export function matchRanges(text, tokens) {
    let folded = '', compact = '';
    const foldedSpans = [], compactSpans = [];
    for (let start = 0; start < text.length;) {
        const end = start + String.fromCodePoint(text.codePointAt(start)).length;
        for (const point of foldText(text.slice(start, end))) {
            folded += point;
            for (let unit = 0; unit < point.length; unit += 1) foldedSpans.push([start, end]);
            if (/[\p{L}\p{N}]/u.test(point)) {
                compact += point;
                for (let unit = 0; unit < point.length; unit += 1) compactSpans.push([start, end]);
            }
        }
        start = end;
    }
    const ranges = [];
    const collect = (haystack, spans, needle) => {
        for (let at = haystack.indexOf(needle); at >= 0; at = haystack.indexOf(needle, at + 1)) ranges.push([spans[at][0], spans[at + needle.length - 1][1]]);
    };
    for (const token of tokens) {
        collect(folded, foldedSpans, token.value);
        if (token.compact.length >= 4) collect(compact, compactSpans, token.compact);
    }
    ranges.sort((left, right) => left[0] - right[0] || right[1] - left[1]);
    const merged = [];
    for (const range of ranges) {
        if (merged.length && range[0] <= merged.at(-1)[1]) merged.at(-1)[1] = Math.max(merged.at(-1)[1], range[1]);
        else merged.push([...range]);
    }
    return merged;
}

// The comment lines a row matched on, for snippets: `{ section, first, last, text, ranges }`,
// with 0-based line numbers. A compact match can straddle two slots ("Purple" / "Motion"),
// so a section with no single matching line falls back to adjacent pairs.
export function commentMatchLines(comments, tokens) {
    const hits = [];
    if (!comments || !tokens.length) return hits;
    for (const section of ['instrumentNames', 'sampleNames', 'songMessage']) {
        if (!comments[section]) continue;
        const lines = comments[section].split('\n');
        const found = [];
        lines.forEach((line, index) => { if (line && lineMatches(line, tokens)) found.push({ section, first: index, last: index, text: line }); });
        if (!found.length) for (let index = 0; index + 1 < lines.length; index += 1) {
            const text = `${lines[index]}\n${lines[index + 1]}`;
            if (lineMatches(text, tokens)) found.push({ section, first: index, last: index + 1, text });
        }
        for (const hit of found) hits.push({ ...hit, ranges: matchRanges(hit.text, tokens) });
    }
    return hits;
}

export function buildTree(library) {
    const root = { path: '', label: '', kind: 'dir', children: [], trackCount: 0 };
    const nodes = new Map([['', root]]);
    for (const row of library.rows) {
        let parent = root;
        const zipPath = row.memberPath || row.containerMember ? row.sourcePath : null;
        const nestedPath = row.containerMember ? `${row.sourcePath}/${row.containerMember.replaceAll('\\', '/')}` : null;
        let path = '';
        if (row.playable) root.trackCount += 1;
        for (const label of row.folder ? row.folder.split('/') : []) {
            path = path ? `${path}/${label}` : label;
            let node = nodes.get(path);
            const kind = path === nestedPath ? 'nested' : path === zipPath ? 'zip' : 'dir';
            if (!node) { node = { path, label, kind, children: [], trackCount: 0 }; nodes.set(path, node); parent.children.push(node); }
            else if (kind !== 'dir') node.kind = kind;
            if (row.playable) node.trackCount += 1;
            parent = node;
        }
    }
    return root;
}

export function parsePlaylistCsv(text) {
    const { rows, issues } = readRows(text, true);
    return { entries: rows.map(({ sourcePath, containerMember, memberPath, rawHex, title, format, library }) => ({ sourcePath, containerMember, memberPath, rawHex, title, format, library })), issues };
}

export function toPlaylistCsv(entries) {
    const quote = value => {
        const text = String(value ?? '');
        return /[",\r\n]/.test(text) ? `"${text.replaceAll('"', '""')}"` : text;
    };
    return 'source_path,container_member,member_path,member_path_raw_hex,title,format_detected,library\r\n' + entries.map(entry =>
        ['sourcePath', 'containerMember', 'memberPath', 'rawHex', 'title', 'format', 'library'].map(key => quote(entry[key])).join(',') + '\r\n').join('');
}
