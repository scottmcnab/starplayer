// Development server for the web player. Node's built-in `http` and nothing else — no
// npm dependencies, so `cargo xtask serve` works on a machine that has never run
// `npm install`.
//
// Its one job beyond serving files is the pair of headers that make
// `SharedArrayBuffer` available:
//
//   Cross-Origin-Opener-Policy:   same-origin
//   Cross-Origin-Embedder-Policy: require-corp
//
// Together those make the document *cross-origin isolated*, which is the precondition
// browsers have required for `SharedArrayBuffer` since Spectre. Without them the page
// still works — it falls back to `postMessage` and says so — which is exactly the
// comparison this spike exists to make.
//
// `--no-isolation` omits them deliberately, which is how the graceful-degradation half
// of the player is exercised: the page detects that `SharedArrayBuffer` is unavailable,
// says so, and drives the worklet over batched `postMessage` instead.
//
// Usage: node apps/starplayer-web/dev-server.mjs [--port 8080] [--host 127.0.0.1] [--root <dir>] [--no-isolation] [--tls [--tls-san IP,DNS,...]]
//
// `--host 0.0.0.0` exposes the server to the LAN (a phone, or another machine). Note that a
// plain-http LAN origin is not a secure context, so the browser withholds
// `SharedArrayBuffer` there and the page runs on its `postMessage` fallback — which is a
// legitimate thing to test, but not the same path as `http://localhost`.
//
// Worse than that: `AudioContext.audioWorklet` is *undefined* outside a secure context, so
// over plain http on a LAN address the player cannot start at all. `--tls` serves https
// with a self-signed certificate generated on first use into `.dev-tls/` (git-ignored),
// with subjectAltNames for localhost plus whatever `--tls-san` lists — the browser will
// warn once about the certificate; accept it and the page is a secure context.

import { createServer as createHttpServer } from 'node:http';
import { createServer as createHttpsServer } from 'node:https';
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync } from 'node:fs';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { readFile, stat } from 'node:fs/promises';
import { join, normalize, extname, resolve } from 'node:path';

const CONTENT_TYPES = {
    '.html': 'text/html; charset=utf-8',
    '.js': 'text/javascript; charset=utf-8',
    '.mjs': 'text/javascript; charset=utf-8',
    '.css': 'text/css; charset=utf-8',
    '.json': 'application/json; charset=utf-8',
    // `WebAssembly.compileStreaming` refuses anything else, and wasm-bindgen's glue
    // prints a warning and takes a slower path if this is wrong.
    '.wasm': 'application/wasm',
    '.s3m': 'application/octet-stream',
    '.map': 'application/json; charset=utf-8',
};

function parseArguments(argv) {
    const options = { port: 8080, host: '127.0.0.1', root: 'apps/starplayer-web/dist', isolate: true, tls: false, tlsSan: [] };
    for (let index = 0; index < argv.length; index += 1) {
        if (argv[index] === '--port') {
            options.port = Number(argv[index + 1]);
            index += 1;
        } else if (argv[index] === '--host') {
            options.host = argv[index + 1];
            index += 1;
        } else if (argv[index] === '--root') {
            options.root = argv[index + 1];
            index += 1;
        } else if (argv[index] === '--no-isolation') {
            options.isolate = false;
        } else if (argv[index] === '--tls') {
            options.tls = true;
        } else if (argv[index] === '--tls-san') {
            options.tlsSan = argv[index + 1].split(',').map((entry) => entry.trim()).filter(Boolean);
            index += 1;
        } else {
            throw new Error(`unexpected argument \`${argv[index]}\``);
        }
    }
    if (!Number.isInteger(options.port) || options.port <= 0 || options.port > 65535) {
        throw new Error(`invalid port \`${options.port}\``);
    }
    return options;
}

const options = parseArguments(process.argv.slice(2));
const documentRoot = resolve(options.root);

function applyCommonHeaders(response) {
    if (options.isolate) {
        response.setHeader('Cross-Origin-Opener-Policy', 'same-origin');
        response.setHeader('Cross-Origin-Embedder-Policy', 'require-corp');
        // Required so the isolated document is allowed to load its own subresources.
        response.setHeader('Cross-Origin-Resource-Policy', 'same-origin');
    }
    // A dev server that caches is a dev server that lies about the build you just made.
    response.setHeader('Cache-Control', 'no-store');
}

/**
 * Resolves a URL path to a file inside the document root, or null if it escapes it.
 * `normalize` collapses `..` before the prefix check, so `/../../etc/passwd` cannot
 * reach outside the root.
 */
