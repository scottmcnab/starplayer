const DEFAULT_UPSTREAM = 'https://hornet.org/music/';
const DEFAULT_ORIGINS = 'https://scottmcnab.github.io,http://localhost:8080,http://127.0.0.1:8080';
const MAX_BYTES = 128 * 1024 * 1024;

export function upstreamUrl(requestUrl, base = DEFAULT_UPSTREAM) {
    const upstream = new URL(base);
    if (!/^https?:$/.test(upstream.protocol) || !base.endsWith('/') || upstream.search || upstream.hash || upstream.username || upstream.password) throw new Error('Invalid UPSTREAM');
    // Use the raw path, before URL dot-segment normalisation. A real Request may
    // already have normalised literal dots; helpers also accept unnormalised URLs.
    const path = requestUrl.replace(/^https?:\/\/[^/]+/i, '').split('?')[0];
    const segments = path.slice(1).split('/').map(decodeURIComponent);
    if (!path.startsWith('/') || segments.some(segment => !segment || segment === '.' || segment === '..' || /[/\\\0]/.test(segment))) throw new Error('Invalid path');
    return base + segments.map(encodeURIComponent).join('/');
}

export function responseHeaders(origin) {
    const headers = new Headers({ 'Cross-Origin-Resource-Policy': 'cross-origin' });
    if (origin) {
        headers.set('Access-Control-Allow-Origin', origin);
        headers.set('Access-Control-Expose-Headers', 'Content-Length');
        headers.set('Vary', 'Origin');
    }
    return headers;
}

export default {
    async fetch(request, env = {}) {
        const origin = request.headers.get('Origin');
        const allowed = (env.ALLOWED_ORIGINS ?? DEFAULT_ORIGINS).split(',').map(value => value.trim());
        const permitted = !origin || allowed.includes(origin);
        const headers = responseHeaders(permitted ? origin : null);
        const reply = (status, text) => new Response(request.method === 'HEAD' || [204, 205, 304].includes(status) ? null : text, { status, headers });
        if (!permitted) return reply(403, 'Origin not allowed');
        if (!['GET', 'HEAD', 'OPTIONS'].includes(request.method)) return reply(405, 'Method not allowed');
        if (request.method === 'OPTIONS') {
            if (origin) {
                headers.set('Access-Control-Allow-Methods', 'GET, HEAD, OPTIONS');
                headers.set('Access-Control-Max-Age', '86400');
            }
            return reply(204);
        }
        let url;
        try { url = upstreamUrl(request.url, env.UPSTREAM ?? DEFAULT_UPSTREAM); }
        catch { return reply(400, 'Invalid proxy path or UPSTREAM'); }
        let response;
        try {
            response = await fetch(url, { method: request.method, headers: { 'User-Agent': 'StarPlayer-proxy (+https://github.com/scottmcnab/starplayer)' },
                redirect: 'manual', cf: { cacheEverything: true, cacheTtl: 2592000 } });
        } catch { return reply(502, 'Upstream fetch failed'); }
        if (Number(response.headers.get('Content-Length')) > MAX_BYTES) {
            await response.body?.cancel();
            return reply(413, 'Upstream file exceeds 128 MiB');
        }
        if (!response.ok) {
            await response.body?.cancel();
            return reply(response.status, `Upstream HTTP ${response.status}`);
        }
        for (const name of ['Content-Type', 'Content-Length', 'ETag', 'Last-Modified']) {
            if (response.headers.has(name)) headers.set(name, response.headers.get(name));
        }
        if (response.status === 200) headers.set('Cache-Control', 'public, max-age=31536000, immutable');
        return new Response(request.method === 'HEAD' ? null : response.body, { status: response.status, headers });
    },
};
