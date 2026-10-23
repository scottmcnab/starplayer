import test from 'node:test';
import assert from 'node:assert/strict';
import worker, { upstreamUrl } from '../remote-proxy/worker.js';

// Stubs never contact any network peer.
test('proxy validates raw paths, methods and origins, and forwards bounded responses', async () => {
    const original = globalThis.fetch;
    const calls = [];
    let status = 200;
    let length = '3';
    globalThis.fetch = async (...args) => {
        calls.push(args);
        return new Response(args[1].method === 'HEAD' ? null : 'abc', { status, headers: {
            'Content-Length': length, 'Content-Type': 'audio/test', ETag: 'test', 'Last-Modified': 'yesterday',
        } });
    };
    // A Request normalises dot segments before the Worker receives it. Supply raw
    // URLs here as well, to test validation of paths that have not been normalised.
    const request = (path, method = 'GET', origin = 'http://localhost:8080') => ({ url: 'https://proxy.test' + path, method, headers: new Headers(origin ? { Origin: origin } : {}) });
    try {
        assert.equal(upstreamUrl('https://proxy.test/a%20b/c%23.s3m?ignored=yes'), 'https://hornet.org/music/a%20b/c%23.s3m');
        for (const path of ['/..', '/.', '/a//b', '/%2e%2e', '/%2F', '/%5c', '/%00', '/%zz', '/']) {
            assert.equal((await worker.fetch(request(path))).status, 400, path);
        }
        assert.equal(calls.length, 0);
        assert.equal((await worker.fetch(request('/song', 'POST'))).status, 405);
        const preflight = await worker.fetch(request('/song', 'OPTIONS'));
        assert.equal(preflight.status, 204);
        assert.equal(preflight.headers.get('Access-Control-Allow-Origin'), 'http://localhost:8080');
        assert.equal(preflight.headers.get('Access-Control-Allow-Methods'), 'GET, HEAD, OPTIONS');
        assert.equal(preflight.headers.get('Access-Control-Max-Age'), '86400');
        assert.equal(preflight.headers.get('Vary'), 'Origin');
        assert.equal((await worker.fetch(request('/song', 'GET', 'https://disallowed.test'))).status, 403);
        assert.equal(calls.length, 0);
        const response = await worker.fetch(request('/a%20b/c%23.s3m?ignore'));
        assert.equal(await response.text(), 'abc');
        assert.equal(calls[0][0], 'https://hornet.org/music/a%20b/c%23.s3m');
        assert.deepEqual(calls[0][1].cf, { cacheEverything: true, cacheTtl: 2592000 });
        assert.equal(calls[0][1].redirect, 'manual');
        assert.match(calls[0][1].headers['User-Agent'], /^StarPlayer-proxy/);
        for (const [name, value] of Object.entries({ 'content-type': 'audio/test', 'content-length': '3', etag: 'test', 'last-modified': 'yesterday',
            'cross-origin-resource-policy': 'cross-origin', 'cache-control': 'public, max-age=31536000, immutable',
            'access-control-allow-origin': 'http://localhost:8080', 'access-control-expose-headers': 'Content-Length', vary: 'Origin' })) assert.equal(response.headers.get(name), value);
        const noOrigin = await worker.fetch(request('/song', 'GET', ''));
        assert.equal(noOrigin.headers.get('Access-Control-Allow-Origin'), null);
        assert.equal(noOrigin.headers.get('Access-Control-Expose-Headers'), null);
        assert.equal((await worker.fetch(request('/song', 'OPTIONS', ''))).headers.get('Access-Control-Allow-Methods'), null);
        assert.equal(await (await worker.fetch(request('/song', 'HEAD'))).text(), '');
        status = 404;
        assert.equal((await worker.fetch(request('/missing'))).status, 404);
        status = 302;
        assert.equal((await worker.fetch(request('/redirect'))).status, 302);
        status = 200; length = String(128 * 1024 * 1024 + 1);
        assert.equal((await worker.fetch(request('/large'))).status, 413);
        length = '3';
        assert.equal((await worker.fetch(request('/song', 'GET', 'https://custom.test'), { UPSTREAM: 'http://127.0.0.1:1234/base/', ALLOWED_ORIGINS: ' https://custom.test ' })).status, 200);
        assert.equal(calls.at(-1)[0], 'http://127.0.0.1:1234/base/song');
        assert.equal(upstreamUrl('https://proxy.test/song', 'http://LOCALHOST:80/base/'), 'http://LOCALHOST:80/base/song');
        assert.equal((await worker.fetch(request('/song'), { UPSTREAM: 'https://archive.test/no-slash' })).status, 400);
        globalThis.fetch = async () => { throw new TypeError('offline'); };
        assert.equal((await worker.fetch(request('/song'))).status, 502);
    } finally { globalThis.fetch = original; }
});
