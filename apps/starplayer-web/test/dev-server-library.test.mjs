import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, symlink, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
import { spawn } from 'node:child_process';
import { request } from 'node:http';
import { once } from 'node:events';

async function startServer(root, argumentsList = []) {
    const child = spawn(process.execPath, ['apps/starplayer-web/dev-server.mjs', '--port', '0', '--root', root, ...argumentsList], { stdio: ['ignore', 'pipe', 'pipe'] });
    let output = '';
    const port = await new Promise((resolvePort, reject) => {
        const timer = setTimeout(() => { child.kill(); reject(new Error(`server startup timeout: ${output}`)); }, 10000);
        child.stdout.on('data', bytes => {
            output += bytes;
            const match = /localhost:(\d+)\//.exec(output);
            if (match) { clearTimeout(timer); resolvePort(Number(match[1])); }
        });
        child.stderr.on('data', bytes => { output += bytes; });
        child.once('exit', () => { clearTimeout(timer); reject(new Error(output)); });
    });
    return { port, async stop() { const exited = once(child, 'exit'); child.kill(); await exited; } };
}
function get(port, path, method = 'GET') {
    // Raw paths preserve traversal probes that fetch/URL would normalise away.
    return new Promise((resolveResponse, reject) => {
        const outgoing = request({ hostname: '127.0.0.1', port, path, method }, response => {
            const chunks = [];
            response.on('data', chunk => chunks.push(chunk));
            response.on('end', () => resolveResponse({ status: response.statusCode, headers: response.headers, body: Buffer.concat(chunks).toString() }));
        });
        outgoing.on('error', reject); outgoing.end();
    });
}

test('library routes serve only mapped files and reject traversal, symlinks and malformed paths', async () => {
    const root = await mkdtemp(join(tmpdir(), 'starplayer-w8-'));
    let server;
    try {
        const library = join(root, 'music');
        await mkdir(library);
        await mkdir(join(root, 'music-other'));
        await writeFile(join(root, 'index.html'), 'page');
        await writeFile(join(root, 'dev-server.mjs'), 'private');
        await writeFile(join(root, 'music-other', 'secret.mod'), 'secret');
        await writeFile(join(library, 'song.mod'), 'module');
        await writeFile(join(library, 'a #%?.mod'), 'special');
        await writeFile(join(library, 'index.csv'), 'wrong index');
        await writeFile(join(root, 'chosen.csv'), 'source_path\nsong.mod\n');
        await symlink(join(root, 'music-other'), join(library, 'escape'));
        await symlink('/etc', join(root, 'document-escape'));
        server = await startServer(root, ['--library', library, '--library-index', join(root, 'chosen.csv')]);
        assert.equal((await get(server.port, '/')).body, 'page');
        assert.equal((await get(server.port, '/index.html')).body, 'page');
        assert.equal((await get(server.port, '/index.html', 'HEAD')).headers['content-length'], '4');
        const served = await get(server.port, '/library/song.mod');
        assert.equal(served.status, 200);
        assert.equal(served.body, 'module');
        assert.equal(served.headers['content-length'], '6');
        assert.equal(served.headers['cross-origin-opener-policy'], 'same-origin');
        assert.equal(served.headers['cross-origin-embedder-policy'], 'require-corp');
        assert.equal(served.headers['cross-origin-resource-policy'], 'same-origin');
        const head = await get(server.port, '/library/song.mod', 'HEAD');
        assert.equal(head.status, 200);
        assert.equal(head.headers['content-length'], '6');
        assert.equal(head.body, '');
        assert.equal((await get(server.port, '/library/' + encodeURIComponent('a #%?.mod'))).body, 'special');
        for (const path of ['/library/../dev-server.mjs', '/library/%2e%2e%2fdev-server.mjs', '/library/%2F..%2Fdev-server.mjs', '/library/escape/secret.mod', '/library/../music-other/secret.mod', '/library//song.mod', '/library/./song.mod', '/library/%00', '/document-escape/hosts']) {
            assert.equal((await get(server.port, path)).status, 403, path);
        }
        for (const path of ['/library/%zz', '/%zz']) assert.equal((await get(server.port, path)).status, 400);
        assert.equal((await get(server.port, '/library/missing.mod')).status, 404);
        assert.equal((await get(server.port, '/library/escape')).status, 403);
        assert.equal((await get(server.port, '/library/song.mod', 'POST')).status, 405);
        const index = await get(server.port, '/library-index.csv');
        assert.equal(index.body, 'source_path\nsong.mod\n');
        assert.equal(index.headers['content-type'], 'text/csv; charset=utf-8');
        assert.equal((await get(server.port, '/library-index.csv', 'HEAD')).headers['content-length'], String(Buffer.byteLength(index.body)));
        await server.stop(); server = null;
        server = await startServer(root, ['--library', library]);
        assert.equal((await get(server.port, '/library-index.csv')).body, 'wrong index');
        await server.stop(); server = null;
        server = await startServer(root);
        for (const path of ['/library/song.mod', '/library-index.csv']) assert.equal((await get(server.port, path)).status, 404);
    } finally {
        if (server) await server.stop();
        await rm(root, { recursive: true, force: true });
    }
});
