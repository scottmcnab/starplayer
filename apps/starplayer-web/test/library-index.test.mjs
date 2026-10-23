import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, access } from 'node:fs/promises';
import { performance } from 'node:perf_hooks';
import { parseCsv, parseLibraryCsv, parseCommentsCsv, isCommentsCsvHeader, attachComments, foldText, compileQuery, commentTokens, matchRanges, commentMatchLines, search, buildTree, libraryIdentity, parsePlaylistCsv, toPlaylistCsv } from '../www/library-index.js';

const hornetHeader = 'source_path,container_member,member_path,entry_kind,extension,format_by_extension,size,compressed_size,crc32,timestamp,compression_method,encrypted,member_path_raw_hex,source_url,status,error,sha256,crc_verified,format_detected,format_variant,channels,title,tracker,instrument_slots,instruments,samples,sample_bytes,extended_status,extended_error';
const quote = value => /[",\r\n]/.test(String(value)) ? `"${String(value).replaceAll('"', '""')}"` : String(value);
function csv(records, header = hornetHeader) {
    const columns = header.split(',');
    return header + '\r\n' + records.map(record => columns.map(column => quote(record[column] ?? '')).join(',')).join('\r\n');
}
const synthetic = [
    { source_path: 'loose/alpha.mod', title: 'Blue "Sky",\nPart Two', channels: 4, size: 50, tracker: 'ProTracker', timestamp: '1995', status: 'ok' },
    { source_path: 'packs/songs.zip', member_path: 'beta.xm', title: 'Blue Sky', channels: 12, size: 100, tracker: 'FastTracker', timestamp: '1996' },
    { source_path: 'packs/songs.zip', container_member: 'set/inner.zip', member_path: 'music/gamma.it', title: 'Green Moon', channels: 8, size: 75 },
    { source_path: 'packs/songs.zip', member_path: 'notes.txt', title: 'Notes' },
];
const library = () => parseLibraryCsv(csv(synthetic), { name: 'Synthetic' });
const ids = (query, options) => [...search(library(), query, options)];

test('RFC 4180 fields, multiline lines, BOM, CRLF and final newline', () => {
    assert.deepEqual(parseCsv('\uFEFFa,b\r\n"one,""two""\r\nthree",last\r\n'), [['a', 'b'], ['one,"two"\r\nthree', 'last']]);
    assert.deepEqual(parseCsv('a,b\nx,'), [['a', 'b'], ['x', '']]);
    assert.deepEqual(parseCsv('""'), [['']]);
    assert.deepEqual(parseCsv(''), []);
    for (const input of ['a\n"unterminated', 'a\n"x"oops', 'a\nx"y']) assert.throws(() => parseCsv(input), /CSV/);
    const parsed = parseLibraryCsv(csv(synthetic));
    assert.equal(parsed.rows[0].title, 'Blue "Sky",\nPart Two');
    assert.equal(parsed.rows[1].line, 4);
});

test('Hornet header, shuffled and minimal headers, normalised records', () => {
    const parsed = library();
    assert.equal(parsed.rows.length, 4);
    assert.deepEqual(parsed.issues, []);
    assert.equal(parsed.identityName, 'Synthetic');
    assert.equal(parsed.rows[2].folder, 'packs/songs.zip/set/inner.zip/music');
    assert.equal(parsed.rows[2].name, 'gamma.it');
    assert.equal(parsed.rows[3].playable, false);
    assert.equal(parsed.rows[3].channels, null);
    assert.deepEqual(parseLibraryCsv('member_path,source_path\na.mod,p.zip').rows.map(row => row.name), ['a.mod']);
    const shuffled = parseLibraryCsv(' TITLE ,SOURCE_PATH,unknown,CHANNELS\nSong,a.mod,ignored,4');
    assert.equal(shuffled.rows[0].title, 'Song');
    assert.equal(shuffled.rows[0].channels, 4);
    assert.throws(() => parseLibraryCsv('title\nx'), /source_path/);
    assert.throws(() => parseLibraryCsv('source_path, TITLE,title\na.mod,x,y'), /title/);
    assert.throws(() => parsePlaylistCsv('source_path,SOURCE_PATH\na,b'), /source_path/);
});

test('source_path rejects absolute, drive, dot, traversal and backslash paths; member names are archive names', () => {
    for (const column of ['source_path']) {
        for (const value of ['/absolute.mod', 'C:song.mod', 'c:/song.mod', './song.mod', 'x/../song.mod', 'x/./song.mod', '..', '.', '\\server\\song.mod']) {
            const result = parseLibraryCsv(csv([{ source_path: 'a.zip', [column]: value }]));
            assert.equal(result.rows.length, 0, `${column}: ${value}`);
            assert.equal(result.issues[0].line, 2);
        }
    }
    assert.equal(parseLibraryCsv('source_path\na.mod\n\nb.mod\n').rows.length, 2, 'blank lines are skipped');
    for (const column of ['container_member', 'member_path']) assert.equal(parseLibraryCsv(csv([{ source_path: 'a.zip', [column]: 'DISK\\SONG.MOD' }])).rows.length, 1, column);
    assert.match(parseLibraryCsv('source_path\n   ').issues[0].reason, /blank/);
});

test('first duplicate wins; row filtering and format fallback', () => {
    const result = parseLibraryCsv(csv([
        { source_path: 'a.mod', title: 'first' }, { source_path: 'a.mod', title: 'second' },
        { source_path: 'b.mod', entry_kind: 'directory' }, { source_path: 'c.mod', status: 'corrupt' },
        { source_path: 'd.mod', extended_status: 'unsupported_method' },
        { source_path: 'e.mod', format_detected: 'XM', format_by_extension: 'it' },
        { source_path: 'f.mod', format_by_extension: 's3m', status: ' ', extended_status: 'ok' },
        { source_path: 'g.MTM', extension: 'xm' }, { source_path: 'h', format_detected: '669' },
    ]));
    assert.equal(result.issues.length, 1);
    assert.equal(result.rows[0].title, 'first');
    assert.deepEqual(result.rows.map(row => row.format), ['mod', 'xm', 's3m', 'mtm', '669']);
    assert.deepEqual(result.rows.map(row => row.id), [0, 1, 2, 3, 4]);
});

test('tokens, phrases, field prefixes, channel comparisons, scopes and formats', () => {
    assert.deepEqual(ids('BLUE sky'), [0, 1]);
    assert.deepEqual(ids('"blue sky"'), [0, 1], 'compact matching crosses punctuation');
    assert.deepEqual(ids('title:"blue sky"'), [1], 'field prefixes retain substring matching');
    assert.deepEqual(ids('title:"green moon"'), [2]);
    assert.deepEqual(ids('format:XM name:beta tracker:FAST path:songs.zip'), [1]);
    assert.deepEqual(ids('ch:8'), [2]);
    assert.deepEqual(ids('ch:>8'), [1]);
    assert.deepEqual(ids('ch:<=4'), [0]);
    assert.deepEqual(ids('ch:>=8'), [1, 2]);
    assert.deepEqual(ids('ch:<8'), [0]);
    assert.deepEqual(ids('ch:bad'), []);
    assert.deepEqual(ids('', { scopeFolder: 'packs/songs.zip' }), [1, 2]);
    assert.deepEqual(ids('', { scopeFolder: 'pack' }), []);
    assert.deepEqual(ids('', { showUnplayable: true, format: 'TXT' }), [3]);
    assert.deepEqual([...search(library(), compileQuery('name:alpha'))], [0]);
});

test('precomputed stable sorts with folder/name ties, numeric and timestamp columns', () => {
    assert.deepEqual(ids('', { sort: { column: 'size', descending: true } }), [1, 2, 0]);
    assert.deepEqual(ids('', { sort: { column: 'channels' } }), [0, 2, 1]);
    assert.deepEqual(ids('', { sort: { column: 'timestamp' } }), [2, 0, 1]);
    for (const column of ['name', 'title', 'format', 'folder']) assert.equal(ids('', { sort: { column } }).length, 3);
    const tied = parseLibraryCsv(csv([
        { source_path: 'b/z.mod', title: 'same' }, { source_path: 'a/z.mod', title: 'Same' },
        { source_path: 'a/b.mod', title: 'same' }, { source_path: 'a/B.mod', title: 'same' },
    ]));
    assert.deepEqual([...search(tied, '', { sort: { column: 'title' } })], [2, 3, 1, 0]);
});

test('tree models ZIP and nested ZIP levels and counts playable descendants', () => {
    const tree = buildTree(library());
    assert.equal(tree.trackCount, 3);
    const packs = tree.children.find(node => node.label === 'packs');
    const outer = packs.children[0];
    assert.equal(outer.kind, 'zip');
    assert.equal(outer.trackCount, 2);
    const nested = outer.children[0].children[0];
    assert.equal(nested.kind, 'nested');
    assert.equal(nested.path, 'packs/songs.zip/set/inner.zip');
    assert.equal(nested.trackCount, 1);
});

test('A–B–A playlist round trip preserves raw names, identities, order and duplicates', () => {
    const first = { sourcePath: 'a.zip', containerMember: '', memberPath: '├⌐.mod', rawHex: 'c3a92e6d6f64', title: 'A, "Song"\nAgain', format: 'mod', library: 'Collection' };
    const second = { sourcePath: 'b.it', containerMember: '', memberPath: '', rawHex: '', title: 'B', format: 'it', library: '' };
    const entries = [first, second, first];
    assert.deepEqual(parsePlaylistCsv(toPlaylistCsv(entries)), { entries, issues: [] });
    assert.equal(parsePlaylistCsv('source_path\na.mod\na.mod').entries.length, 2);
});

test('identity hashes exact CSV bytes followed by UTF-8 name', async () => {
    const bytes = new TextEncoder().encode('source_path\na.mod');
    const identity = await libraryIdentity(bytes, 'Synthetic');
    assert.match(identity.sha256, /^[a-f0-9]{64}$/);
    assert.deepEqual(identity, await libraryIdentity(bytes.buffer, 'Synthetic'));
    assert.notEqual(identity.sha256, (await libraryIdentity(bytes, 'Other')).sha256);
    assert.notEqual(identity.sha256, (await libraryIdentity(new TextEncoder().encode('source_path\nb.mod'), 'Synthetic')).sha256);
});

test('37,674 row scale budgets', () => {
    let state = 17;
    const random = () => { state = (Math.imul(state, 1664525) + 1013904223) >>> 0; return state; };
    const records = Array.from({ length: 37674 }, (_, index) => ({
        source_path: `music/${random() % 100}/pack${index % 200}.zip`, member_path: `track${index}.mod`,
        sha256: index.toString(16).padStart(64, '0'),
        title: `${random() % 100000} blue sky${index % 31 === 0 ? ', "Moon"\nAgain' : ''}`, channels: 4 + random() % 16,
    }));
    const text = csv(records);
    const commentsText = csv(records.map((record, index) => ({ sha256: record.sha256, instrument_names: `Purple Motion ${index}\n` + 'Synthetic sample greeting and ASCII art <===>. '.repeat(6) })), 'sha256,instrument_names');
    const heapBefore = process.memoryUsage().heapUsed;
    let started = performance.now();
    const parsed = parseLibraryCsv(text);
    const indexParseTime = performance.now() - started;
    attachComments(parsed, parseCommentsCsv(commentsText).comments);
    const built = buildTree(parsed);
    const combinedTime = performance.now() - started;
    const parseTime = indexParseTime;
    assert.equal(parsed.rows.length, 37674);
    started = performance.now();
    assert.equal(search(parsed, 'blue sky').length, 37674);
    const searchTime = performance.now() - started;
    // The first comment search after loading pays one-off JIT and first-touch costs
    // (about 40 ms); the budget is for a keystroke, so warm up with other terms first.
    started = performance.now();
    search(parsed, 'text:warmup');
    const coldCommentsTime = performance.now() - started;
    search(parsed, 'warmup');
    started = performance.now();
    assert.equal(search(parsed, 'text:purplemotion').length, 37674);
    const commentsSearchTime = performance.now() - started;
    started = performance.now();
    assert.equal(search(parsed, 'purplemotion').length, 37674);
    const plainCommentsTime = performance.now() - started;
    started = performance.now();
    search(parsed, '', { sort: { column: 'title' } });
    const sortTime = performance.now() - started;
    started = performance.now();
    assert.equal(buildTree(parsed).trackCount, 37674);
    const treeTime = performance.now() - started;
    console.log(`Comments budget: ${Buffer.byteLength(commentsText)} bytes, parse + attach + tree ${combinedTime.toFixed(1)} ms, first comment search ${coldCommentsTime.toFixed(1)} ms, plain ${plainCommentsTime.toFixed(1)} ms, text ${commentsSearchTime.toFixed(1)} ms, heap ${(process.memoryUsage().heapUsed / 1048576).toFixed(1)} MiB (delta ${((process.memoryUsage().heapUsed - heapBefore) / 1048576).toFixed(1)} MiB)`);
    assert.equal(built.trackCount, 37674);
    assert.ok(combinedTime < 1500, `combined ${combinedTime} ms`);
    assert.ok(plainCommentsTime < 60, `plain ${plainCommentsTime} ms`);
    assert.ok(commentsSearchTime < 60, `text ${commentsSearchTime} ms`);
    console.log(`Scale timings: parse ${parseTime.toFixed(1)} ms, search ${searchTime.toFixed(1)} ms, title sort ${sortTime.toFixed(1)} ms, tree ${treeTime.toFixed(1)} ms`);
    assert.ok(parseTime < 1000, `parse ${parseTime} ms`);
    assert.ok(searchTime < 50, `search ${searchTime} ms`);
    assert.ok(sortTime < 300, `sort ${sortTime} ms`);
    assert.ok(treeTime < 300, `tree ${treeTime} ms`);
});

test('local Hornet CSV compatibility (read only)', async context => {
    const path = '/home/scott/projects/moddna/index/hornet/hornet-modules.csv';
    try { await access(path); } catch { context.skip('Local Hornet CSV is absent'); return; }
    const started = performance.now();
    const parsed = parseLibraryCsv(await readFile(path, 'utf8'), { name: 'Hornet' });
    console.log(`Hornet CSV: ${parsed.rows.length} rows, ${parsed.issues.length} issues, ${(performance.now() - started).toFixed(1)} ms including read`);
    assert.equal(parsed.rows.length, 9585);
    assert.deepEqual(parsed.issues, []);
});


test('source paths reject empty segments and NUL; DOS member names retain locators and get folders', () => {
    const library = parseLibraryCsv('source_path,container_member,member_path\na//b.mod,,\na/, ,\na\0b.mod,,\npack.zip,bonus\\inner.zip,music\\song.s3m\n');
    assert.equal(library.issues.length, 3);
    assert.equal(library.rows[0].memberPath, 'music\\song.s3m');
    assert.equal(library.rows[0].name, 'song.s3m');
    assert.equal(library.rows[0].folder, 'pack.zip/bonus/inner.zip/music');
    const tree = buildTree(library);
    assert.equal(tree.children[0].children[0].children[0].kind, 'nested');
});

const firstSha = 'a'.repeat(64), secondSha = 'b'.repeat(64);
test('comments CSV headers, multiline fields, invalid and duplicate hashes and blank records', () => {
    const parsed = parseCommentsCsv(` SONG_MESSAGE ,SHA256,INSTRUMENT_NAMES,ignored\n"Hello\nWorld", ${firstSha.toUpperCase()} ,"One\n\nThree",x\n\nbad,nope,,\nsecond,${firstSha},ignored,`);
    assert.equal(parsed.comments.size, 1);
    assert.deepEqual(parsed.comments.get(firstSha), { instrumentNames: 'One\n\nThree', sampleNames: '', songMessage: 'Hello\nWorld' });
    assert.deepEqual(parsed.issues, [{ line: 7, reason: 'invalid sha256' }, { line: 8, reason: 'duplicate sha256' }]);
    assert.throws(() => parseCommentsCsv('instrument_names\nx'), /sha256/);
    assert.throws(() => parseCommentsCsv('sha256,format\nx,it'), /instrument_names.*sample_names.*song_message/);
    assert.throws(() => parseCommentsCsv('sha256,SHA256,song_message'), /Duplicate/);
    assert.throws(() => parseCommentsCsv('sha256,song_message,unknown,UNKNOWN'), /Duplicate/);
    assert.equal(isCommentsCsvHeader(hornetHeader), false);
    assert.equal(isCommentsCsvHeader('sha256,format,instrument_names,sample_names,song_message'), true);
    assert.equal(isCommentsCsvHeader('" SHA256 ","SONG_MESSAGE"'), true);
    assert.equal(isCommentsCsvHeader('sha256,song_message,source_path'), false);
    assert.equal(isCommentsCsvHeader('"unterminated'), false);
});

test('comment precedence, shared hashes, counts, replacement and detach retain inline text', () => {
    const parsed = parseLibraryCsv(csv([
        { source_path: 'a.mod', sha256: ` ${firstSha.toUpperCase()} `, instrument_names: 'Inline' },
        { source_path: 'b.mod', sha256: firstSha, instrument_names: 'Inline' },
        { source_path: 'c.mod', sha256: 'invalid', song_message: 'Standalone' },
    ], 'source_path,sha256,instrument_names,song_message'));
    assert.equal(parsed.rows[0].sha256, firstSha);
    assert.equal(parsed.rows[2].sha256, '');
    assert.equal(parsed.commentsFor(0).instrumentNames, 'Inline');
    const sidecar = parseCommentsCsv(`sha256,instrument_names\n${firstSha},Sidecar\n${secondSha},Unused`).comments;
    assert.deepEqual(attachComments(parsed, sidecar), { joined: 2, unmatched: 1 });
    assert.ok(parsed.commentHaystacks[0] === parsed.commentHaystacks[1]);
    assert.ok(parsed.compactCommentHaystacks[0] === parsed.compactCommentHaystacks[1]);
    assert.ok(parsed.commentsFor(0) === parsed.commentsFor(1));
    assert.equal(parsed.commentsFor(0).instrumentNames, 'Sidecar');
    assert.equal(search(parsed, 'Inline').length, 0);
    assert.deepEqual(attachComments(parsed, null), { joined: 0, unmatched: 0 });
    assert.equal(parsed.commentsFor(0).instrumentNames, 'Inline');
    assert.equal(search(parsed, 'Sidecar').length, 0);
    assert.equal(search(parsed, 'Standalone').length, 1);
});

test('folding, compact matching and comment search options', () => {
    assert.equal(foldText('Café ╬'), 'cafe ╬');
    const parsed = parseLibraryCsv(csv([
        { source_path: 'cafe.mod', title: 'Café', tracker: 'Träckér', instrument_names: 'Purple Motion\nP. Motion\nF.C. ╬' },
        { source_path: 'purple-motion.mod', title: 'Other' },
    ], 'source_path,title,tracker,instrument_names'));
    for (const query of ['purplemotion', 'purple-motion', '"purple motion"', 'p.motion']) assert.ok(search(parsed, query).includes(0), query);
    assert.equal(search(parsed, 'fc').length, 0);
    assert.deepEqual([...search(parsed, 'cafe title:cafe tracker:tracker')], [0]);
    assert.deepEqual([...search(parsed, 'text:purplemotion', { includeComments: false })], [0]);
    assert.deepEqual([...search(parsed, 'purplemotion', { includeComments: false })], [1]);
    assert.deepEqual([...search(parsed, 'purplemotion')], [0, 1]);
    assert.equal(search(parsed, 'name:purplemotion').length, 0);
    assert.equal(search(parsed, 'text:cafe').length, 0);
});

test('match ranges and snippet lines follow the search rules', () => {
    const tokens = query => commentTokens(compileQuery(query));
    assert.deepEqual(commentTokens(compileQuery('xm title:foo text:bar baz text:')).map(token => token.value), ['xm', 'bar', 'baz']);
    assert.deepEqual(commentTokens(compileQuery('xm text:bar'), { includeComments: false }).map(token => token.value), ['bar']);
    // Compact, folded and literal hits map back to the original text, merged where they overlap.
    assert.deepEqual(matchRanges('by Purple Motion / FC', tokens('purplemotion')), [[3, 16]]);
    assert.deepEqual(matchRanges('Café Café', tokens('cafe')), [[0, 4], [5, 9]]);
    assert.deepEqual(matchRanges('Future-Crew', tokens('future-crew')), [[0, 11]]);
    assert.deepEqual(matchRanges('a.b.c fc', tokens('fc')), [[6, 8]]);
    assert.deepEqual(matchRanges('😀 Skaven', tokens('skaven')), [[3, 9]]);
    const comments = { instrumentNames: 'intro\nby Purple\nMotion of FC\n\ngreets', sampleNames: '', songMessage: 'Hello Purple Motion' };
    assert.deepEqual(commentMatchLines(comments, tokens('purple-motion')), [
        { section: 'instrumentNames', first: 1, last: 2, text: 'by Purple\nMotion of FC', ranges: [[3, 16]] },
        { section: 'songMessage', first: 0, last: 0, text: 'Hello Purple Motion', ranges: [[6, 19]] },
    ]);
    assert.deepEqual(commentMatchLines(comments, tokens('greets')).map(hit => [hit.first, hit.ranges]), [[4, [[0, 6]]]]);
    assert.deepEqual(commentMatchLines(comments, []), []);
    assert.deepEqual(commentMatchLines(null, tokens('greets')), []);
});

test('a leading minus excludes rows the rest of the token matches', () => {
    const library = parseLibraryCsv(`source_path,title,channels,sha256\na/ONE.S3M,Oxygen,4,${'1'.repeat(64)}\na/TWO.XM,Oxygene remix,12,\na/THREE.S3M,Air,,${'3'.repeat(64)}\n`);
    attachComments(library, new Map([['1'.repeat(64), { instrumentNames: 'by oxy-gene', sampleNames: '', songMessage: '' }], ['3'.repeat(64), { instrumentNames: 'oxygen tank', sampleNames: '', songMessage: '' }]]));
    const names = (query, options) => Array.from(search(library, query, options), id => library.rows[id].name);
    assert.deepEqual(names('oxygen'), ['ONE.S3M', 'TWO.XM', 'THREE.S3M']);
    // ONE.S3M is excluded by its comment, through compact matching, even though its title is "Oxygen".
    assert.deepEqual(names('oxygen -oxygene'), ['THREE.S3M']);
    assert.deepEqual(names('oxygen -oxygene', { includeComments: false }), ['ONE.S3M']);
    assert.deepEqual(names('oxygen -text:oxygene'), ['TWO.XM', 'THREE.S3M']);
    assert.deepEqual(names('oxygen -title:oxygene'), ['ONE.S3M', 'THREE.S3M']);
    assert.deepEqual(names('-format:s3m -"oxygene remix"'), []);
    assert.deepEqual(names('-ch:>8'), ['ONE.S3M', 'THREE.S3M']);
    assert.deepEqual(names('-text:'), ['ONE.S3M', 'TWO.XM', 'THREE.S3M']);
    assert.deepEqual(compileQuery('"-=FC=-" - -x').map(token => [token.value, token.negated]), [['-=fc=-', false], ['-', false], ['x', true]]);
    assert.deepEqual(commentTokens(compileQuery('oxygen -oxygene')).map(token => token.value), ['oxygen']);
});

test('local Hornet comments compatibility (read only)', async context => {
    const directory = '/home/scott/projects/moddna/index/hornet/';
    try { await access(directory + 'hornet-modules.csv'); await access(directory + 'hornet-comments.csv'); }
    catch { context.skip('Local Hornet CSVs are absent'); return; }
    const library = parseLibraryCsv(await readFile(directory + 'hornet-modules.csv', 'utf8'));
    const parsed = parseCommentsCsv(await readFile(directory + 'hornet-comments.csv', 'utf8'));
    const counts = attachComments(library, parsed.comments);
    const matches = search(library, 'text:"purple motion"').length;
    console.log(`Hornet comments: ${parsed.comments.size} modules, ${parsed.issues.length} issues, joined ${counts.joined}, unmatched ${counts.unmatched}, Purple Motion ${matches}`);
    assert.deepEqual(parsed.issues, []);
    assert.equal(counts.unmatched, 0);
    assert.ok(counts.joined > 9000);
    assert.ok(matches >= 100);
});
