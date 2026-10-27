// Serve the actual --pages output beneath a repository-style subpath, without
// isolation headers. The shipped service worker must supply those headers.
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

export async function runPagesSmoke({ Page, executable, dist, syntheticMod, buildZip }) {
    const requests = [];
    const missing = [];
    const responses = [];
    const prefix = '/starplayer/';
    const server = createServer(async (request, response) => {
        const path = new URL(request.url, 'http://localhost').pathname;
        requests.push(path);
        response.on('finish', () => responses.push({ path, status: response.statusCode }));
        if (!path.startsWith(prefix) || path.includes('..')) { missing.push(path); response.writeHead(404); response.end(); return; }
        const relative = path.slice(prefix.length) || 'index.html';
        const bytes = await readFile(join(dist, relative)).catch(() => null);
        if (!bytes) { missing.push(path); response.writeHead(404); response.end(); return; }
        const type = relative.endsWith('.wasm') ? 'application/wasm' : relative.endsWith('.js') ? 'text/javascript' : relative.endsWith('.css') ? 'text/css' : relative.endsWith('.html') ? 'text/html' : 'application/octet-stream';
        response.writeHead(200, { 'Content-Type': type, 'Cache-Control': 'no-store' }); response.end(bytes);
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    let page;
    try {
        page = await Page.open(executable, `http://127.0.0.1:${server.address().port}${prefix}`);
        await page.waitFor('Pages shim isolates the subpath', "crossOriginIsolated && document.documentElement.dataset.playerReady === 'true'", 40000);
        const zip = buildZip([{ name: 'inner.zip', bytes: buildZip([{ name: 'song.mod', bytes: syntheticMod({ title: 'Pages worker smoke' }) }]) }]);
        const result = await page.evaluate(`
            const { createLibraryWorkerRuntime } = await import('./library-worker-runtime.js');
            const bytes = Uint8Array.from(${JSON.stringify([...zip])});
            const runtime = createLibraryWorkerRuntime({ source: { size: () => bytes.length, readFile: async () => bytes.slice() } });
            try {
                const catalog = await runtime.enqueue({ generation: 1, kind: 'catalog', sourcePath: 'synthetic.zip' }).result;
                let inspect; await runtime.enqueue({ generation: 1, kind: 'inspect-group', sourcePath: 'synthetic.zip', members: [{ token: 'pages', version: 0, locator: { sourcePath: 'synthetic.zip', containerMember: 'inner.zip', memberPath: 'song.mod', rawHex: '' } }], purpose: 'scan' }, { onMember: frame => { inspect = frame.payload; } }).result;
                if (catalog.payload.entries.length !== 1 || inspect.metadata.title !== 'Pages worker smoke') throw new Error('Pages real-worker catalog/metadata failed');
                return { isolated: crossOriginIsolated, serviceWorker: navigator.serviceWorker.controller?.scriptURL, catalog: catalog.payload.entries.length, title: inspect.metadata.title };
            } finally { runtime.dispose(); }
        `);
        const required = ['coi-serviceworker.js', 'library-worker.js', 'library-worker-jobs.js', 'cooperative-yield.js', 'starplayer_web.js', 'starplayer_web_bg.wasm'].map(name => prefix + name);
        for (const path of required) {
            assert.ok(requests.includes(path), `${path} requested under Pages subpath`);
            assert.ok(responses.some(response => response.path === path && response.status === 200), `${path} served successfully`);
        }
        // Pages intentionally ships no fixture manifest; the app probes it before
        // and after the service-worker reload. Account for every actual server404
        // path before allowing Chromium's corresponding generic resource diagnostic.
        const optional = new Set(['/library-index.csv', '/library-comments.csv', prefix + 'modules/index.json']);
        assert.deepEqual(missing.filter(path => !optional.has(path)), [], 'unexpected missing Pages resource');
        assert.deepEqual(responses.filter(response => response.status !== 200 && !(response.status === 404 && optional.has(response.path))), [], 'unexpected Pages response status');
        const missingDiagnostic = 'Failed to load resource: the server responded with a status of 404 (Not Found)';
        assert.ok(page.consoleErrors.filter(error => error === missingDiagnostic).length <= missing.length);
        assert.deepEqual(page.consoleErrors.filter(error => error !== missingDiagnostic), []);
        return { ...result, workerAssetsUnderSubpath: true, optionalMissingResources: missing, assetResponses: responses.filter(response => required.includes(response.path) || response.status === 404) };

    } finally {
        if (page) await page.close();
        await new Promise(resolve => { server.close(resolve); server.closeAllConnections(); });
    }
}
