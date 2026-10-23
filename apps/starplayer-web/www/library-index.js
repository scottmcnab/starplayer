// Pure library/playlist data layer. No filesystem, DOM, or playback ownership.
const knownColumns = new Set(('source_path container_member member_path entry_kind extension format_by_extension size compressed_size crc32 timestamp compression_method encrypted member_path_raw_hex source_url status error sha256 crc_verified format_detected format_variant channels title tracker instrument_slots instruments samples sample_bytes extended_status extended_error library instrument_names sample_names song_message').split(' '));
const playableFormats = new Set(['mod', 's3m', 'mtm', 'xm', 'it']);

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
        const key = JSON.stringify([sourcePath, containerMember, memberPath]);
        if (!keepDuplicates && keys.has(key)) { issues.push({ line, reason: 'duplicate locator' }); continue; }
        keys.add(key);
        const innermost = memberPath || containerMember || sourcePath;
        const name = innermost.split(/[\\/]/).at(-1);
        const fullPath = [sourcePath, containerMember, memberPath].filter(Boolean).join('/').replaceAll('\\', '/');
        const folder = fullPath.slice(0, fullPath.length - name.length).replace(/\/$/, '');
        const extension = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1).toLowerCase() : '';
        const format = (get('format_detected').trim() || get('format_by_extension').trim() || extension).toLowerCase();
        const number = column => get(column).trim() && Number.isFinite(Number(get(column))) ? Number(get(column)) : null;
        rows.push({ id: rows.length, sourcePath, containerMember, memberPath, rawHex: get('member_path_raw_hex'), name, folder,
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

export function attachComments(library, comments) {
    const shared = new Map();
    const used = new Set();
    const display = [];
    library.commentHaystacks = [];
    library.compactCommentHaystacks = [];
    let joined = 0;
    library.hasComments = false;
    for (const row of library.rows) {
        const sidecar = comments?.get(row.sha256);
        if (sidecar) { joined += 1; used.add(row.sha256); }
        const text = sidecar || row;
        // Sidecar records share both folded strings and their original display object.
        let stored = shared.get(text);
        if (!stored) {
            const original = Object.fromEntries(commentKeys.map(key => [key, text[key] || '']));
            const haystack = foldText(commentKeys.map(key => original[key]).filter(Boolean).join('\n'));
            stored = { original, haystack, compact: compactText(haystack) };
            shared.set(text, stored);
        }
        display[row.id] = stored.original;
        library.commentHaystacks[row.id] = stored.haystack;
        library.compactCommentHaystacks[row.id] = stored.compact;
        if (commentKeys.some(key => stored.original[key])) library.hasComments = true;
    }
    library.commentsFor = rowId => display[rowId];
    return { joined, unmatched: (comments?.size || 0) - used.size };
}

export function parseLibraryCsv(text, { name = '' } = {}) {
    const library = readRows(text, false);
    library.identityName = name;
    library.haystacks = library.rows.map(row => [row.name, row.title, row.folder, row.format, row.tracker].join('\n')).map(foldText);
    library.compactHaystacks = library.haystacks.map(compactText);
    library.fieldHaystacks = library.rows.map(row => Object.fromEntries(['name', 'title', 'tracker', 'path'].map(field => [field, foldText(field === 'path' ? [row.sourcePath, row.containerMember, row.memberPath].filter(Boolean).join('/') : row[field])])));
    attachComments(library, null);
    library.sortKeys = library.rows.map(row => Object.fromEntries(['name', 'title', 'format', 'timestamp', 'folder'].map(column => [column, row[column].toLowerCase()])));
    return library;
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
    // Quotes can follow a field prefix: title:"a phrase".
    for (const match of text.matchAll(/(?:[^\s"]|"[^"]*")+/g)) {
        const literal = match[0].replaceAll('"', '');
        const token = foldText(literal);
        const colon = token.indexOf(':');
        const field = colon >= 0 ? token.slice(0, colon) : '';
        tokens.push(['format', 'title', 'name', 'path', 'tracker', 'ch', 'text'].includes(field) ? { field, value: field === 'format' ? literal.slice(colon + 1).toLowerCase() : token.slice(colon + 1), literal: literal.slice(colon + 1), compact: compactText(token.slice(colon + 1)) } : { field: '', value: token, literal, compact: compactText(token) });
    }
    return tokens;
}

export function search(library, query = '', { scopeFolder = '', showUnplayable = false, format = '', includeComments = true, sort } = {}) {
    const tokens = typeof query === 'string' ? compileQuery(query) : query;
    const matches = [];
    const scope = scopeFolder.replace(/\/$/, '');
    for (const row of library.rows) {
        if (!showUnplayable && !row.playable || format && row.format !== format.toLowerCase()) continue;
        if (scope && row.folder !== scope && !row.folder.startsWith(`${scope}/`)) continue;
        if (!tokens.every(({ field, value, compact = compactText(value) }) => {
            const contains = (haystack, compactHaystack) => haystack.includes(value) || compact.length >= 4 && compactHaystack.includes(compact);
            if (field === 'text') return contains(library.commentHaystacks[row.id], library.compactCommentHaystacks[row.id]);
            if (!field) return contains(library.haystacks[row.id], library.compactHaystacks[row.id]) || includeComments && contains(library.commentHaystacks[row.id], library.compactCommentHaystacks[row.id]);
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
        })) continue;
        matches.push(row.id);
    }
    if (sort) {
        const compare = (left, right) => left === right ? 0 : left === null ? -1 : right === null ? 1 : left < right ? -1 : 1;
        const column = sort.column;
        if (!['name', 'title', 'format', 'channels', 'size', 'timestamp', 'folder'].includes(column)) throw new Error(`Unknown sort column: ${column}`);
        matches.sort((left, right) => {
            const leftKeys = library.sortKeys[left], rightKeys = library.sortKeys[right];
            const primary = compare(leftKeys[column] ?? library.rows[left][column], rightKeys[column] ?? library.rows[right][column]);
            return primary * (sort.descending ? -1 : 1) || compare(leftKeys.folder, rightKeys.folder) || compare(leftKeys.name, rightKeys.name) || left - right;
        });
    }
    return Int32Array.from(matches);
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