function resolveRequestPath(urlPath) {
    const decoded = decodeURIComponent(urlPath.split('?')[0]);
    const relative = normalize(decoded === '/' ? '/index.html' : decoded).replace(/^(\.\.[/\\])+/, '');
    const candidate = resolve(join(documentRoot, relative));
    return candidate === documentRoot || candidate.startsWith(documentRoot + '/') ? candidate : null;
}

/// A self-signed certificate for `--tls`, generated once with the system `openssl`.
function developmentTlsOptions(extraSubjectAltNames) {
    const directory = join(dirname(fileURLToPath(import.meta.url)), '.dev-tls');
    const certificatePath = join(directory, 'cert.pem');
    const keyPath = join(directory, 'key.pem');
    if (!existsSync(certificatePath) || !existsSync(keyPath)) {
        mkdirSync(directory, { recursive: true });
        const subjectAltNames = ['DNS:localhost', 'IP:127.0.0.1', ...extraSubjectAltNames.map((entry) => (/^[0-9.]+$/.test(entry) ? `IP:${entry}` : `DNS:${entry}`))];
        execFileSync('openssl', [
            'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '825',
            '-keyout', keyPath, '-out', certificatePath,
            '-subj', '/CN=starplayer-dev',
            '-addext', `subjectAltName=${subjectAltNames.join(',')}`,
        ], { stdio: 'ignore' });
        console.log(`  generated a self-signed certificate in ${directory} (${subjectAltNames.join(', ')})`);
    }
    return { cert: readFileSync(certificatePath), key: readFileSync(keyPath) };
}

const createServer = options.tls
    ? (handler) => createHttpsServer(developmentTlsOptions(options.tlsSan), handler)
    : createHttpServer;

const server = createServer(async (request, response) => {
    applyCommonHeaders(response);

    if (request.method !== 'GET' && request.method !== 'HEAD') {
        response.writeHead(405, { 'Content-Type': 'text/plain; charset=utf-8', Allow: 'GET, HEAD' });
        response.end('method not allowed\n');
        return;
    }

    const filePath = resolveRequestPath(request.url);
    if (filePath === null) {
        response.writeHead(403, { 'Content-Type': 'text/plain; charset=utf-8' });
        response.end('forbidden\n');
        return;
    }

    try {
        const info = await stat(filePath);
        if (!info.isFile()) {
            throw Object.assign(new Error('not a file'), { code: 'ENOENT' });
        }
        const body = await readFile(filePath);
        response.writeHead(200, {
            // Lower-cased: the packaged modules are `REFLEX.S3M`, not `reflex.s3m`.
            'Content-Type': CONTENT_TYPES[extname(filePath).toLowerCase()] ?? 'application/octet-stream',
            'Content-Length': body.length,
        });
        response.end(request.method === 'HEAD' ? undefined : body);
    } catch (error) {
        const missing = error.code === 'ENOENT' || error.code === 'ENOTDIR';
        response.writeHead(missing ? 404 : 500, { 'Content-Type': 'text/plain; charset=utf-8' });
        response.end(missing ? 'not found\n' : `server error: ${error.message}\n`);
    } finally {
        // One line per request: a 404 that nobody sees is a 404 that gets debugged in the
        // browser console instead of here.
        console.log(`  ${response.statusCode} ${request.method} ${request.url}`);
    }
});

server.listen(options.port, options.host, () => {
    const scheme = options.tls ? 'https' : 'http';
    console.log(`starplayer dev server: ${scheme}://localhost:${options.port}/`);
    if (options.host !== '127.0.0.1') {
        console.log(`  bound to ${options.host} — reachable from the LAN${options.tls ? '' : ' (plain http there is NOT a secure context: no AudioWorklet, no SharedArrayBuffer — use --tls)'}`);
    }
    console.log(`  document root: ${documentRoot}`);
    if (options.isolate) {
        console.log('  Cross-Origin-Opener-Policy: same-origin');
        console.log('  Cross-Origin-Embedder-Policy: require-corp');
    } else {
        console.log('  --no-isolation: no COOP/COEP, so the page takes its postMessage fallback');
    }
    console.log('  press Ctrl-C to stop');
});

server.on('error', (error) => {
    console.error(`starplayer dev server: ${error.message}`);
    process.exitCode = 1;
});

for (const signal of ['SIGINT', 'SIGTERM']) {
    process.on(signal, () => server.close(() => process.exit(0)));
}
