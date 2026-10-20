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
// Usage: node apps/starplayer-web/dev-server.mjs [--port 8080] [--host 127.0.0.1] [--root <dir>] [--library <dir>] [--library-index <file>] [--no-isolation] [--tls [--tls-san IP,DNS,...]]
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
import { readFile, stat, realpath } from 'node:fs/promises';
import { join, extname, resolve, sep } from 'node:path';

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
        } else if (argv[index] === '--library' || argv[index] === '--library-index') {
            if (!argv[index + 1] || argv[index + 1].startsWith('--')) throw new Error(`${argv[index]} needs a value`);
            options[argv[index] === '--library' ? 'library' : 'libraryIndex'] = argv[++index];
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
    if (!Number.isInteger(options.port) || options.port < 0 || options.port > 65535) {
        throw new Error(`invalid port \`${options.port}\``);
    }
    return options;
}

const options = parseArguments(process.argv.slice(2));
const documentRoot = await realpath(resolve(options.root));
const libraryRoot = options.library ? await realpath(resolve(options.library)) : null;
const libraryIndex = libraryRoot ? resolve(options.libraryIndex ?? join(libraryRoot, 'index.csv')) : null;

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

// Decode each segment once. Encoded separators must never become filesystem paths.
async function confinedPath(root, urlPath) {
    const segments = urlPath.split('/').map(segment => decodeURIComponent(segment));
    if (segments.some(segment => !segment || segment === '.' || segment === '..' || /[/\\\0]/.test(segment))) {
        throw Object.assign(new Error('forbidden'), { status: 403 });
    }
    const target = await realpath(join(root, ...segments));
    if (target !== root && !target.startsWith(root + sep)) throw Object.assign(new Error('forbidden'), { status: 403 });
    return target;
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

    try {
        const urlPath = request.url.split('?')[0];
        let filePath;
        let isIndex = false;
        if (urlPath === '/library-index.csv' || urlPath === '/library' || urlPath.startsWith('/library/')) {
            if (!libraryRoot) throw Object.assign(new Error('not found'), { code: 'ENOENT' });
            isIndex = urlPath === '/library-index.csv';
            filePath = isIndex ? libraryIndex : await confinedPath(libraryRoot, urlPath.slice('/library/'.length));
        } else {
            filePath = await confinedPath(documentRoot, urlPath === '/' ? 'index.html' : urlPath.slice(1));
        }
        const info = await stat(filePath);
        if (!info.isFile()) {
            throw Object.assign(new Error('not a file'), { code: 'ENOENT' });
        }
        const body = request.method === 'HEAD' ? undefined : await readFile(filePath);
        response.writeHead(200, {
            // Lower-cased: the packaged modules are `REFLEX.S3M`, not `reflex.s3m`.
            'Content-Type': isIndex ? 'text/csv; charset=utf-8' : CONTENT_TYPES[extname(filePath).toLowerCase()] ?? 'application/octet-stream',
            'Content-Length': info.size,
        });
        response.end(request.method === 'HEAD' ? undefined : body);
    } catch (error) {
        const missing = error.code === 'ENOENT' || error.code === 'ENOTDIR';
        response.writeHead(error instanceof URIError ? 400 : error.status ?? (missing ? 404 : 500), { 'Content-Type': 'text/plain; charset=utf-8' });
        response.end(missing ? 'not found\n' : `${error.message}\n`);
    } finally {
        // One line per request: a 404 that nobody sees is a 404 that gets debugged in the
        // browser console instead of here.
        console.log(`  ${response.statusCode} ${request.method} ${request.url}`);
    }
});

server.listen(options.port, options.host, () => {
    const scheme = options.tls ? 'https' : 'http';
    console.log(`starplayer dev server: ${scheme}://localhost:${server.address().port}/`);
    if (options.host !== '127.0.0.1') {
        console.log(`  bound to ${options.host} — reachable from the LAN${options.tls ? '' : ' (plain http there is NOT a secure context: no AudioWorklet, no SharedArrayBuffer — use --tls)'}`);
    }
    console.log(`  document root: ${documentRoot}`);
    if (libraryRoot) console.log(`  library root: ${libraryRoot}`);
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
