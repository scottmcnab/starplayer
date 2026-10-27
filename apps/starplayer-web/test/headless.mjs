import remoteWorker from '../remote-proxy/worker.js';
import { runW15, runW15View, runW15Stress, runW15Playlist, runW15Runtime } from './w15-headless.mjs';
import { runRemediation, runPerformance, runBackground, runDigestMeasurements, runVisibilityReturn, runEnumerationDiagnostic } from './w15-remediation-headless.mjs';
import { runPagesSmoke } from './w15-pages-smoke.mjs';
import { verifyPlaylistStore } from './playlist-store.test.mjs';
// End-to-end check of the packaged player in headless Chromium, over the DevTools
// protocol, with Node's built-in `WebSocket` and `http` and nothing else installed.
//
// It runs the page three ways, because the three are genuinely different code paths:
//
//   sab       cross-origin isolated, SharedArrayBuffer command ring + telemetry seqlock
//   fallback  cross-origin isolated, but the page's "Test postMessage fallback" box on
//   plain     no COOP/COEP at all, so `crossOriginIsolated` is false and the page has to
//             notice that for itself
//
// Each run loads a fixture, plays for a while, loads a second module over the top of the
// first, drops a deliberately broken file on it, and finally shrinks the viewport to a
// phone. It asserts on what the page itself displays — the same text a human reads — so
// a regression in the display is as visible here as a regression in the engine.
//
// Usage: node apps/starplayer-web/test/headless.mjs [--seconds 60] [--mode sab|fallback|plain]
// W15-only: --w15-only, or --w15-case app|view|stress|playlist|runtime|remediation.
// Dedicated evidence: --w15-case performance|digest|background|visibility|enumeration (select one mode).
// Background runs at least six minutes with normal throttling. STARPLAYER_BACKGROUND_SMOKE=1
// exercises its lifecycle paths quickly and is explicitly labeled non-acceptance smoke.
// After cargo xtask wasm --pages: --pages-smoke serves a local repository subpath.
//
// Exits 0 and prints `headless: skipped` when no Chromium is installed.

import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import { spawn } from 'node:child_process';
import { access, readFile, readdir, mkdir, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { crc32, deflateRawSync } from 'node:zlib';

const REPOSITORY_ROOT = fileURLToPath(new URL('../../../', import.meta.url));
const DEV_SERVER = join(REPOSITORY_ROOT, 'apps/starplayer-web/dev-server.mjs');
const DIST = join(REPOSITORY_ROOT, 'apps/starplayer-web/dist');
const FIRST_MODULE = 'REFLEX.S3M';
const SECOND_MODULE = 'PETRI.S3M';
const PHONE_VIEWPORT = { width: 390, height: 844 };

const options = parseArguments(process.argv.slice(2));

function parseArguments(argv) {
    const parsed = { seconds: 60, modes: ['sab', 'fallback', 'plain'], libraryOnly: false, w15Only: false, w15Case: null, pagesSmoke: false };
    for (let index = 0; index < argv.length; index += 1) {
        if (argv[index] === '--pages-smoke') parsed.pagesSmoke = true;
        else if (argv[index] === '--w15-only') parsed.w15Only = true;
        else if (argv[index] === '--w15-case') { parsed.w15Case = argv[++index]; parsed.w15Only = true; }
        else if (argv[index] === '--library-only') parsed.libraryOnly = true;
        else if (argv[index] === '--seconds') {
            parsed.seconds = Number(argv[index + 1]);
            index += 1;
        } else if (argv[index] === '--mode') {
            parsed.modes = [argv[index + 1]];
            index += 1;
        } else {
            throw new Error(`unexpected argument \`${argv[index]}\``);
        }
    }
    if (parsed.w15Case !== null && !['app', 'view', 'stress', 'playlist', 'runtime', 'remediation', 'performance', 'background', 'digest', 'visibility', 'enumeration'].includes(parsed.w15Case)) throw new Error('invalid --w15-case');
    if (!parsed.modes.every(mode => ['sab', 'fallback', 'plain'].includes(mode))) throw new Error('invalid --mode');
    return parsed;
}

// ── environment ─────────────────────────────────────────────────────────────────────

async function verifyPackagedSources() {
    const names=['cooperative-yield.js','library-source.js','library-discovery.js','library-worker.js','library-worker-runtime.js','library-worker-jobs.js','library-index.js','library-controls.js','playlist-transfer.js','playlist-controls.js','library-view.js','library-view-state.js'];
    const hashes={};
    for(const name of names) {
        const source=await readFile(join(REPOSITORY_ROOT,'apps/starplayer-web/www',name));
        const packaged=await readFile(join(DIST,name)).catch(()=>null);
        if(!packaged||!source.equals(packaged))throw new Error(`Stale packaged asset ${name}; rebuild packaged assets with cargo xtask wasm in ${REPOSITORY_ROOT}`);
        hashes[name]=createHash('sha256').update(source).digest('hex');
    }
    console.log(`packaged-source-preflight ${JSON.stringify(hashes)}`);
}

async function findChromium() {
    if (process.env.CHROME) return process.env.CHROME;
    const cache = join(homedir(), '.cache/ms-playwright');
    const directories = await readdir(cache).catch(() => []);
    for (const directory of directories.filter((name) => name.startsWith('chromium-')).sort().reverse()) {
        const candidate = join(cache, directory, 'chrome-linux64/chrome');
        if (await access(candidate).then(() => true, () => false)) return candidate;
        const older = join(cache, directory, 'chrome-linux/chrome');
        if (await access(older).then(() => true, () => false)) return older;
    }
    for (const candidate of ['/usr/bin/chromium', '/usr/bin/chromium-browser', '/usr/bin/google-chrome']) {
        if (await access(candidate).then(() => true, () => false)) return candidate;
    }
    return null;
}

async function freePort() {
    return new Promise((resolve, reject) => {
        const probe = createServer();
        probe.on('error', reject);
        probe.listen(0, '127.0.0.1', () => {
            const { port } = probe.address();
            probe.close(() => resolve(port));
        });
    });
}

/// One physical key, pressed and released through the DevTools protocol — a real
/// `KeyboardEvent` with a real `code`, which is what the page's map is keyed on.
async function pressKey(page, code, key, virtualKeyCode) {
    const base = { code, key, windowsVirtualKeyCode: virtualKeyCode, nativeVirtualKeyCode: virtualKeyCode };
    await page.send('Input.dispatchKeyEvent', { type: 'keyDown', ...base });
    await delay(30);
    await page.send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
    await delay(30);
}

function delay(milliseconds) {
    return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

// Build the fixture ZIPs independently of starplayer-archive. Only the standard local
// header, central-directory record and EOCD are needed for these Deflate entries.
function buildZip(entries) {
    const localRecords = [];
    const centralRecords = [];
    let localOffset = 0;
    for (const entry of entries) {
        const name = Buffer.from(entry.name, 'utf8');
        const bytes = Buffer.from(entry.bytes);
        const compressed = entry.stored ? bytes : deflateRawSync(bytes);
        const checksum = crc32(bytes) >>> 0;

        const localHeader = Buffer.alloc(30);
        localHeader.writeUInt32LE(0x04034B50, 0);
        localHeader.writeUInt16LE(20, 4);
        localHeader.writeUInt16LE(entry.encrypted ? 0x0801 : 0x0800, 6);
        localHeader.writeUInt16LE(entry.stored ? 0 : 8, 8);
        localHeader.writeUInt32LE(checksum, 14);
        localHeader.writeUInt32LE(compressed.length, 18);
        localHeader.writeUInt32LE(bytes.length, 22);
        localHeader.writeUInt16LE(name.length, 26);
        localRecords.push(localHeader, name, compressed);

        const centralHeader = Buffer.alloc(46);
        centralHeader.writeUInt32LE(0x02014B50, 0);
        centralHeader.writeUInt16LE(20, 4);
        centralHeader.writeUInt16LE(20, 6);
        centralHeader.writeUInt16LE(entry.encrypted ? 0x0801 : 0x0800, 8);
        centralHeader.writeUInt16LE(entry.stored ? 0 : 8, 10);
        centralHeader.writeUInt32LE(checksum, 16);
        centralHeader.writeUInt32LE(compressed.length, 20);
        centralHeader.writeUInt32LE(bytes.length, 24);
        centralHeader.writeUInt16LE(name.length, 28);
        centralHeader.writeUInt32LE(localOffset, 42);
        centralRecords.push(centralHeader, name);
        localOffset += localHeader.length + name.length + compressed.length;
    }

    const centralDirectory = Buffer.concat(centralRecords);
    const end = Buffer.alloc(22);
    end.writeUInt32LE(0x06054B50, 0);
    end.writeUInt16LE(entries.length, 8);
    end.writeUInt16LE(entries.length, 10);
    end.writeUInt32LE(centralDirectory.length, 12);
    end.writeUInt32LE(localOffset, 16);
    return Buffer.concat([...localRecords, centralDirectory, end]);
}

/// A minimal two-pattern MOD. `channels` picks the tag, so the caller can build two
/// modules that are distinguishable in telemetry by their channel count alone. `speed`
/// puts an `Fxx` speed command on row 0 of channel 0, which is what the "Adjust speed"
/// option biases — a module with no speed command of its own would show nothing.
function syntheticMod({ channels = 4, tag = 'M.K.', title = 'headphone test', speed = null } = {}) {
    const headerBytes = 1084;
    const patternBytes = 64 * channels * 4;
    const sampleFrames = 256;
    const bytes = Buffer.alloc(headerBytes + patternBytes * 2 + sampleFrames);
    bytes.write(title, 0, 'ascii');
    bytes.writeUInt16BE(sampleFrames / 2, 42);
    bytes[45] = 64;
    bytes[950] = 2;
    bytes[952] = 0;
    bytes[953] = 1;
    bytes.write(tag, 1080, 'ascii');
    for (let channel = 0; channel < channels; channel += 1) {
        bytes.set([0x01, 0xAC, 0x10, 0x00], headerBytes + channel * 4);
        bytes.set([0x01, 0xAC, 0x10, 0x00], headerBytes + patternBytes + channel * 4);
    }
    if (speed !== null) {
        // Effect F in the low nibble of byte 2, its parameter in byte 3 — on row 0 of both
        // patterns, so the speed is re-requested rather than merely inherited.
        bytes.set([0x01, 0xAC, 0x1F, speed], headerBytes);
        bytes.set([0x01, 0xAC, 0x1F, speed], headerBytes + patternBytes);
    }
    for (let index = headerBytes + patternBytes * 2; index < bytes.length; index += 1) {
        bytes[index] = index & 1 ? 0x80 : 0x7F;
    }
    return bytes;
}

async function startDevServer(isolate, libraryDirectory = null) {
    const port = await freePort();
    const argv = [DEV_SERVER, '--port', String(port), '--root', DIST];
    if (!isolate) argv.push('--no-isolation');
    if (libraryDirectory) argv.push('--library', libraryDirectory, '--library-comments', join(libraryDirectory, 'comments.csv'));
    const child = spawn(process.execPath, argv, { stdio: ['ignore', 'pipe', 'inherit'] });
    await new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error('the dev server did not start')), 10_000);
        child.stdout.on('data', (chunk) => {
            if (String(chunk).includes('dev server:')) {
                clearTimeout(timer);
                resolve();
            }
        });
        child.on('exit', (code) => reject(new Error(`the dev server exited with ${code}`)));
    });
    child.stdout.resume();
    return { port, stop: () => child.kill('SIGTERM') };
}

// ── DevTools protocol ───────────────────────────────────────────────────────────────

/** One page target, driven over its own WebSocket. Console noise is collected, not lost. */
class Page {
    constructor(socket) {
        this.socket = socket;
        this.nextId = 1;
        this.pending = new Map();
        this.consoleErrors = [];
        socket.addEventListener('message', (event) => this.receive(JSON.parse(event.data)));
    }

    static async open(executable, url) {
        const port = await freePort();
        const browserArguments = [
            '--headless=new',
            `--remote-debugging-port=${port}`,
            '--remote-allow-origins=*',
            '--no-sandbox',
            '--disable-gpu',
            '--disable-dev-shm-usage',
            // The AudioWorklet renders in real time whether or not anything listens.
            // Without this the context stays suspended, because a synthetic click is
            // not a user gesture.
            '--autoplay-policy=no-user-gesture-required',
            // `--headless=new` is a full browser and DOES open the system audio device:
            // under WSLg that is PulseAudio forwarded to the Windows speakers, and the
            // owner heard the test corpus playing from a closed tab. Mute the output
            // (the worklet still renders and the telemetry still moves).
            '--mute-audio',
            '--user-data-dir=' + join(process.env.TMPDIR ?? '/tmp', `starplayer-cdp-${port}`),
            'about:blank',
        ];
        const browser = spawn(executable, browserArguments, { stdio: ['ignore', 'ignore', 'ignore'] });

        let version = null;
        for (let attempt = 0; attempt < 100 && version === null; attempt += 1) {
            await delay(100);
            version = await fetch(`http://127.0.0.1:${port}/json/version`).then((r) => r.json()).catch(() => null);
        }
        if (version === null) {
            browser.kill('SIGKILL');
            throw new Error('Chromium never opened its DevTools endpoint');
        }
        const target = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent('about:blank')}`, { method: 'PUT' })
            .then((response) => response.json());
        const socket = new WebSocket(target.webSocketDebuggerUrl);
        await new Promise((resolve, reject) => {
            socket.addEventListener('open', resolve, { once: true });
            socket.addEventListener('error', () => reject(new Error('could not attach to the page')), { once: true });
        });
        const page = new Page(socket);
        page.browser = browser;
        page.browserArguments = browserArguments;
        page.debugPort = port;
        page.targetId = target.id;
        page.version = version.Browser;
        page.profileDirectory = join(process.env.TMPDIR ?? '/tmp', `starplayer-cdp-${port}`);
        await page.send('Runtime.enable');
        await page.send('Log.enable');
        await page.send('Page.enable');
        await page.send('Page.addScriptToEvaluateOnNewDocument', { source: `
            window.mediaHandlers = {};
            if (navigator.mediaSession) {
                const original = navigator.mediaSession.setActionHandler.bind(navigator.mediaSession);
                navigator.mediaSession.setActionHandler = (action, callback) => { window.mediaHandlers[action] = callback; return original(action, callback); };
            }
            const OriginalContext = window.AudioContext;
            window.AudioContext = class extends OriginalContext {
                constructor(...options) { super(...options); window.testAudioContext = this; }
            };
        ` });
        await page.send('Page.navigate', { url });
        await page.waitFor('the document', "document.readyState === 'complete'");
        return page;
    }

    receive(message) {
        if (message.id !== undefined) {
            const pending = this.pending.get(message.id);
            this.pending.delete(message.id);
            if (message.error) pending?.reject(new Error(message.error.message));
            else pending?.resolve(message.result);
            return;
        }
        if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
            this.consoleErrors.push(message.params.args.map((argument) => argument.value ?? argument.description).join(' '));
        } else if (message.method === 'Runtime.exceptionThrown') {
            const details = message.params.exceptionDetails;
            this.consoleErrors.push(details.exception?.description ?? details.text);
        } else if (message.method === 'Log.entryAdded' && message.params.entry.level === 'error') {
            const entry = message.params.entry;
            // W8 deliberately probes this optional route on startup (also on reload).
            if (['/library-index.csv', '/library-comments.csv'].some(path => entry.url?.endsWith(path)) && entry.text.includes('404')) return;
            this.consoleErrors.push(entry.text);
        }
    }

    send(method, params = {}) {
        const id = this.nextId++;
        return new Promise((resolve, reject) => {
            this.pending.set(id, { resolve, reject });
            this.socket.send(JSON.stringify({ id, method, params }));
        });
    }

    /** Evaluate in the page and return the value, turning a page-side throw into ours. */
    async evaluate(expression) {
        const result = await this.send('Runtime.evaluate', {
            expression: `(async () => { ${expression} })()`,
            awaitPromise: true,
            returnByValue: true,
        });
        if (result.exceptionDetails) {
            throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
        }
        return result.result.value;
    }

    /// A condition evaluated against a page that may still be navigating: an expression
    /// that throws because the document is not there yet is simply "not true yet".
    async waitFor(description, expression, timeoutMilliseconds = 20_000) {
        const deadline = Date.now() + timeoutMilliseconds;
        for (;;) {
            const satisfied = await this.evaluate(`return Boolean(${expression});`).catch(() => false);
            if (satisfied) return;
            if (Date.now() > deadline) {
                const shown = await this.evaluate("return document.getElementById('message').textContent + ' | ' + document.getElementById('error').textContent + ' | playlist telemetry: ' + JSON.stringify(window.latestPlaylistSnapshot && { playing: window.latestPlaylistSnapshot.playing, frame: window.latestPlaylistSnapshot.songFrame, length: window.latestPlaylistSnapshot.songLengthFrames, generation: window.latestPlaylistSnapshot.moduleGeneration, progressMax: document.getElementById('progress').max });").catch((error) => `unreadable (${error.message})`);
                throw new Error(`timed out waiting for ${description}\n  page says: ${shown}\n  console: ${this.consoleErrors.join(' / ') || 'quiet'}`);
            }
            await delay(100);
        }
    }

    async close() {
        this.socket.close();
        this.browser.kill('SIGTERM');
        await delay(200);
        this.browser.kill('SIGKILL');
        await rm(this.profileDirectory, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
    }
}

// ── the page, as a human uses it ────────────────────────────────────────────────────

const READ_STATE = `
    const text = (id) => document.getElementById(id).textContent.trim();
    return {
        quantum: text('quantum'),
        memory: text('memory'),
        retired: text('retired'),
        health: text('health'),
        dropped: text('dropped'),
        commandTransport: text('command-transport'),
        telemetryTransport: text('telemetry-transport'),
        title: text('title'),
        moduleDetail: text('module-detail'),
        message: text('message'),
        order: text('order'),
        row: text('row'),
        speed: text('speed'),
        bpm: text('bpm'),
        chip: text('transport-chip'),
        effects: [...document.querySelectorAll('#channel-body .effect')].map((cell) => cell.textContent.trim()),
        patternRows: document.querySelectorAll('#pattern-table tbody tr:not([hidden])').length,
        soundingRow: document.querySelector('#pattern-table tr.sounding td')?.textContent.trim() ?? null,
        masterPeak: parseFloat(document.getElementById('master-peak').style.width) || 0,
        errorShown: !document.getElementById('error').hidden,
        errorText: text('error'),
        instruments: document.querySelectorAll('#instrument-list li').length,
        crossOriginIsolated: globalThis.crossOriginIsolated === true,
        archivePickerVisible: !document.getElementById('archive-picker').hidden,
        engineMode: text('engine-mode'),
        outputEngineMode: text('output-engine-mode'),
        liveInputStatus: text('live-input-status'),
        eventLead: text('event-lead'),
        keyboardOctave: text('keyboard-octave'),
        keyboardInstrument: text('keyboard-instrument'),
        midiEvents: text('midi-events'),
        outputStatus: text('output-status'),
        sampleRateStatus: text('sample-rate-status'),
        contextState: text('context-state'),
        baseLatency: text('base-latency'),
        outputLatency: text('output-latency'),
        channelStatus: text('channel-status'),
        maxChannelCount: text('max-channel-count'),
        sinkStatus: text('sink-status'),
        workletRate: text('worklet-rate'),
        pans: [...document.querySelectorAll('#channel-body tr')].map((row) => row.children[4]?.textContent.trim()).filter(Boolean),
        headphonePanning: document.getElementById('mod-headphone-panning').checked,
        headphonePanningDisabled: document.getElementById('mod-headphone-panning').disabled,
        // M10-W4: built from the wasm host's own \`enhancements_json()\`, so the count is
        // whatever the current build offers rather than a number hard-coded here.
        enhancementFlags: [...document.querySelectorAll('#enhancement-options input[type=checkbox]')]
            .reduce((flags, input) => flags | (input.checked ? (1 << Number(input.dataset.bit)) : 0), 0),
        enhancementCheckboxCount: document.querySelectorAll('#enhancement-options input[type=checkbox]').length,
        enhancementCheckboxesDisabled: [...document.querySelectorAll('#enhancement-options input[type=checkbox]')].every((input) => input.disabled),
        mutes: [...document.querySelectorAll('#channel-body button')].map((button) => button.textContent.trim()),
        elapsed: text('elapsed'),
        duration: text('duration'),
        progressValue: Number(document.getElementById('progress').value),
        progressMax: Number(document.getElementById('progress').max),
        progressDisabled: document.getElementById('progress').disabled,
        repeatChecked: document.getElementById('repeat').checked,
    };
`;

/// The Output panel formats rates with `toLocaleString`, so "22,050 Hz" and "22050 Hz" are
/// both correct depending on the browser's locale. Compare the digits, not the text.
function rateDigits(text) {
    return Number(text.replace(/[^0-9]/g, ''));
}

function orderNumber(text) {
    return Number(text.split('/')[0]);
}

async function loadFixture(page, name) {
    await page.evaluate(`
        document.getElementById('fixture-picker').value = ${JSON.stringify(name)};
        document.getElementById('load-fixture').click();
        return true;
    `);
}

async function loadFile(page, bytes, name, drop = false) {
    const base64 = Buffer.from(bytes).toString('base64');
    await page.evaluate(`
        const bytes = Uint8Array.from(atob(${JSON.stringify(base64)}), (character) => character.charCodeAt(0));
        const transfer = new DataTransfer();
        transfer.items.add(new File([bytes], ${JSON.stringify(name)}, { type: 'application/zip' }));
        if (${drop}) {
            document.getElementById('drop-zone').dispatchEvent(new DragEvent('drop', { dataTransfer: transfer, bubbles: true, cancelable: true }));
        } else {
            const picker = document.getElementById('file-picker');
            picker.files = transfer.files;
            picker.dispatchEvent(new Event('change', { bubbles: true }));
        }
        return true;
    `);
}

async function loadUrl(page, bytes) {
    const base64 = Buffer.from(bytes).toString('base64');
    await page.evaluate(`
        const bytes = Uint8Array.from(atob(${JSON.stringify(base64)}), (character) => character.charCodeAt(0));
        const url = URL.createObjectURL(new Blob([bytes], { type: 'application/zip' }));
        document.getElementById('url').value = url;
        document.getElementById('url-form').requestSubmit();
        return true;
    `);
}

async function run(executable, mode) {
    const isolate = mode !== 'plain';
    const server = await startDevServer(isolate);
    const page = await Page.open(executable, `http://127.0.0.1:${server.port}/`);
    const report = { mode };
    try {
        const firstModuleBytes = await readFile(join(DIST, 'modules', FIRST_MODULE));
        const secondModuleBytes = await readFile(join(DIST, 'modules', SECOND_MODULE));
        const singleModuleZip = buildZip([{ name: FIRST_MODULE, bytes: firstModuleBytes }]);
        const twoModuleZip = buildZip([
            { name: FIRST_MODULE, bytes: firstModuleBytes },
            { name: SECOND_MODULE, bytes: secondModuleBytes },
        ]);
        const modBytes = syntheticMod();
        await page.waitFor('the page module to attach its listeners', "document.documentElement.dataset.playerReady === 'true'");
        assert.equal(await page.evaluate("return document.getElementById('mod-headphone-panning').checked;"), false, 'the MOD preference is initially unchecked');
        const speedChooser = await page.evaluate(`
            const option = document.getElementById('speed-adjust');
            return { tag: option.tagName, multiple: option.multiple, value: option.value,
                options: [...option.options].map((entry) => [entry.value, entry.textContent]),
                defaults: [...option.options].filter((entry) => entry.defaultSelected).map((entry) => entry.value) };
        `);
        assert.equal(speedChooser.tag, 'SELECT', 'speed adjustment uses a native select');
        assert.equal(speedChooser.multiple, false, 'only one speed adjustment can be selected');
        assert.equal(speedChooser.value, '0', 'a fresh visit uses canonical playback');
        assert.deepEqual(speedChooser.defaults, ['0'], 'zero is the selected HTML default');
        assert.deepEqual(speedChooser.options, Array.from({ length: 13 }, (_, index) => {
            const value = index - 6;
            return [String(value), value === 0 ? '0 (default)' : value > 0 ? `+${value}` : String(value)];
        }), 'the chooser offers exactly -6 through +6, with explicit positive signs');
        await page.evaluate("localStorage.setItem('starplayer.output-and-mixer.v1', JSON.stringify({ path: 'float' })); return true;");
        await page.send('Page.reload', { ignoreCache: true });
        await page.waitFor('the reloaded page module', "document.documentElement.dataset.playerReady === 'true'");
        assert.equal(await page.evaluate("return document.getElementById('mod-headphone-panning').checked;"), false, 'an older v1 preference object restores as unchecked');
        assert.equal(await page.evaluate("return document.getElementById('speed-adjust').value;"), '0', 'an absent saved adjustment restores canonical playback');
        // Restore checks happen before audio starts, keeping each transport mode's
        // existing worklet setup and fallback choice intact.
        for (const [saved, expected] of [[3, '3'], [-8, '-6'], [-7, '-6'], [7, '6'], [8, '6'], ['invalid', '0']]) {
            await page.evaluate(`localStorage.setItem('starplayer.output-and-mixer.v1', JSON.stringify({ speedAdjust: ${JSON.stringify(saved)} })); return true;`);
            await page.send('Page.reload', { ignoreCache: true });
            await page.waitFor('the restored speed chooser', "document.documentElement.dataset.playerReady === 'true'");
            assert.equal(await page.evaluate("return document.getElementById('speed-adjust').value;"), expected, `saved adjustment ${saved} restores as ${expected}`);
        }

        // ── M10-W4: a persisted enhancement choice restores onto the checkboxes ─────
        //
        // The enhancement checkboxes do not exist until the worklet's first `ready`
        // message arrives (`enhancements_json()`), so the restore is only observable once
        // audio has started — unlike the panning checkbox above, which is static HTML.
        await page.evaluate("localStorage.setItem('starplayer.output-and-mixer.v1', JSON.stringify({ headphoneFriendlyModPanning: false, enhancementFlags: 1 })); return true;");
        // `restorePreferences()` runs once, at page load: the new record only takes effect
        // after another reload, exactly as the v1-shape check above needed one. This is
        // also where this run's real "Start audio" happens — a reload clears the
        // `force-fallback` checkbox along with everything else, so `mode === 'fallback'`
        // has to be reapplied here rather than after a second, separate reload.
        await page.send('Page.reload', { ignoreCache: true });
        await page.waitFor('the page reloaded for the enhancement restore check', "document.documentElement.dataset.playerReady === 'true'");
        if (mode === 'fallback') {
            await page.evaluate("document.getElementById('force-fallback').checked = true; return true;");
        }
        await page.evaluate("document.getElementById('start-audio').click(); return true;");
        await page.waitFor('the AudioWorklet', "document.getElementById('transport-chip').textContent === 'audio ready'");
        await page.waitFor('the enhancement checkboxes to be built', "document.querySelectorAll('#enhancement-options input[type=checkbox]').length > 0");
        const restoredEnhancementFlags = await page.evaluate(
            "return [...document.querySelectorAll('#enhancement-options input[type=checkbox]')].reduce((flags, input) => flags | (input.checked ? (1 << Number(input.dataset.bit)) : 0), 0);"
        );
        assert.equal(restoredEnhancementFlags, 1, 'a persisted enhancement choice restores onto the checkboxes once they exist');
        report.enhancementRestored = restoredEnhancementFlags;
        // Reset for the rest of the run: no module is loaded yet, so unchecking here only
        // persists the choice rather than reloading anything (`applyLoadOptions`'s
        // `state.currentModuleBytes === null` early-out).
        await page.evaluate(`
            for (const checkbox of document.querySelectorAll('#enhancement-options input[type=checkbox]')) {
                if (checkbox.checked) { checkbox.checked = false; checkbox.dispatchEvent(new Event('change')); }
            }
            return true;
        `);
        assert.equal(
            await page.evaluate("return [...document.querySelectorAll('#enhancement-options input[type=checkbox]')].every((input) => !input.checked);"),
            true,
            'the enhancement checkboxes reset for the rest of the run'
        );

        // Audio is already started above (the enhancement-restore check needed it); the
        // rest of the run continues from here.

        // ── headphone-friendly MOD panning ─────────────────────────────────────────
        await loadFile(page, modBytes, 'headphones.mod');
        await page.waitFor('the synthetic MOD', "document.getElementById('module-detail').textContent.includes('headphones.mod')");
        await page.waitFor('the MOD telemetry', "document.querySelectorAll('#channel-body tr').length === 4 && document.getElementById('row').textContent !== '\u2014'");
        const authenticMod = await page.evaluate(READ_STATE);
        assert.deepEqual(authenticMod.pans, ['LFT', 'RGT', 'RGT', 'LFT'], 'MOD defaults to authentic hard L-R-R-L panning');
        await page.evaluate("document.querySelector('#channel-body button').click(); return true;");
        await page.waitFor('the MOD channel mute', "document.querySelector('#channel-body button').textContent === 'Unmute'");
        await page.evaluate("document.getElementById('seek-order').value = '1'; document.getElementById('seek-order').dispatchEvent(new Event('change')); return true;");
        await page.waitFor('the second MOD order', "document.getElementById('order').textContent.startsWith('2/')");
        const beforeHeadphone = await page.evaluate(READ_STATE);
        assert.equal(beforeHeadphone.chip, 'playing');
        const disabledDuringReload = await page.evaluate("const option = document.getElementById('mod-headphone-panning'); option.checked = true; option.dispatchEvent(new Event('change')); return option.disabled;");
        assert.equal(disabledDuringReload, true, 'the panning option is disabled while its replacement is prepared');
        await page.waitFor('headphone panning to apply', "document.getElementById('mod-headphone-panning').disabled === false && document.getElementById('message').textContent.includes('60% spacing')");
        await page.waitFor('60% MOD telemetry', "[...document.querySelectorAll('#channel-body tr')].map((row) => row.children[4].textContent.trim()).join(',') === '3,C,C,3'");
        const headphoneMod = await page.evaluate(READ_STATE);
        assert.deepEqual(headphoneMod.pans, ['3', 'C', 'C', '3'], 'the option selects exact S3M-style 60% spacing');
        assert.equal(headphoneMod.chip, 'playing', 'a live panning reload preserves playing state');
        assert.match(headphoneMod.order, /^2\//, 'a live panning reload restores the sounding order');
        assert.equal(headphoneMod.mutes[0], 'Unmute', 'a live panning reload restores channel mute state');

        await page.evaluate("document.getElementById('stop').click(); return true;");
        await page.waitFor('the synthetic MOD to stop', "document.getElementById('transport-chip').textContent === 'stopped'");
        await page.evaluate("const option = document.getElementById('mod-headphone-panning'); option.checked = false; option.dispatchEvent(new Event('change')); return true;");
        await page.waitFor('the stopped panning reload', "document.getElementById('mod-headphone-panning').disabled === false && document.getElementById('transport-chip').textContent === 'stopped'");
        const stoppedReload = await page.evaluate(READ_STATE);
        assert.equal(stoppedReload.chip, 'stopped', 'a live panning reload preserves stopped state');
        assert.match(stoppedReload.order, /^2\//, 'the stopped reload also restores the sounding order');
        assert.equal(stoppedReload.mutes[0], 'Unmute', 'the stopped reload also restores channel mute state');
        // A stopped engine intentionally does not dispatch row-zero channel state. Resume
        // after proving Stop survived, then read the newly loaded header pans from telemetry.
        await page.evaluate("document.getElementById('play').click(); return true;");
        await page.waitFor('authentic panning to return', "[...document.querySelectorAll('#channel-body tr')].map((row) => row.children[4].textContent.trim()).join(',') === 'LFT,RGT,RGT,LFT'");
        const restoredMod = await page.evaluate(READ_STATE);
        assert.match(await page.evaluate("return localStorage.getItem('starplayer.output-and-mixer.v1');"), /"headphoneFriendlyModPanning":false/, 'the final panning choice is persisted');
        report.modPanning = `${authenticMod.pans.join('-')} → ${headphoneMod.pans.join('-')} → ${restoredMod.pans.join('-')}`;

        // ── the panning toggle must not race a concurrent load (C6a deliverable 4) ──
        //
        // The worklet port is a FIFO, so two `loadModule` messages in flight at once are
        // decided by posting order, not by which handler resolves first. Before the fix
        // the toggle reused `state.moduleRevision` instead of claiming one, and the load
        // that resolved second painted its metadata over the module that was sounding:
        // the page showed the six-channel module while the four-channel one played.
        //
        // The toggle is dispatched from inside the page, in the task right after the load
        // hands its bytes to the worklet, so the window does not depend on round-trip
        // timing. The six-channel module makes the mismatch observable: the channel table
        // is sized from telemetry, the module line from the page's own metadata.
        const raceModule = Buffer.from(syntheticMod({ channels: 6, tag: '6CHN', title: 'race test' })).toString('base64');
        const disabledDuringLoad = await page.evaluate(`
            const bytes = Uint8Array.from(atob(${JSON.stringify(raceModule)}), (character) => character.charCodeAt(0));
            const transfer = new DataTransfer();
            transfer.items.add(new File([bytes], 'race.mod', { type: 'application/octet-stream' }));
            const picker = document.getElementById('file-picker');
            const message = document.getElementById('message');
            const option = document.getElementById('mod-headphone-panning');

            // "Activating race.mod…" is written in the same task that posts the bytes to
            // the worklet, and a MutationObserver callback is a microtask at the end of
            // that task — so the toggle below lands after the post and before the reply
            // can be delivered, without depending on any timing at all.
            return await new Promise((resolve, reject) => {
                const timer = setTimeout(() => reject(new Error('the raced load never reached activation')), 15000);
                const observer = new MutationObserver(() => {
                    if (!message.textContent.includes('Activating race.mod')) return;
                    observer.disconnect();
                    clearTimeout(timer);
                    const wasDisabled = option.disabled;
                    option.checked = true;
                    option.dispatchEvent(new Event('change'));
                    resolve(wasDisabled);
                });
                observer.observe(message, { childList: true, characterData: true, subtree: true });
                picker.files = transfer.files;
                picker.dispatchEvent(new Event('change', { bubbles: true }));
            });
        `);
        assert.equal(disabledDuringLoad, true, 'the panning option is unavailable while a module load is in flight');
        await page.waitFor('the raced load and toggle to settle', "document.getElementById('mod-headphone-panning').disabled === false");
        // Channel pans only refresh when the reloaded module dispatches its next row,
        // so wait for telemetry that postdates the race rather than reading what the
        // pre-race module left behind. Both the fixed and the broken outcome change them.
        await page.waitFor('telemetry after the raced toggle', "[...document.querySelectorAll('#channel-body tr')].map((row) => row.children[4].textContent.trim()).join(',') !== 'LFT,RGT,RGT,LFT'");
        const raced = await page.evaluate(READ_STATE);
        const displayedChannels = Number(raced.moduleDetail.match(/(\d+) channels/)?.[1]);
        assert.equal(raced.pans.length, displayedChannels, `the sounding module has the ${displayedChannels} channels the page displays`);
        assert.match(raced.moduleDetail, /headphones\.mod/, 'the page shows the module that is actually sounding, not the one whose load lost the race');
        assert.equal(raced.headphonePanning, true, 'the toggle that won the race is the one the checkbox shows');
        assert.deepEqual(raced.pans, ['3', 'C', 'C', '3'], 'the sounding module carries the panning the checkbox claims');
        assert.equal(raced.errorShown, false, 'a raced toggle is not an error');
        report.panningRace = `ui ${displayedChannels}ch = audio ${raced.pans.length}ch, toggle ${raced.headphonePanning}`;

        // ── the experimental speed bias ────────────────────────────────────────────
        //
        // The module asks for speed 5 on every row 0. Canonically it plays at 5; biased by
        // +2 it plays at 7 — deliberately neither 5 nor the MOD's own initial 6, so the
        // reading cannot be satisfied by the bias being ignored *or* by it reaching the
        // initial speed, which it must not. Like every load option this rescans, so the
        // song length the slider draws is measured at the speed being played.
        await loadFile(page, syntheticMod({ title: 'speed test', speed: 5 }), 'speed.mod');
        await page.waitFor('the speed-command MOD', "document.getElementById('module-detail').textContent.includes('speed.mod')");
        await page.waitFor('its first telemetry frame', "document.getElementById('speed').textContent === '5'");
        const canonicalSpeed = await page.evaluate("return document.getElementById('speed').textContent;");
        await page.evaluate("const option = document.getElementById('speed-adjust'); option.value = '2'; option.dispatchEvent(new Event('change')); return true;");
        await page.waitFor('the biased reload', "document.getElementById('speed-adjust').disabled === false && document.getElementById('message').textContent.includes('Speed adjustment of +2 applied')");
        await page.waitFor('the biased speed to sound', "document.getElementById('speed').textContent === '7'");
        const biasedSpeed = await page.evaluate("return document.getElementById('speed').textContent;");
        assert.equal(canonicalSpeed, '5', 'the module asks for speed 5 and canonically gets it');
        assert.equal(biasedSpeed, '7', 'a +2 adjustment makes every requested speed two ticks longer');
        assert.match(
            await page.evaluate("return localStorage.getItem('starplayer.output-and-mixer.v1');"),
            /"speedAdjust":2/,
            'the speed adjustment is persisted'
        );
        assert.match(
            await page.evaluate("return document.getElementById('load-options-summary').textContent;"),
            /speed \+2/,
            'a non-default option is named in the closed disclosure summary'
        );
        const speedDisabledDuringReload = await page.evaluate("const option = document.getElementById('speed-adjust'); option.value = '-6'; option.dispatchEvent(new Event('change')); return option.disabled;");
        assert.equal(speedDisabledDuringReload, true, 'the speed chooser is unavailable during a load');
        await page.waitFor('the negative adjustment to apply', "document.getElementById('speed-adjust').disabled === false && document.getElementById('message').textContent.includes('Speed adjustment of -6 applied')");
        await page.waitFor('the clamped speed to sound', "document.getElementById('speed').textContent === '1' && document.getElementById('transport-chip').textContent === 'playing'");
        const negativeSpeed = await page.evaluate("return document.getElementById('speed').textContent;");
        const negativePosition = await page.evaluate("return document.getElementById('order').textContent + ':' + document.getElementById('row').textContent;");
        await page.waitFor('playback to advance at the one-tick minimum', `document.getElementById('transport-chip').textContent === 'playing' && document.getElementById('speed').textContent === '1' && (document.getElementById('order').textContent + ':' + document.getElementById('row').textContent) !== ${JSON.stringify(negativePosition)}`);
        assert.equal(negativeSpeed, '1', 'a -6 adjustment to speed 5 clamps at one tick per row');
        assert.match(await page.evaluate("return localStorage.getItem('starplayer.output-and-mixer.v1');"), /"speedAdjust":-6/, 'the negative adjustment is persisted');
        assert.match(await page.evaluate("return document.getElementById('load-options-summary').textContent;"), /speed -6/, 'the summary names the negative adjustment');
        await page.evaluate("const option = document.getElementById('speed-adjust'); option.value = '0'; option.dispatchEvent(new Event('change')); return true;");
        await page.waitFor('the canonical reload', "document.getElementById('speed-adjust').disabled === false && document.getElementById('speed').textContent === '5'");
        report.speedAdjust = `${canonicalSpeed} \u2192 ${biasedSpeed} \u2192 ${negativeSpeed} (advancing) \u2192 ${await page.evaluate("return document.getElementById('speed').textContent;")}`;

        await loadFixture(page, FIRST_MODULE);
        await page.waitFor('the first module', "document.getElementById('title').textContent !== 'No module loaded'");
        await page.waitFor('the first telemetry frame', "document.getElementById('row').textContent !== '\\u2014'");

        const started = await page.evaluate(READ_STATE);
        assert.equal(started.crossOriginIsolated, isolate, 'the document isolation matches the server');
        assert.equal(started.quantum, '128 frames', 'the host block size is the engine render quantum');
        if (mode === 'sab') {
            assert.match(started.commandTransport, /SharedArrayBuffer/);
            assert.match(started.telemetryTransport, /SharedArrayBuffer/);
        } else {
            assert.match(started.commandTransport, /postMessage/);
            assert.match(started.telemetryTransport, /postMessage/);
        }
        assert.ok(started.instruments > 0, 'the instrument list is populated');
        report.transport = `${started.commandTransport} / ${started.telemetryTransport}`;

        // ── progress slider: the song timeline reaches the page ────────────────────
        assert.notEqual(started.duration, '0:00', 'the duration reads a nonzero song length');
        assert.notEqual(started.duration, '--:--', 'the duration is known for a bundled S3M');
        assert.ok(started.progressMax > 0, 'the progress slider max is the song length in frames');
        assert.equal(started.progressDisabled, false, 'the slider is enabled once the song length is known');
        assert.equal(started.repeatChecked, true, 'Repeat defaults to checked');

        // ── play ────────────────────────────────────────────────────────────────────
        const seconds = options.seconds;
        const rowsSeen = new Set();
        let peakSeen = 0;
        let ordersSeen = new Set();
        let effectsSeen = new Set();
        const baselineMemory = started.memory;
        // `#row` reads `row:tick`, so its distinct values saturate at a few hundred on a
        // long run. What proves the display is live is how often it *changes*, not how
        // many values it has taken.
        const samplesPerSecond = 4;
        let rowChanges = 0;
        let previousRow = started.row;
        for (let sample = 0; sample < seconds * samplesPerSecond; sample += 1) {
            await delay(1000 / samplesPerSecond);
            const state = await page.evaluate(READ_STATE);
            if (state.row !== previousRow) rowChanges += 1;
            previousRow = state.row;
            rowsSeen.add(state.row);
            ordersSeen.add(state.order);
            peakSeen = Math.max(peakSeen, state.masterPeak);
            for (const effect of state.effects) if (effect !== '—' && effect !== '') effectsSeen.add(effect);
            const elapsed = (sample / samplesPerSecond).toFixed(1);
            assert.equal(state.memory, baselineMemory, `wasm memory changed during playback at ${elapsed}s: ${state.memory}`);
            assert.ok(state.memory.includes('stable'), `wasm memory grew during playback: ${state.memory}`);
            assert.equal(state.errorShown, false, `an error appeared during playback: ${state.errorText}`);
        }
        const playing = await page.evaluate(READ_STATE);
        assert.ok(rowChanges >= seconds, `the telemetry row kept advancing (${rowChanges} changes over ${seconds}s)`);
        assert.ok(rowsSeen.size > 10, `and moved through the pattern (${rowsSeen.size} distinct values)`);
        assert.ok(peakSeen > 0, 'the master peak meter moved above zero');
        assert.equal(playing.chip, 'playing');
        assert.equal(playing.health, 'healthy');
        assert.ok(playing.patternRows > 0, 'the pattern window is drawn');
        assert.ok(playing.soundingRow !== null, 'a pattern row is highlighted as sounding');
        assert.ok(effectsSeen.size > 0, 'at least one effect was spelled out in English');
        assert.notEqual(playing.elapsed, started.elapsed, 'the elapsed readout advanced while playing');
        assert.ok(playing.progressValue > 0, 'the progress slider advanced with playback');
        // ── clicking a time readout toggles elapsed ↔ remaining, as in VLC ─────────
        await page.evaluate("document.getElementById('elapsed').click(); return true;");
        const remaining = await page.evaluate(READ_STATE);
        assert.ok(remaining.elapsed.startsWith('-'), `the readout shows remaining time after a click: ${remaining.elapsed}`);
        assert.equal(await page.evaluate("return localStorage.getItem('starplayer.time-display.v1');"), 'remaining', 'the choice is remembered');
        await page.evaluate("document.getElementById('duration').click(); return true;");
        const elapsedAgain = await page.evaluate(READ_STATE);
        assert.ok(!elapsedAgain.elapsed.startsWith('-'), `a second click on either readout goes back to elapsed: ${elapsedAgain.elapsed}`);
        report.playedSeconds = seconds;
        report.rowChanges = rowChanges;
        report.distinctRows = rowsSeen.size;
        report.distinctOrders = ordersSeen.size;
        report.peak = peakSeen;
        report.effectSample = [...effectsSeen].slice(0, 6);
        report.memory = baselineMemory;
        report.title = playing.title;

        // ── insert effects (M7-H7): a reverb on channel 1, installed through the page ──
        //
        // The target and effect selects are driven exactly as a person would use them —
        // by name, not by the wire number `InsertKind::ALL` happens to give reverb — so a
        // reordering of the effect list would fail this test rather than silently install
        // the wrong effect.
        await page.evaluate(`
            const targetSelect = document.getElementById('effects-target');
            targetSelect.value = '0';
            targetSelect.dispatchEvent(new Event('change'));
            return true;
        `);
        const slotCount = await page.evaluate("return document.querySelectorAll('#effects-slots .effects-slot').length;");
        assert.equal(slotCount, 4, 'the four insert slot rows are built');
        const reverbSelected = await page.evaluate(`
            const slot = document.querySelectorAll('#effects-slots .effects-slot')[0];
            const select = slot.querySelector('select');
            const option = [...select.options].find((candidate) => candidate.textContent === 'reverb');
            if (!option) return false;
            select.value = option.value;
            select.dispatchEvent(new Event('change'));
            return select.value === option.value;
        `);
        assert.equal(reverbSelected, true, 'reverb is one of the effect select\'s options and was chosen');
        await page.waitFor(
            'the reverb’s parameters to appear (the panel installs optimistically, before the worklet confirms)',
            "document.querySelectorAll('#effects-slots .effects-slot')[0].querySelectorAll('.effects-param').length > 0",
        );
        assert.equal(await page.evaluate(READ_STATE).then((state) => state.errorShown), false, 'installing the reverb raised no page error');

        let insertPeakSeen = 0;
        for (let sample = 0; sample < 2 * samplesPerSecond; sample += 1) {
            await delay(1000 / samplesPerSecond);
            const state = await page.evaluate(READ_STATE);
            insertPeakSeen = Math.max(insertPeakSeen, state.masterPeak);
            assert.equal(state.errorShown, false, `an error appeared after installing the reverb: ${state.errorText}`);
        }
        assert.ok(insertPeakSeen > 0, 'the audio peak did not drop to zero after installing a reverb on channel 1');
        report.insertEffectPeak = insertPeakSeen;

        // Removing it the same way proves the None option round-trips too, and leaves the
        // graph in the state the rest of this run expects.
        const reverbRemoved = await page.evaluate(`
            const slot = document.querySelectorAll('#effects-slots .effects-slot')[0];
            const select = slot.querySelector('select');
            select.value = '';
            select.dispatchEvent(new Event('change'));
            return select.value === '';
        `);
        assert.equal(reverbRemoved, true, 'choosing None removes the installed effect');
        await page.waitFor(
            'the reverb’s parameters to disappear',
            "document.querySelectorAll('#effects-slots .effects-slot')[0].querySelectorAll('.effects-param').length === 0",
        );

        // ── transport ───────────────────────────────────────────────────────────────
        await page.evaluate("document.getElementById('stop').click(); return true;");
        await page.waitFor('the transport to stop', "document.getElementById('transport-chip').textContent === 'stopped'");
        await delay(500);
        const stopped = await page.evaluate(READ_STATE);
        const stillStopped = await page.evaluate(READ_STATE);
        assert.equal(stopped.row, stillStopped.row, 'a stopped transport freezes the row');
        await page.evaluate("document.getElementById('seek-order').value = '1'; document.getElementById('seek-order').dispatchEvent(new Event('change')); return true;");
        await page.evaluate("document.getElementById('play').click(); return true;");
        await page.waitFor('playback to resume', "document.getElementById('transport-chip').textContent === 'playing'");
        await delay(1500);
        const resumed = await page.evaluate(READ_STATE);
        assert.notEqual(resumed.row, stopped.row, 'play resumes the row clock');
        report.seekedOrder = resumed.order;

        // ── the progress slider seeks the engine, not just the display ──────────────
        const beforeSliderSeek = await page.evaluate(READ_STATE);
        const sliderSeekTarget = Math.floor(beforeSliderSeek.progressMax / 2);
        await page.evaluate(`
            const progress = document.getElementById('progress');
            progress.value = '${sliderSeekTarget}';
            progress.dispatchEvent(new Event('change'));
            return true;
        `);
        await page.waitFor('the slider seek to move the order', `document.getElementById('order').textContent !== ${JSON.stringify(beforeSliderSeek.order)}`);
        const afterSliderSeek = await page.evaluate(READ_STATE);
        await delay(500);
        const settledSliderSeek = await page.evaluate(READ_STATE);
        assert.equal(afterSliderSeek.order, settledSliderSeek.order, 'the order the page shows settles at the frame the slider was seeked to');
        report.sliderSeek = `${beforeSliderSeek.order} → ${settledSliderSeek.order} at frame ${sliderSeekTarget}/${beforeSliderSeek.progressMax}`;

        // ── Repeat off stops at the end of a song that ends, resetting to 0:00 ──────
        // REFLEX's order list runs out; nothing in it jumps back into music already
        // played, so task D2 reads it as an *end* rather than a loop. With Repeat off the
        // host stops it on its end frame instead of playing a second pass under a fade,
        // and the slider therefore still covers exactly one pass — the fade is added to
        // the displayed length only for a song that really loops (`app.js`,
        // `updateProgress`). This assertion used to demand the five-second growth, which
        // was right before D2 and has been stale since.
        await page.evaluate("const repeat = document.getElementById('repeat'); repeat.checked = false; repeat.dispatchEvent(new Event('change')); return true;");
        const rate = rateDigits(settledSliderSeek.workletRate);
        await delay(500);
        const repeatOff = await page.evaluate(READ_STATE);
        assert.equal(repeatOff.progressMax, settledSliderSeek.progressMax, 'a song that ends keeps a one-pass slider with Repeat off');
        assert.equal(repeatOff.duration, settledSliderSeek.duration, 'and the displayed length with it');
        const nearEndFrame = Math.max(0, settledSliderSeek.progressMax - rate);
        await page.evaluate(`
            const progress = document.getElementById('progress');
            progress.value = '${nearEndFrame}';
            progress.dispatchEvent(new Event('change'));
            return true;
        `);
        await page.waitFor('the transport to stop at the end of the song', "document.getElementById('transport-chip').textContent === 'stopped'", 30_000);
        const afterFadeStop = await page.evaluate(READ_STATE);
        assert.equal(afterFadeStop.chip, 'stopped', 'Repeat off stops at the end of the song instead of wrapping');
        assert.equal(afterFadeStop.elapsed, '0:00', 'elapsed reads 0:00 once the end-of-song stop lands');
        assert.equal(afterFadeStop.progressValue, 0, 'the slider rewinds to 0 once the end-of-song stop lands');
        report.endStop = `stopped at ${afterFadeStop.elapsed}`;

        // Leave Repeat checked and the transport playing again, as the rest of this run
        // (and the next mode's run of the same page) expects.
        await page.evaluate("const repeat = document.getElementById('repeat'); repeat.checked = true; repeat.dispatchEvent(new Event('change')); return true;");
        await page.evaluate("document.getElementById('play').click(); return true;");
        await page.waitFor('playback to resume after the fade-out test', "document.getElementById('transport-chip').textContent === 'playing'");

        // ── live input: the tracker-style keyboard (task E6) ────────────────────────
        //
        // A real key event through the DevTools protocol, so the focus rules, the
        // `KeyboardEvent.code` map and the ring opcode are all exercised the way a person
        // exercises them. Jam mode keeps the module playing while the MIDI channel appears
        // beside it in the channel table.
        await page.evaluate("const keyboard = document.getElementById('keyboard-enable'); keyboard.checked = true; keyboard.dispatchEvent(new Event('change')); return true;");
        await page.waitFor('live input to install', "document.getElementById('live-input-status').textContent !== 'off'");
        const armed = await page.evaluate(READ_STATE);
        assert.match(armed.eventLead, /^lead 256 frames \(\d+\.\d ms\)/, `the lead is reported: ${armed.eventLead}`);
        if (!isolate || mode === 'fallback') {
            assert.match(armed.eventLead, /batched/, 'the fallback transport reports its extra animation-frame latency');
        }
        await delay(400);
        assert.ok((await page.evaluate(READ_STATE)).masterPeak > 0, 'the module keeps playing in jam mode');

        // Typing in a field is typing, never playing.
        await page.evaluate("document.getElementById('url').focus(); return true;");
        await pressKey(page, 'KeyZ', 'z', 90);
        await delay(200);
        assert.equal((await page.evaluate(READ_STATE)).midiEvents, '0 sent', 'a key pressed inside a text field plays nothing');
        await page.evaluate("document.getElementById('url').blur(); return true;");

        // `]` up an octave, `=` to the next instrument, then a held Z.
        await pressKey(page, 'BracketRight', ']', 221);
        await pressKey(page, 'Equal', '=', 187);
        const stepped = await page.evaluate(READ_STATE);
        assert.equal(stepped.keyboardOctave, '5', 'the octave stepped up');
        assert.equal(stepped.keyboardInstrument, '2', 'and the instrument stepped on');
        await pressKey(page, 'BracketLeft', '[', 219);

        await page.send('Input.dispatchKeyEvent', { type: 'keyDown', code: 'KeyZ', key: 'z', windowsVirtualKeyCode: 90, nativeVirtualKeyCode: 90 });
        let notePeak = 0;
        for (let attempt = 0; attempt < 40 && notePeak === 0; attempt += 1) {
            await delay(50);
            notePeak = (await page.evaluate(READ_STATE)).masterPeak;
        }
        await page.send('Input.dispatchKeyEvent', { type: 'keyUp', code: 'KeyZ', key: 'z', windowsVirtualKeyCode: 90, nativeVirtualKeyCode: 90 });
        assert.ok(notePeak > 0, 'the keyboard sounded a note beside the playing module');
        const played = await page.evaluate(READ_STATE);
        assert.match(played.midiEvents, /^[1-9]\d* sent$/, `events reached the ring: ${played.midiEvents}`);
        report.liveInput = `${played.liveInputStatus}; ${played.eventLead}; ${played.midiEvents}; peak ${notePeak}`;

        // The note-off the key-up queued is still crossing to the worklet. Tearing the
        // live-input source down under it would leave a stale record to refuse — see the
        // toggle's own handling of held keys in `app.js` — so let the ring drain first.
        await delay(250);
        await page.evaluate("const keyboard = document.getElementById('keyboard-enable'); keyboard.checked = false; keyboard.dispatchEvent(new Event('change')); return true;");
        await page.waitFor('live input to go off', "document.getElementById('live-input-status').textContent === 'off'");
        await page.waitFor('the module to play again', "parseFloat(document.getElementById('master-peak').style.width) > 0", 30_000);
        assert.equal((await page.evaluate(READ_STATE)).errorShown, false, 'turning live input on and off is not an error');

        // ── a single-module ZIP loads straight through the file picker ──────────────
        await loadFile(page, singleModuleZip, 'reflex.zip');
        await page.waitFor('the single ZIP module', "document.getElementById('module-detail').textContent.includes('REFLEX.S3M (from reflex.zip)')");
        const singleZipState = await page.evaluate(READ_STATE);
        assert.equal(singleZipState.archivePickerVisible, false, 'a single-module ZIP does not open the picker');
        assert.equal(singleZipState.chip, 'playing');
        assert.match(singleZipState.moduleDetail, /REFLEX\.S3M \(from reflex\.zip\)/);
        report.singleZipLabel = singleZipState.moduleDetail.split(' · ')[0];

        // ── a multi-module ZIP keeps playing until its second entry is chosen ───────
        const beforePicker = await page.evaluate(READ_STATE);
        await loadFile(page, twoModuleZip, 'two-modules.zip');
        await page.waitFor('the archive picker', "document.getElementById('archive-picker').hidden === false");
        const pickerState = await page.evaluate(READ_STATE);
        assert.equal(pickerState.title, beforePicker.title, 'opening the picker does not replace the current module');
        assert.equal(pickerState.chip, 'playing', 'the current module keeps playing behind the picker');
        const pickerEntries = await page.evaluate("return [...document.getElementById('archive-entries').options].map((option) => option.textContent);");
        assert.equal(pickerEntries.length, 2);
        assert.match(pickerEntries[1], /PETRI\.S3M/);
        await page.waitFor('playback to advance behind the picker', `document.getElementById('row').textContent !== ${JSON.stringify(pickerState.row)}`);
        await page.evaluate("document.getElementById('archive-cancel').click(); return true;");
        const afterCancel = await page.evaluate(READ_STATE);
        assert.equal(afterCancel.archivePickerVisible, false);
        assert.equal(afterCancel.moduleDetail, beforePicker.moduleDetail, 'Cancel leaves the current module unchanged');
        await loadFile(page, twoModuleZip, 'two-modules.zip');
        await page.waitFor('the reopened archive picker', "document.getElementById('archive-picker').hidden === false");
        await page.evaluate("document.getElementById('archive-entries').selectedIndex = 1; document.getElementById('archive-load').click(); return true;");
        await page.waitFor('the selected second ZIP module', "document.getElementById('module-detail').textContent.includes('PETRI.S3M (from two-modules.zip)')");
        await page.waitFor('the retired module to be collected', "parseInt(document.getElementById('retired').textContent, 10) >= 1");
        await delay(2000);
        const swapped = await page.evaluate(READ_STATE);
        assert.equal(swapped.errorShown, false, `the ZIP hot swap raised an error: ${swapped.errorText}`);
        assert.equal(swapped.health, 'healthy');
        report.pickerEntries = pickerEntries;
        report.multiZipLabel = swapped.moduleDetail.split(' · ')[0];
        report.retiredAfterSwap = swapped.retired;
        report.secondTitle = swapped.title;

        // ── load-time sample enhancement checkboxes (M10-W4) ────────────────────────
        //
        // PETRI.S3M (swapped in above) is loaded and playing here — a non-MOD format,
        // which proves the enhancement checkboxes are not MOD-specific like the panning
        // option — and large enough (K5a's research measured it at 63 KB plain, 250 KB
        // under sinc4x) that the wasm memory line reliably moves when the upsampler runs.
        await page.waitFor('the enhancement checkboxes to be built', "document.querySelectorAll('#enhancement-options input[type=checkbox]').length > 0");
        const beforeEnhance = await page.evaluate(READ_STATE);
        assert.equal(beforeEnhance.enhancementFlags, 0, 'enhancements default to off');
        assert.ok(beforeEnhance.enhancementCheckboxCount > 0, 'the load panel built at least one enhancement checkbox from enhancements_json()');

        const toggleEnhancementCheckbox = async (index, checked) => page.evaluate(`
            const checkboxes = [...document.querySelectorAll('#enhancement-options input[type=checkbox]')];
            const target = checkboxes[${index}];
            target.checked = ${checked};
            target.dispatchEvent(new Event('change'));
            return target.disabled;
        `);

        // Each checkbox toggles independently: on, wait for its own reload, then off again.
        for (let index = 0; index < beforeEnhance.enhancementCheckboxCount; index += 1) {
            const bit = await page.evaluate(`return Number(document.querySelectorAll('#enhancement-options input[type=checkbox]')[${index}].dataset.bit);`);

            const disabledDuringOn = await toggleEnhancementCheckbox(index, true);
            assert.equal(disabledDuringOn, true, `checkbox ${index} is disabled while its own reload is in flight`);
            await page.waitFor(`every enhancement checkbox re-enabled after enabling checkbox ${index}`, "[...document.querySelectorAll('#enhancement-options input[type=checkbox]')].every((input) => !input.disabled)");
            await page.waitFor('the enhanced module to resume playing', "document.getElementById('transport-chip').textContent === 'playing'");
            const onState = await page.evaluate(READ_STATE);
            assert.equal(onState.errorShown, false, `enabling checkbox ${index} raised an error: ${onState.errorText}`);
            assert.equal(onState.enhancementFlags & (1 << bit), 1 << bit, `checkbox ${index}'s bit is set once its own reload lands`);

            const disabledDuringOff = await toggleEnhancementCheckbox(index, false);
            assert.equal(disabledDuringOff, true, `checkbox ${index} is disabled while turning it back off`);
            await page.waitFor(`every enhancement checkbox re-enabled after disabling checkbox ${index}`, "[...document.querySelectorAll('#enhancement-options input[type=checkbox]')].every((input) => !input.disabled)");
            await page.waitFor('the un-enhanced module to resume playing', "document.getElementById('transport-chip').textContent === 'playing'");
            const offState = await page.evaluate(READ_STATE);
            assert.equal(offState.enhancementFlags & (1 << bit), 0, `checkbox ${index}'s bit clears once its own reload lands`);
        }

        // Re-enable the sinc upsampler (bit 0) alone, and check the memory line: a reload
        // is exactly where memory is *allowed* to move (the rebuild is a real allocation
        // outside process(), like any other load), and it must then hold steady through
        // ordinary playback, exactly like every other reload in this file. Whether it
        // visibly moves depends on how much of `HEAP_RESERVE_BYTES`'s slack earlier loads
        // in this run already used — PETRI's own +187 KB (K5a's research measurement) is
        // small next to a 16 MiB reservation, so this reports the observation rather than
        // asserting a specific direction; "GREW" appearing on the *load* itself would be
        // wrong regardless, since only a page count freshly settled by `bindViews()` after
        // activation should ever be reported at that instant.
        const sincIndex = await page.evaluate(
            "return [...document.querySelectorAll('#enhancement-options input[type=checkbox]')].findIndex((input) => input.dataset.bit === '0');"
        );
        assert.ok(sincIndex >= 0, 'the sinc upsampler checkbox exists at bit 0');
        await toggleEnhancementCheckbox(sincIndex, true);
        await page.waitFor('the sinc4x reload to settle', "[...document.querySelectorAll('#enhancement-options input[type=checkbox]')].every((input) => !input.disabled)");
        await page.waitFor('the enhanced module to resume playing', "document.getElementById('transport-chip').textContent === 'playing'");
        const afterSinc = await page.evaluate(READ_STATE);
        report.enhancementMemory = afterSinc.memory === beforeEnhance.memory
            ? `${beforeEnhance.memory} (unchanged: existing heap headroom absorbed the rebuild)`
            : `${beforeEnhance.memory} -> ${afterSinc.memory}`;

        await delay(2000);
        const afterSincPlaying = await page.evaluate(READ_STATE);
        assert.equal(afterSincPlaying.memory, afterSinc.memory, `wasm memory changed during playback after enabling sinc4x: ${afterSincPlaying.memory}`);
        assert.ok(afterSincPlaying.memory.includes('stable'), `wasm memory grew during playback after enabling sinc4x: ${afterSincPlaying.memory}`);

        const storedEnhance = await page.evaluate("return localStorage.getItem('starplayer.output-and-mixer.v1');");
        assert.match(storedEnhance, /"enhancementFlags":1/, 'the enhancement choice is persisted alongside the existing v1 record');
        assert.match(storedEnhance, /"headphoneFriendlyModPanning":/, 'the v1 record keeps its existing key');

        // Turn it back off so the rest of the run — which assumes an un-enhanced module
        // for its own memory and timing assertions — is unaffected.
        await toggleEnhancementCheckbox(sincIndex, false);
        await page.waitFor('the sinc4x-off reload to settle', "[...document.querySelectorAll('#enhancement-options input[type=checkbox]')].every((input) => !input.disabled)");
        await page.waitFor('the module to resume playing without the enhancer', "document.getElementById('transport-chip').textContent === 'playing'");
        const afterSincOff = await page.evaluate(READ_STATE);
        assert.equal(afterSincOff.enhancementFlags, 0, 'the sinc4x checkbox reload turned it back off');

        // ── the panning toggle's race guard generalised to `applyLoadOptions` ────────
        //
        // The concurrent-load race test above (`panningRace`) already exercises this same
        // shared function through the panning checkbox; nothing about the race depends on
        // which load option changed, so it is not duplicated per checkbox here.

        // ── the retained ZIP keeps its other tracks one click away ──────────────────
        const trackPicker = await page.evaluate(`
            const select = document.getElementById('archive-tracks');
            return {
                hidden: select.hidden,
                buttonHidden: document.getElementById('load-archive-track').hidden,
                options: [...select.options].map((option) => option.textContent),
                selected: select.selectedOptions[0].textContent,
            };
        `);
        assert.equal(trackPicker.hidden, false, 'the retained-archive dropdown stays on the page');
        assert.equal(trackPicker.buttonHidden, false, 'the retained-archive load button stays on the page');
        assert.match(trackPicker.options[0], /two-modules\.zip/, 'the placeholder names the retained archive');
        assert.deepEqual(trackPicker.options.slice(1), pickerEntries, 'the dropdown lists what the modal listed');
        assert.match(trackPicker.selected, /PETRI\.S3M/, 'the playing entry is selected in the dropdown');

        await page.evaluate("document.getElementById('archive-tracks').value = '0'; document.getElementById('load-archive-track').click(); return true;");
        await page.waitFor('the other ZIP track, loaded from the dropdown', "document.getElementById('module-detail').textContent.includes('REFLEX.S3M (from two-modules.zip)')");
        const fromDropdown = await page.evaluate(READ_STATE);
        assert.equal(fromDropdown.archivePickerVisible, false, 'the dropdown loads without reopening the modal');
        assert.equal(fromDropdown.errorShown, false, `the dropdown load raised an error: ${fromDropdown.errorText}`);
        assert.equal(fromDropdown.chip, 'playing');
        report.archiveDropdownLabel = fromDropdown.moduleDetail.split(' · ')[0];

        // A plain module leaves the retained ZIP alone; another ZIP replaces it.
        await loadFixture(page, SECOND_MODULE);
        await page.waitFor('the bundled fixture over the ZIP track', `document.getElementById('module-detail').textContent.includes(${JSON.stringify(SECOND_MODULE)}) && !document.getElementById('module-detail').textContent.includes('(from ')`);
        assert.equal(await page.evaluate("return document.getElementById('archive-tracks').hidden;"), false, 'a plain module leaves the retained ZIP in the load panel');
        await loadFile(page, singleModuleZip, 'one-module.zip');
        await page.waitFor('the replacing single-module ZIP', "document.getElementById('module-detail').textContent.includes('REFLEX.S3M (from one-module.zip)')");
        const replacedPicker = await page.evaluate("return [...document.getElementById('archive-tracks').options].map((option) => option.textContent);");
        assert.equal(replacedPicker.length, 2, 'the replacing ZIP lists its one module behind the placeholder');
        assert.match(replacedPicker[0], /one-module\.zip/);
        assert.match(replacedPicker[1], /REFLEX\.S3M/);

        // Exercise the same archive front half through drag-and-drop as a third route.
        await loadFile(page, singleModuleZip, 'dropped.zip', true);
        await page.waitFor('the dropped ZIP module', "document.getElementById('module-detail').textContent.includes('REFLEX.S3M (from dropped.zip)')");
        const droppedZip = await page.evaluate(READ_STATE);
        assert.equal(droppedZip.chip, 'playing');
        report.droppedZipLabel = droppedZip.moduleDetail.split(' · ')[0];

        // URL loading uses the same byte path; a blob URL gives the harness a local ZIP
        // URL without writing a test fixture beside the generated distribution.
        await loadUrl(page, singleModuleZip);
        await page.waitFor('the ZIP URL module', "document.getElementById('module-detail').textContent.includes('REFLEX.S3M (from blob:')");
        const urlZip = await page.evaluate(READ_STATE);
        assert.equal(urlZip.chip, 'playing');
        report.zipUrlLoaded = true;

        // ── a deliberately broken file, dropped on the page ─────────────────────────
        await page.evaluate(`
            const bytes = new Uint8Array(64).fill(0x41);
            const transfer = new DataTransfer();
            transfer.items.add(new File([bytes], 'broken.s3m', { type: 'application/octet-stream' }));
            document.getElementById('drop-zone').dispatchEvent(new DragEvent('drop', { dataTransfer: transfer, bubbles: true, cancelable: true }));
            return true;
        `);
        await page.waitFor('a readable error', "document.getElementById('error').hidden === false");
        const afterBadFile = await page.evaluate(READ_STATE);
        assert.match(afterBadFile.errorText, /broken\.s3m/, 'the error names the file the user dropped');
        assert.ok(afterBadFile.errorText.length > 20, `the error is a sentence, not a code: ${afterBadFile.errorText}`);
        assert.equal(afterBadFile.title, urlZip.title, 'the good module is still loaded');
        report.badFileError = afterBadFile.errorText;

        const beforeSurvival = await page.evaluate(READ_STATE);
        await delay(2000);
        const afterSurvival = await page.evaluate(READ_STATE);
        assert.notEqual(afterSurvival.row, beforeSurvival.row, 'playback continued through the failed load');
        assert.equal(afterSurvival.memory, baselineMemory, 'a failed load did not grow wasm memory');

        // ── a rebuilt AudioContext at another sample rate ───────────────────────────
        // Changing the rate is a whole-graph rebuild: a new AudioContext, a new worklet
        // node, the module reloaded from the bytes the page retained, and the transport
        // seeked back to the order that was sounding. Chromium honours every rate in the
        // menu; Firefox may throw NotSupportedError, which the page has to report while
        // keeping the old context alive. Both outcomes pass here, and the report says which.
        // The broken-file banner above is still on screen and stays there: nothing here
        // clears it, so these steps assert the banner never says anything *new*.
        const beforeRate = await page.evaluate(READ_STATE);
        const rateBefore = rateDigits(beforeRate.workletRate);
        const orderBefore = orderNumber(beforeRate.order);
        assert.equal(beforeRate.engineMode, 'float · linear · 32-bit float · stereo', 'the default mixer mode is reported before anything is changed');
        await page.evaluate(`
            const select = document.getElementById('sample-rate');
            select.value = '22050';
            select.dispatchEvent(new Event('change'));
            document.getElementById('apply-sample-rate').click();
            return true;
        `);
        await page.waitFor('the 22050 Hz request to be reported', "document.getElementById('sample-rate-status').textContent.replace(/[^0-9]/g, '').startsWith('22050')");
        await page.waitFor('telemetry from the rebuilt graph', `document.getElementById('row').textContent !== ${JSON.stringify(beforeRate.row)}`);
        await delay(2000);
        const rebuilt = await page.evaluate(READ_STATE);
        const rateAfter = rateDigits(rebuilt.workletRate);
        const refused = /refused/.test(rebuilt.sampleRateStatus);
        assert.ok(refused || rateAfter === 22050, `the context rate moved to 22050 Hz, or the refusal was reported: ${rebuilt.sampleRateStatus}`);
        if (refused) assert.equal(rateAfter, rateBefore, 'a refused rate leaves the old context playing');
        assert.equal(rateDigits(rebuilt.outputStatus), rateAfter, 'the context and the worklet agree on the rate');
        assert.equal(rebuilt.chip, 'playing', 'the rebuilt graph is playing again');
        assert.equal(rebuilt.errorText, beforeRate.errorText, `the rate rebuild raised an error: ${rebuilt.errorText}`);
        const orderAfter = orderNumber(rebuilt.order);
        assert.ok(orderAfter >= orderBefore && orderAfter <= orderBefore + 1, `the rebuild resumed at the sounding order (${orderBefore} → ${orderAfter})`);
        assert.equal(rebuilt.health, 'healthy');
        report.rateRequested = 22050;
        report.rateActual = rateAfter;
        report.rateStatus = rebuilt.sampleRateStatus;
        report.rateRefused = refused;
        report.orderAcrossRebuild = `${orderBefore} → ${orderAfter}`;
        report.latency = `${rebuilt.baseLatency} base, ${rebuilt.outputLatency} output`;
        report.sink = `${rebuilt.sinkStatus} (max ${rebuilt.maxChannelCount} channels)`;

        // ── the retro mixer mode, applied to the live engine ────────────────────────
        // The mode string is read back out of the engine's own telemetry header, so this
        // asserts the whole round trip: page select → wire opcode → host engine rebuild →
        // active mode word → panel.
        const beforeMixer = await page.evaluate(READ_STATE);
        await page.evaluate(`
            document.getElementById('mixer-path').value = 'fixed';
            document.getElementById('mixer-interpolator').value = 'nearest';
            document.getElementById('mixer-depth').value = 'i8';
            document.getElementById('mixer-dither').value = 'off';
            document.getElementById('apply-mixer').click();
            return true;
        `);
        await page.waitFor('the retro mixer mode in the engine telemetry', "document.getElementById('engine-mode').textContent === 'fixed \u00b7 nearest \u00b7 8-bit \u00b7 stereo'");
        const retro = await page.evaluate(READ_STATE);
        assert.equal(retro.outputEngineMode, 'fixed \u00b7 nearest \u00b7 8-bit \u00b7 stereo', 'the Output panel shows the same active mode');
        assert.equal(retro.chip, 'playing', 'the engine rebuild did not stop the transport');
        assert.equal(retro.order, beforeMixer.order, 'the rebuilt engine resumed at the order that was sounding');
        // The rebuilt sequencer seeks to the sounding order, which restarts it at row 0, so
        // two samples taken the same distance after two rebuilds read the same row. Wait for
        // the row to move instead of comparing two snapshots.
        await page.waitFor('the 8-bit fixed path to keep rendering', `document.getElementById('row').textContent !== ${JSON.stringify(retro.row)}`);
        await delay(2000);
        const afterRetro = await page.evaluate(READ_STATE);
        assert.equal(afterRetro.memory, retro.memory, `wasm memory moved after the engine rebuild: ${afterRetro.memory}`);
        assert.ok(afterRetro.memory.includes('stable'), `wasm memory grew after the engine rebuild: ${afterRetro.memory}`);
        assert.equal(afterRetro.health, 'healthy');
        assert.equal(afterRetro.errorText, beforeRate.errorText, `the mixer switch raised an error: ${afterRetro.errorText}`);
        const stored = await page.evaluate("return localStorage.getItem('starplayer.output-and-mixer.v1');");
        assert.match(stored, /"sampleRate":"22050"/, 'the requested rate is remembered');
        assert.match(stored, /"depth":"i8"/, 'the mixer depth is remembered');
        assert.match(stored, /"path":"fixed"/, 'the mixer path is remembered');
        report.mixerMode = retro.engineMode;
        report.mixerMemory = afterRetro.memory;
        report.stored = JSON.parse(stored);

        // ── mono, which rebuilds the node inside the same AudioContext ──────────────
        // The mono engine arms are otherwise only reached by the Rust unit tests; this is
        // the one place the whole mono path renders through a real AudioWorklet.
        await page.evaluate(`
            const select = document.getElementById('output-channels');
            select.value = '1';
            select.dispatchEvent(new Event('change'));
            document.getElementById('apply-channels').click();
            return true;
        `);
        await page.waitFor('the mono engine arm', "document.getElementById('engine-mode').textContent === 'fixed \u00b7 nearest \u00b7 8-bit \u00b7 mono'");
        const mono = await page.evaluate(READ_STATE);
        assert.match(mono.channelStatus, /^mono/, `the Output panel reports mono: ${mono.channelStatus}`);
        assert.equal(mono.chip, 'playing', 'the mono rebuild kept the transport running');
        await page.waitFor('the mono path to keep rendering', `document.getElementById('row').textContent !== ${JSON.stringify(mono.row)}`);
        const afterMono = await page.evaluate(READ_STATE);
        assert.equal(afterMono.errorText, beforeRate.errorText, `the mono rebuild raised an error: ${afterMono.errorText}`);
        report.monoMode = mono.engineMode;
        report.monoChannels = mono.channelStatus;

        // A fresh graph starts the silent keep-alive in the first audio gesture.
        await page.send('Page.reload', { ignoreCache: true });
        await page.waitFor('background scenario startup', "document.documentElement.dataset.playerReady === 'true'");
        assert.equal(await page.evaluate("return document.getElementById('background-playback').checked;"), false, 'desktop background playback defaults off');
        await page.evaluate(`
            document.getElementById('force-fallback').checked = ${mode === 'fallback'};
            document.getElementById('background-playback').click();
            document.getElementById('start-audio').click();
            return true;
        `);
        await page.waitFor('background audio graph', "document.getElementById('transport-chip').textContent === 'audio ready'");
        await loadFixture(page, FIRST_MODULE);
        await page.waitFor('background media playing', "document.querySelector('audio')?.src.startsWith('blob:') && document.querySelector('audio').duration > 5 && document.querySelector('audio').srcObject === null && !document.querySelector('audio').paused && navigator.mediaSession.playbackState === 'playing'");
        assert.equal(await page.evaluate("return document.querySelector('audio').hidden;"), true);
        const contextTime = await page.evaluate('return window.testAudioContext.currentTime;');
        await delay(300);
        assert.ok(await page.evaluate(`return window.testAudioContext.currentTime > ${contextTime};`), 'direct context time advances');
        await page.waitFor('background master peak', "parseFloat(document.getElementById('master-peak').style.width) > 0");
        assert.equal(await page.evaluate('return navigator.mediaSession.metadata.title;'), (await page.evaluate(READ_STATE)).title);
        await loadFixture(page, SECOND_MODULE);
        await page.waitFor('background track replacement', "document.getElementById('title').textContent === 'Petrified' && navigator.mediaSession.metadata.title === 'Petrified' && !document.querySelector('audio').paused && navigator.mediaSession.playbackState === 'playing'");
        await page.evaluate("window.mediaHandlers.pause(); return true;");
        await page.waitFor('media pause', "navigator.mediaSession.playbackState === 'paused' && document.getElementById('transport-chip').textContent === 'stopped' && document.querySelector('audio').paused");
        await page.evaluate("window.mediaHandlers.play(); return true;");
        await page.waitFor('media resume', "navigator.mediaSession.playbackState === 'playing' && document.getElementById('transport-chip').textContent === 'playing' && !document.querySelector('audio').paused");
        await page.evaluate("document.getElementById('background-playback').click(); return true;");
        await page.waitFor('direct output after background toggle', "document.querySelector('audio').paused && document.querySelector('audio').srcObject === null && parseFloat(document.getElementById('master-peak').style.width) > 0 && navigator.mediaSession.playbackState === 'playing'");
        report.backgroundPlayback = 'silent WAV, direct rendering, media metadata, pause/play, toggle passed';

        // ── phone width ─────────────────────────────────────────────────────────────
        await page.evaluate("document.getElementById('load-options-disclosure').open = true; return true;");
        await page.send('Emulation.setDeviceMetricsOverride', {
            ...PHONE_VIEWPORT, deviceScaleFactor: 3, mobile: true,
        });
        await delay(500);
        const layout = await page.evaluate(`
            return {
                scrollWidth: document.documentElement.scrollWidth,
                innerWidth: window.innerWidth,
                bodyScrollWidth: document.body.scrollWidth,
                controlsVisible: document.getElementById('play').getBoundingClientRect().width > 0,
                speedChooserVisible: document.getElementById('speed-adjust').getBoundingClientRect().width > 0,
            };
        `);
        assert.ok(layout.scrollWidth <= layout.innerWidth, `the document overflows at ${PHONE_VIEWPORT.width}px: ${layout.scrollWidth} > ${layout.innerWidth}`);
        assert.ok(layout.bodyScrollWidth <= layout.innerWidth, `the body overflows at ${PHONE_VIEWPORT.width}px: ${layout.bodyScrollWidth}`);
        assert.ok(layout.controlsVisible, 'the transport controls are still laid out at phone width');
        assert.ok(layout.speedChooserVisible, 'the speed chooser is visible in the open options at phone width');
        report.phone = `${layout.scrollWidth} <= ${layout.innerWidth}`;
        await page.send('Emulation.clearDeviceMetricsOverride');

        // ── console ─────────────────────────────────────────────────────────────────
        assert.deepEqual(page.consoleErrors, [], 'the page logged console errors');
        report.browser = page.version;
        report.dropped = afterBadFile.dropped;
    } finally {
        await page.close();
        server.stop();
    }
    return report;
}

// Both owner fixtures end naturally. Derive the looping case from PETRI in memory:
// append B00 to its last row, preserving all sample/music bytes and the fixture on disk.
function withFinalLoop(bytes) {
    const orders = bytes.readUInt16LE(32);
    const instruments = bytes.readUInt16LE(34);
    const lastPattern = [...bytes.subarray(96, 96 + orders)].filter(order => order < 254).at(-1);
    const pointerOffset = 96 + orders + instruments * 2 + lastPattern * 2;
    const patternOffset = bytes.readUInt16LE(pointerOffset) * 16;
    let cursor = patternOffset + 2;
    let lastRowEnd = cursor;
    for (let row = 0; row < 64; row += 1) {
        while (bytes[cursor] !== 0) {
            const token = bytes[cursor++];
            if (token & 32) cursor += 2;
            if (token & 64) cursor += 1;
            if (token & 128) cursor += 2;
        }
        lastRowEnd = cursor++;
    }
    const pattern = Buffer.concat([bytes.subarray(patternOffset + 2, lastRowEnd), Buffer.from([128, 2, 0, 0])]);
    const appendedOffset = Math.ceil(bytes.length / 16) * 16;
    const result = Buffer.alloc(appendedOffset + 2 + pattern.length);
    bytes.copy(result);
    result.writeUInt16LE(appendedOffset / 16, pointerOffset);
    result.writeUInt16LE(pattern.length, appendedOffset);
    pattern.copy(result, appendedOffset + 2);
    return result;
}

// W9 uses only the two owner-authored S3Ms. All generated files stay in this worktree.
async function runLibrary(executable, mode) {
    const artifacts = join(REPOSITORY_ROOT, 'target/w9-verification');
    await mkdir(artifacts, { recursive: true });
    const directory = await mkdtemp(join(artifacts, 'library-'));
    let server;
    let page;
    try {
        const reflex = await readFile(join(REPOSITORY_ROOT, 'crates/starplayer-s3m/tests/fixtures/REFLEX.S3M'));
        const petri = await readFile(join(REPOSITORY_ROOT, 'crates/starplayer-s3m/tests/fixtures/PETRI.S3M'));
        await mkdir(join(directory, 'loose'));
        await mkdir(join(directory, 'disks'));
        await writeFile(join(directory, 'loose/REFLEX.S3M'), reflex);
        await writeFile(join(directory, 'disks/disk1.zip'), buildZip([{ name: 'PETRI.S3M', bytes: petri }, { name: 'REFLEX.S3M', bytes: reflex }]));
        await writeFile(join(directory, 'disks/outer.zip'), buildZip([{ name: 'inner.zip', bytes: buildZip([{ name: 'PETRI.S3M', bytes: petri }]) }]));
        const header = 'member_path,title,source_path,container_member,sha256\n';
        await writeFile(join(directory, 'index.csv'), header + [
            ',Loose Reflex,loose/REFLEX.S3M,',
            'PETRI.S3M,Disk Petri,disks/disk1.zip,',
            'REFLEX.S3M,Disk Reflex,disks/disk1.zip,',
            'PETRI.S3M,Nested Petri,disks/outer.zip,inner.zip',
            'MISSING.S3M,Missing member,disks/disk1.zip,',
        ].map((row, index) => `${row},${index < 2 ? String(index + 1).repeat(64) : ''}`).join('\n'));
        await writeFile(join(directory, 'comments.csv'), `sha256,instrument_names,song_message\n${'1'.repeat(64)},"Future-Crew\n\n<b>bold</b>","Hello\nMessage line two"\n${'2'.repeat(64)},Other group,`);
        server = await startDevServer(mode !== 'plain', directory);
        page = await Page.open(executable, `http://127.0.0.1:${server.port}/`);
        await page.waitFor('library startup', "document.documentElement.dataset.playerReady === 'true'");
        await page.evaluate(`
            if (${JSON.stringify(mode)} === 'fallback') document.getElementById('force-fallback').checked = true;
            document.getElementById('library-server').click();
        `);
        await page.waitFor('five library rows', "document.getElementById('library-count').textContent === '5 of 5'");
        const filter = async (value, expected) => {
            await page.evaluate(`await new Promise(resolve => { const observer = new MutationObserver(() => { observer.disconnect(); requestAnimationFrame(resolve); }); observer.observe(document.getElementById('library-count'), { childList: true }); const input = document.getElementById('library-search'); input.value = ${JSON.stringify(value)}; input.dispatchEvent(new Event('input')); });`);
            await page.waitFor('filtered library count', `document.getElementById('library-count').textContent === ${JSON.stringify(expected)}`);
        };
        const clickRow = (id, modifiers = {}) => page.evaluate(`document.querySelector('[data-row-id="${id}"]').dispatchEvent(new MouseEvent('click', { bubbles: true, ...${JSON.stringify(modifiers)} }));`);
        await page.waitFor('comments attached', "document.getElementById('library-summary').textContent.includes('2 tracks matched')");
        await filter('text:futurecrew', '1 of 5');
        await filter('futurecrew', '1 of 5');
        await page.evaluate("document.getElementById('library-search-comments').click();");
        await page.waitFor('comments excluded', "document.getElementById('library-count').textContent === '0 of 5'");
        await filter('text:futurecrew', '1 of 5');
        await page.evaluate("document.getElementById('library-search-comments').click();");
        await filter('text:Future-Crew', '1 of 5');
        await clickRow(0);
        const details = await page.evaluate(`
            const pane = document.getElementById('library-details');
            return { hidden: pane.hidden, text: pane.textContent, instruments: pane.querySelector('pre').textContent, marks: [...pane.querySelectorAll('mark')].map(mark => mark.textContent), bold: pane.querySelectorAll('b').length };
        `);
        assert.equal(details.hidden, false);
        assert.equal(details.instruments, '01  Future-Crew\n02  \n03  <b>bold</b>');
        assert.deepEqual(details.marks, ['Future-Crew']);
        assert.equal(details.bold, 0);
        assert.ok(details.text.includes('Hello\nMessage line two'));
        // The match column and a real hover's popover highlight the comment line the row matched on, as text only.
        const matchCell = await page.evaluate("const cell = document.querySelector('.library-comment-cell'); return { visible: getComputedStyle(document.querySelector('.library-comment-heading')).display !== 'none', text: cell.textContent, marks: [...cell.querySelectorAll('mark')].map(mark => mark.textContent) };");
        assert.deepEqual(matchCell, { visible: true, text: '#01Future-Crew', marks: ['Future-Crew'] });
        const hoverPoint = await page.evaluate("const row = document.querySelector('#library-rows > div'); row.scrollIntoView({ block: 'center' }); const bounds = row.getBoundingClientRect(); return { x: bounds.x + 60, y: bounds.y + 16 };");
        await page.send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...hoverPoint });
        await page.waitFor('comment match popover', "!document.getElementById('library-match-popover').hidden");
        const popover = await page.evaluate("const popover = document.getElementById('library-match-popover'); return { text: popover.textContent, marks: [...popover.querySelectorAll('mark')].map(mark => mark.textContent), bold: popover.querySelectorAll('b').length };");
        assert.ok(popover.text.includes('Instrument 01'), popover.text);
        assert.deepEqual(popover.marks, ['Future-Crew']);
        assert.equal(popover.bold, 0);
        await filter('', '5 of 5');
        assert.ok(await page.evaluate("return document.getElementById('library-match-popover').hidden && !document.querySelector('.library-comment-cell') && getComputedStyle(document.querySelector('.library-comment-heading')).display === 'none';"));
        await clickRow(4);
        await page.evaluate("window.paintedLibraryRow = document.querySelector('#library-rows > div'); document.getElementById('library-grid').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));");
        assert.ok(await page.evaluate("return document.getElementById('library-details').textContent.includes('Other group');"));
        assert.ok(await page.evaluate("return window.paintedLibraryRow === document.querySelector('#library-rows > div');"), 'focus updates details without replacing grid rows');
        await clickRow(2);
        assert.ok(await page.evaluate("return document.getElementById('library-details').textContent.includes('No comment text');"));
        await page.send('Emulation.setDeviceMetricsOverride', { ...PHONE_VIEWPORT, deviceScaleFactor: 3, mobile: true });
        assert.ok(await page.evaluate("return document.documentElement.scrollWidth <= 390;"));
        await page.send('Emulation.clearDeviceMetricsOverride');
        await filter('title:Disk', '2 of 5');
        await clickRow(1);
        await clickRow(2, { shiftKey: true });
        await page.evaluate("document.getElementById('library-add-selected').click();");
        assert.deepEqual(await page.evaluate("return [...document.querySelectorAll('#playlist-list li')].map(item => item.querySelector('.playlist-name').textContent);"), ['PETRI.S3M', 'REFLEX.S3M']);
        await page.evaluate("document.querySelector('[data-row-id=\"1\"] button').click();");
        assert.deepEqual(await page.evaluate("return [...document.querySelectorAll('#playlist-list li')].map(item => item.querySelector('.playlist-name').textContent);"), ['PETRI.S3M', 'REFLEX.S3M', 'PETRI.S3M', 'REFLEX.S3M']);
        await filter('', '5 of 5');
        await clickRow(0, { ctrlKey: true });
        await clickRow(2, { ctrlKey: true });
        await page.evaluate("document.getElementById('library-add-selected').click();");
        assert.deepEqual(await page.evaluate("return [...document.querySelectorAll('#playlist-list li')].map(item => item.querySelector('.playlist-name').textContent);"), ['PETRI.S3M', 'REFLEX.S3M', 'PETRI.S3M', 'REFLEX.S3M', 'PETRI.S3M', 'REFLEX.S3M']);
        // A real Shift-click must select rows, not page text (Edge pops up a Copy/Search menu over a text selection).
        const realClick = async (position, modifiers = 0) => {
            const point = await page.evaluate(`const row = document.querySelector('#library-rows > :nth-child(${position})'); row.scrollIntoView({ block: 'center' }); const bounds = row.getBoundingClientRect(); return { x: bounds.x + 25, y: bounds.y + 16 };`);
            for (const type of ['mousePressed', 'mouseReleased']) await page.send('Input.dispatchMouseEvent', { type, button: 'left', clickCount: 1, modifiers, ...point });
        };
        await realClick(1);
        await realClick(3, 8);
        assert.equal(await page.evaluate("return document.querySelectorAll('.library-track[aria-selected=true]').length;"), 3);
        assert.equal(await page.evaluate("return getSelection().toString();"), '');
        await page.evaluate("document.getElementById('library-browse').click(); document.querySelector('[data-scope=disks]').click();");
        await page.waitFor('folder scope', "document.getElementById('library-count').textContent === '4 of 5'");
        assert.equal(await page.evaluate("return document.querySelector('[role=tree] [aria-selected=true]').dataset.scope;"), 'disks');
        await page.evaluate("document.querySelector('[data-scope=disks] [data-tree-toggle]').click();");
        assert.equal(await page.evaluate("return document.activeElement.dataset.scope + ' ' + document.activeElement.getAttribute('aria-expanded');"), 'disks true');
        const treeKey = key => page.evaluate(`document.activeElement.dispatchEvent(new KeyboardEvent('keydown', { key: ${JSON.stringify(key)}, bubbles: true }));`);
        await treeKey('ArrowLeft');
        assert.equal(await page.evaluate("return document.activeElement.dataset.scope + ' ' + document.activeElement.getAttribute('aria-expanded');"), 'disks false');
        await treeKey('ArrowRight');
        await treeKey('ArrowRight');
        assert.equal(await page.evaluate("return document.activeElement.dataset.scope;"), 'disks/disk1.zip');
        await treeKey('ArrowLeft');
        assert.equal(await page.evaluate("return document.activeElement.dataset.scope;"), 'disks');
        await page.evaluate("document.querySelector('[data-scope=\"disks/disk1.zip\"]').focus(); document.activeElement.click();");
        assert.equal(await page.evaluate("return document.activeElement.dataset.scope;"), 'disks/disk1.zip');
        await page.waitFor('ZIP scope', "document.getElementById('library-count').textContent === '3 of 5'");
        await page.evaluate("document.querySelector('[aria-label=\"Add folder disks/disk1.zip\"]').click();");
        assert.equal(await page.evaluate("return document.getElementById('playlist-count').textContent;"), '9');
        // Real mouse clicks also prove that selection updates preserve the double-click target.
        const doubleClick = async id => {
            const point = await page.evaluate(`const row = document.querySelector('[data-row-id="${id}"]'); row.scrollIntoView({ block: 'center' }); const bounds = row.getBoundingClientRect(); return { x: bounds.x + 25, y: bounds.y + 16 };`);
            for (const clickCount of [1, 2]) {
                await page.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount, ...point });
                await page.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount, ...point });
            }
        };
        await doubleClick(2);
        await page.waitFor('ZIP playback title', "document.getElementById('title').textContent.toLowerCase().includes('reflex') && document.getElementById('transport-chip').textContent === 'playing'");
        await page.evaluate("document.querySelector('[role=tree] [data-scope=\"\"]').click();");
        await doubleClick(3);
        await page.waitFor('nested playback label', "document.getElementById('module-detail').textContent.includes('inner.zip') && document.getElementById('transport-chip').textContent === 'playing'");
        const beforeMissing = await page.evaluate(READ_STATE);
        assert.equal(beforeMissing.title, 'Petrified');
        await doubleClick(4);
        await page.waitFor('missing member diagnostic', "!document.getElementById('error').hidden && document.getElementById('error').textContent.includes('MISSING.S3M')");
        const missing = await page.evaluate(READ_STATE);
        assert.equal(missing.title, beforeMissing.title);
        assert.equal(missing.chip, 'playing');
        await page.waitFor('playback survives missing member', `document.getElementById('row').textContent !== ${JSON.stringify(missing.row)}`);
        assert.equal(await page.evaluate("return document.querySelector('.library-track.playing').dataset.rowId;"), '3');
        await page.send('Emulation.setDeviceMetricsOverride', { ...PHONE_VIEWPORT, deviceScaleFactor: 3, mobile: true });
        const phoneLayout = await page.evaluate("return { width: innerWidth, scrollWidth: document.documentElement.scrollWidth, overflowing: [...document.querySelectorAll('main > *, .load-panel > *, .now-playing > *')].filter(element => element.getBoundingClientRect().right > 390).map(element => [element.className, element.getBoundingClientRect().width]) };");
        assert.ok(phoneLayout.width === 390 && phoneLayout.scrollWidth <= 390, `library fits at 390px: ${JSON.stringify(phoneLayout)}`);
        await page.send('Emulation.clearDeviceMetricsOverride');

        // Measure the real CSV -> parser -> view route, then input -> filtered paint and
        // animation-frame scroll cadence. No application-only benchmark shortcut.
        await writeFile(join(directory, 'index.csv'), header + Array.from({ length: 37674 }, (_, index) => `,Track ${index},group${index % 100}/TRACK${String(index).padStart(5, '0')}.S3M,`).join('\n'));
        await page.evaluate(`
            const originalFetch = window.fetch;
            window.fetch = async (...args) => {
                const response = await originalFetch(...args);
                if (args[0] === '/library-index.csv' && !args[1]?.method) {
                    const originalBuffer = response.arrayBuffer.bind(response);
                    response.arrayBuffer = async () => { const bytes = await originalBuffer(); window.csvLoadedAt = performance.now(); return bytes; };
                }
                return response;
            };
            window.firstPaint = new Promise(resolve => {
                const observer = new MutationObserver(() => {
                    if (document.getElementById('library-count').textContent === '37,674 of 37,674') {
                        observer.disconnect();
                        requestAnimationFrame(() => resolve(performance.now() - window.csvLoadedAt));
                    }
                });
                observer.observe(document.getElementById('library-count'), { childList: true });
            });
            document.getElementById('library-server').click();
        `);
        const firstPaint = await page.evaluate('return await window.firstPaint;');
        const filterMilliseconds = await page.evaluate(`
            const results = [];
            for (const value of ['T', 'TR', 'TRA', 'TRACK3', 'TRACK37']) {
                const start = performance.now();
                await new Promise(resolve => {
                    const observer = new MutationObserver(() => { observer.disconnect(); requestAnimationFrame(resolve); });
                    observer.observe(document.getElementById('library-count'), { childList: true });
                    const input = document.getElementById('library-search');
                    input.value = value;
                    input.dispatchEvent(new Event('input'));
                });
                results.push(performance.now() - start);
            }
            return results;
        `);
        await filter('', '37,674 of 37,674');
        const scroll = await page.evaluate(`
            const viewport = document.getElementById('library-viewport');
            const intervals = [];
            let maximumRows = 0;
            let previous = performance.now();
            for (let index = 0; index < 120; index += 1) {
                viewport.scrollTop = index * 1000;
                await new Promise(requestAnimationFrame);
                const now = performance.now();
                intervals.push(now - previous);
                previous = now;
                maximumRows = Math.max(maximumRows, document.querySelectorAll('.library-track').length);
            }
            return { maximumRows, averageFrame: intervals.reduce((sum, value) => sum + value, 0) / intervals.length, maximumFrame: Math.max(...intervals) };
        `);
        assert.ok(firstPaint < 1500, `first paint: ${firstPaint}ms`);
        assert.ok(scroll.maximumRows <= 200);
        await page.evaluate("document.querySelector('[data-column=title] button').click();");
        assert.equal(await page.evaluate("return document.querySelector('[data-column=title]').getAttribute('aria-sort');"), 'ascending');
        await page.evaluate("document.querySelector('[data-column=title] button').click();");
        assert.equal(await page.evaluate("return document.querySelector('[data-column=title]').getAttribute('aria-sort');"), 'descending');
        await page.evaluate("document.getElementById('library-grid').focus(); document.getElementById('library-grid').dispatchEvent(new KeyboardEvent('keydown', { key: 'a', ctrlKey: true, bubbles: true }));");
        assert.match(await page.evaluate("return document.getElementById('library-selection-count').textContent;"), /^37,674 playable selected/);
        await page.evaluate("const grid = document.getElementById('library-grid'); grid.dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true }));");
        assert.ok(await page.evaluate("return document.getElementById(document.getElementById('library-grid').getAttribute('aria-activedescendant')) !== null;"));
        assert.match(await page.evaluate("return document.getElementById('library-selection-count').textContent;"), /^1 playable selected/);
        assert.equal(await page.evaluate("return document.getElementById('library-add-selected').disabled;"), false);
        await page.evaluate("document.getElementById('library-grid').dispatchEvent(new KeyboardEvent('keydown', { key: 'Home', shiftKey: true, bubbles: true }));");
        assert.match(await page.evaluate("return document.getElementById('library-selection-count').textContent;"), /^37,674 playable selected/);
        await page.evaluate("document.getElementById('library-grid').dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));");
        assert.match(await page.evaluate("return document.getElementById('library-selection-count').textContent;"), /^0 playable selected/);
        assert.equal(await page.evaluate("return document.getElementById('library-add-selected').disabled;"), true);
        await page.evaluate("document.getElementById('library-grid').dispatchEvent(new KeyboardEvent('keydown', { key: 'End', bubbles: true }));");
        assert.match(await page.evaluate("return document.getElementById('library-selection-count').textContent;"), /^1 playable selected/);
        assert.equal(await page.evaluate("return document.getElementById('library-add-selected').disabled;"), false);
        await page.evaluate("document.getElementById('library-grid').dispatchEvent(new KeyboardEvent('keydown', { key: 'Home', shiftKey: true, bubbles: true }));");
        assert.match(await page.evaluate("return document.getElementById('library-selection-count').textContent;"), /^37,674 playable selected/);
        await filter('TRACK37673', '1 of 37,674');
        await page.evaluate("document.getElementById('library-add-selected').click();");
        assert.equal(await page.evaluate("return document.querySelector('#playlist-list li:last-child .playlist-name').textContent;"), 'TRACK37673.S3M');
        assert.equal(await page.evaluate("return document.getElementById('playlist-count').textContent;"), '10');
        await writeFile(join(directory, 'index.csv'), header + ',Reflex,loose/REFLEX.S3M,\n,Unsupported,loose/SILENT.669,');
        await page.evaluate("document.getElementById('library-server').click();");
        await page.waitFor('unplayable hidden by default', "document.getElementById('library-count').textContent === '1 of 2'");
        await page.evaluate("document.getElementById('library-unplayable').click();");
        await page.waitFor('show unplayable', "document.getElementById('library-count').textContent === '2 of 2'");
        await page.evaluate("const format = document.getElementById('library-format'); format.value = '669'; format.dispatchEvent(new Event('change'));");
        await page.waitFor('format filter', "document.getElementById('library-count').textContent === '1 of 2'");
        await clickRow(1);
        assert.equal(await page.evaluate("return document.querySelector('.library-track button').disabled && document.getElementById('library-add-selected').disabled;"), true);
        await doubleClick(1);
        assert.equal((await page.evaluate(READ_STATE)).title, 'Petrified');
        // W10: capture real engine snapshots on both transports without an app test API.
        await page.evaluate(`
            window.playlistSnapshots = [];
            const capture = snapshot => {
                if (!snapshot) return;
                window.latestPlaylistSnapshot = snapshot;
                const value = { playing: snapshot.playing, flags: snapshot.songFlags, frame: snapshot.songFrame, generation: snapshot.moduleGeneration };
                const last = window.playlistSnapshots.at(-1);
                if (!last || last.playing !== value.playing || last.flags !== value.flags || last.generation !== value.generation || (last.frame === 0) !== (value.frame === 0)) window.playlistSnapshots.push(value);
            };
            const read = StarPlayerRing.readTelemetry;
            StarPlayerRing.readTelemetry = (...args) => { const snapshot = read(...args); capture(snapshot); return snapshot; };
            const OriginalNode = AudioWorkletNode;
            window.AudioWorkletNode = class extends OriginalNode {
                constructor(...args) { super(...args); this.port.addEventListener('message', event => { if (event.data.type === 'telemetry') capture(event.data); }); }
            };
        `);
        await page.evaluate("document.getElementById('background-playback').checked = true; document.getElementById('background-playback').dispatchEvent(new Event('change'));");
        // Queue REFLEX -> a PETRI loop variant. The original fixture is unchanged.
        await writeFile(join(directory, 'disks/loop.zip'), buildZip([{ name: 'PETRI.S3M', bytes: withFinalLoop(petri) }]));
        await writeFile(join(directory, 'index.csv'), header + [
            ',Loose Reflex,loose/REFLEX.S3M,',
            'PETRI.S3M,Disk Petri,disks/loop.zip,',
            'REFLEX.S3M,Disk Reflex,disks/disk1.zip,',
            'PETRI.S3M,Nested Petri,disks/outer.zip,inner.zip',
            'MISSING.S3M,Missing member,disks/disk1.zip,',
        ].join('\n'));
        await page.evaluate("document.getElementById('playlist-clear').click(); document.getElementById('library-format').value = ''; document.getElementById('library-server').click();");
        await page.waitFor('restored library', "document.getElementById('library-count').textContent === '5 of 5'");
        await filter('title:Loose', '1 of 5');
        await page.waitFor('loose row painted', "document.querySelector('[data-row-id=\"0\"]') !== null");
        await clickRow(0);
        await page.evaluate("document.getElementById('library-add-selected').click();");
        await filter('title:Disk name:PETRI', '1 of 5');
        await page.waitFor('filtered Petri row', "document.querySelector('[data-row-id=\"1\"]') !== null");
        await clickRow(1);
        await page.evaluate(`
            document.getElementById('library-add-selected').click();
            const repeat = document.getElementById('repeat'); repeat.checked = true; repeat.dispatchEvent(new Event('change'));
            document.querySelector('#playlist-list li').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
        `);
        await page.waitFor('first playlist track', "document.querySelector('#playlist-list li.current .playlist-name')?.textContent === 'REFLEX.S3M'");
        assert.equal(await page.evaluate("return document.getElementById('repeat').checked && document.getElementById('repeat').disabled;"), true);
        // Stop in the same turn as starting a rebuild: its saved Play must not win.
        await page.evaluate("document.getElementById('sample-rate').value = '32000'; document.getElementById('apply-sample-rate').click(); document.getElementById('stop').click();");
        await page.waitFor('Stop during pending graph rebuild', "document.getElementById('transport-chip').textContent === 'stopped'");
        await delay(1500);
        assert.equal((await page.evaluate(READ_STATE)).chip, 'stopped');
        await page.evaluate("document.getElementById('play').click();");
        await page.waitFor('playlist resumed after cancelled rebuild', "document.getElementById('transport-chip').textContent === 'playing'");
        // Rebuild while playlist mode owns at-end policy. The new node also captures
        // fallback snapshots; its generation starts again at one.
        await page.evaluate("document.getElementById('sample-rate').value = '48000'; document.getElementById('apply-sample-rate').click();");
        await page.waitFor('playlist sample-rate rebuild', "document.getElementById('message').textContent.includes('rebuilt') && document.getElementById('message').textContent.includes('48,000 Hz')");
        await page.waitFor('rebuilt playlist telemetry painted', "window.latestPlaylistSnapshot?.moduleGeneration === 1 && window.latestPlaylistSnapshot.playing === true && Number(document.getElementById('progress').max) === window.latestPlaylistSnapshot.songLengthFrames");
        const seekNearEnd = async seconds => {
            const rate = rateDigits((await page.evaluate(READ_STATE)).workletRate);
            await page.evaluate(`const progress = document.getElementById('progress'); progress.value = String(Number(progress.max) - ${rate * seconds}); progress.dispatchEvent(new Event('change'));`);
        };
        await seekNearEnd(1);
        await page.evaluate("window.savedAnimationFrame = requestAnimationFrame; window.requestAnimationFrame = callback => { window.pausedAnimationFrame = callback; return 0; };");
        await page.waitFor('automatic second playlist track', "document.querySelector('#playlist-list li.current .playlist-name')?.textContent === 'PETRI.S3M' && document.getElementById('title').textContent === 'Petrified'", 15000);
        assert.equal(await page.evaluate("return document.querySelector('#playlist-list li').classList.contains('current');"), false);
        await page.evaluate("window.requestAnimationFrame = window.savedAnimationFrame; if (window.pausedAnimationFrame) requestAnimationFrame(window.pausedAnimationFrame);");
        await page.waitFor('second playlist telemetry', "window.latestPlaylistSnapshot?.moduleGeneration === 2 && window.latestPlaylistSnapshot?.playing");
        await page.waitFor('keep-alive through playlist advance', "!document.querySelector('audio').paused && navigator.mediaSession.playbackState === 'playing'");
        await delay(200);
        assert.equal(await page.evaluate("return window.latestPlaylistSnapshot.songFlags & 2;"), 2);
        await page.evaluate("document.getElementById('playlist-loop').checked = true;");
        await seekNearEnd(4);
        await page.waitFor('PETRI fade', "(window.latestPlaylistSnapshot?.songFlags & 8) !== 0", 15000);
        await page.evaluate("document.getElementById('stop').click();");
        await page.waitFor('manual stop in fade', "document.getElementById('transport-chip').textContent === 'stopped'");
        await delay(5500);
        assert.equal(await page.evaluate("return document.querySelector('#playlist-list li.current .playlist-name').textContent;"), 'PETRI.S3M');
        const stoppedFade = await page.evaluate("return window.latestPlaylistSnapshot;");
        assert.equal(stoppedFade.playing, false);
        // Re-arm and let the complete fade finish to measure the host rewind too.
        await page.evaluate("document.getElementById('playlist-loop').checked = false; document.getElementById('play').click();");
        await seekNearEnd(6);
        await page.waitFor('rearmed looping playback', "document.getElementById('transport-chip').textContent === 'playing'");
        await page.waitFor('natural looping stop', "document.getElementById('transport-chip').textContent === 'stopped'", 15000);
        await page.waitFor('keep-alive paused at playlist end', "document.querySelector('audio').paused && navigator.mediaSession.playbackState === 'paused'");
        await delay(500);
        assert.equal(await page.evaluate("return document.querySelector('#playlist-list li.current .playlist-name').textContent;"), 'PETRI.S3M');
        assert.equal((await page.evaluate(READ_STATE)).progressValue, 0);
        assert.equal(await page.evaluate("return window.latestPlaylistSnapshot.songFlags;"), 7);
        await page.evaluate("document.getElementById('play').click();");
        await page.waitFor('host rewind on next Play', "window.latestPlaylistSnapshot?.playing && window.latestPlaylistSnapshot.songFrame < 48000 && window.latestPlaylistSnapshot.songFlags === 3");
        const playlistSnapshots = await page.evaluate("return window.playlistSnapshots;");
        assert.equal(await page.evaluate("return document.querySelector('#playlist-list li.current .playlist-name').textContent;"), 'PETRI.S3M');
        await page.evaluate("document.querySelector('#playlist-list li.current').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowUp', altKey: true, bubbles: true }));");
        assert.equal(await page.evaluate("return document.querySelector('#playlist-list li').classList.contains('current');"), true);
        await page.evaluate("document.querySelector('#playlist-list li.current').dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', altKey: true, bubbles: true }));");
        assert.equal(await page.evaluate("return document.querySelector('#playlist-list li:last-child').classList.contains('current');"), true);
        await page.evaluate("document.querySelector('#playlist-list li.current').dispatchEvent(new KeyboardEvent('keydown', { key: 'Delete', bubbles: true }));");
        await delay(300);
        assert.equal(await page.evaluate("return document.querySelector('#playlist-list li.current');"), null);
        assert.equal((await page.evaluate(READ_STATE)).chip, 'playing');
        assert.equal(await page.evaluate("return document.getElementById('playlist-count').textContent;"), '1');
        await page.evaluate("document.getElementById('playlist-clear').click();");
        assert.equal(await page.evaluate("return document.getElementById('playlist-count').textContent;"), '0');
        // A failed load from a stopped state starts the keep-alive in its gesture; nothing then
        // plays, so the idle rule must pause it again rather than leave a silent session running.
        await page.evaluate("document.getElementById('stop').click(); return true;");
        await page.waitFor('keep-alive paused by Stop', "document.querySelector('audio').paused && document.getElementById('transport-chip').textContent === 'stopped'");
        await page.evaluate(`
            const transfer = new DataTransfer();
            transfer.items.add(new File([new Uint8Array(64).fill(0x41)], 'broken.s3m', { type: 'application/octet-stream' }));
            document.getElementById('drop-zone').dispatchEvent(new DragEvent('drop', { dataTransfer: transfer, bubbles: true, cancelable: true }));
            return true;
        `);
        await page.waitFor('failed load diagnostic', "!document.getElementById('error').hidden && document.getElementById('error').textContent.includes('broken.s3m')");
        await page.waitFor('keep-alive paused after a failed load', "document.querySelector('audio').paused && navigator.mediaSession.playbackState === 'paused'");
        const storeResults = await page.evaluate(`const { openPlaylistStore } = await import('./playlist-store.js'); return await (${verifyPlaylistStore.toString()})(openPlaylistStore);`);
        console.log(`playlist-store[${mode}]: ${storeResults}`);
        // W11: module and two-member ZIP uploads, persisted current id/loop and
        // disconnected library state, then actual audio activation of every upload.
        await filter('title:Loose', '1 of 5');
        await page.waitFor('loose row painted', "document.querySelector('[data-row-id=\"0\"]') !== null");
        await clickRow(0);
        await page.evaluate("document.getElementById('library-add-selected').click();");
        await page.evaluate(`
            const files = new DataTransfer();
            files.items.add(new File([await (await fetch('/library/loose/REFLEX.S3M')).arrayBuffer()], 'REFLEX.S3M'));
            const input = document.getElementById('playlist-files'); input.files = files.files; input.dispatchEvent(new Event('change'));
            const dropped = new DataTransfer();
            dropped.items.add(new File([await (await fetch('/library/disks/disk1.zip')).arrayBuffer()], 'disk.zip'));
            document.getElementById('playlist-panel').dispatchEvent(new DragEvent('drop', { dataTransfer: dropped, bubbles: true, cancelable: true }));
        `);
        await page.waitFor('three uploaded entries', "document.getElementById('playlist-count').textContent === '4' && document.getElementById('playlist-status').textContent.includes('disk.zip')");
        await page.evaluate("document.getElementById('playlist-loop').checked = true; document.getElementById('playlist-loop').dispatchEvent(new Event('change')); document.querySelectorAll('#playlist-list li')[2].dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true}));");
        await page.waitFor('upload current', "document.querySelectorAll('#playlist-list li')[2].classList.contains('current')");
        await page.waitFor('current entry persisted', "await (async () => { const { openPlaylistStore } = await import('./playlist-store.js'); const store = await openPlaylistStore(); const saved = store.state; store.close(); return saved.entries.length === 4 && saved.currentId === document.querySelectorAll('#playlist-list li')[2].dataset.entryId && saved.loop; })()");
        await page.send('Page.reload');
        await page.waitFor('restored playlist', "document.documentElement.dataset.playerReady === 'true' && document.getElementById('playlist-count').textContent === '4'");
        assert.equal(await page.evaluate("return document.querySelectorAll('#playlist-list li')[2].classList.contains('current') && document.getElementById('playlist-loop').checked;"), true);
        assert.match(await page.evaluate("return document.querySelector('#playlist-list .playlist-marker').textContent;"), /library not connected/);
        await page.evaluate(`if (${JSON.stringify(mode)} === 'fallback') document.getElementById('force-fallback').checked = true;`);
        for (const [index, title] of [[1, 'reflex'], [2, 'petrified'], [3, 'reflex']]) {
            await page.evaluate(`document.querySelectorAll('#playlist-list li')[${index}].dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true}));`);
            await page.waitFor('restored upload plays', `document.querySelectorAll('#playlist-list li')[${index}].classList.contains('current') && document.getElementById('title').textContent.toLowerCase().includes('${title}') && document.getElementById('transport-chip').textContent === 'playing'`);
        }
        await page.evaluate("document.getElementById('library-server').click();");
        await page.waitFor('library reconnected', "!document.querySelector('#playlist-list .playlist-marker').textContent.includes('library not connected')");
        await page.evaluate(`
            const dropped = new DataTransfer();
            dropped.items.add(new File([await (await fetch('/library/disks/outer.zip')).arrayBuffer()], 'outer.zip'));
            document.getElementById('playlist-panel').dispatchEvent(new DragEvent('drop', { dataTransfer: dropped, bubbles: true, cancelable: true }));
        `);
        await page.waitFor('nested upload added', "document.getElementById('playlist-count').textContent === '5' && document.getElementById('playlist-status').textContent.includes('outer.zip')");
        await page.evaluate("document.querySelector('#playlist-list li:last-child').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));");
        await page.waitFor('nested upload plays', "document.querySelector('#playlist-list li:last-child').classList.contains('current') && document.getElementById('title').textContent === 'Petrified'");


        assert.deepEqual(page.consoleErrors, []);
        return { playlistSnapshots, stoppedFade: { flags: stoppedFade.songFlags, frame: stoppedFade.songFrame }, firstPaint, filterMilliseconds, scroll, missing: missing.errorText, nestedTitle: beforeMissing.title };
    } finally {
        if (page) await page.close();
        server?.stop();
        await rm(directory, { recursive: true, force: true });
    }
}

async function runRemoteLibrary(executable, mode) {
    let server, page, upstream, proxy;
    let cors = true;
    let upstreamRequests = 0, proxyRequests = 0, agentRequests = 0;
    const listen = server => new Promise(resolve => server.listen(0, '127.0.0.1', () => resolve(`http://127.0.0.1:${server.address().port}/`)));
    const close = server => server ? new Promise(resolve => { server.close(resolve); server.closeAllConnections(); }) : Promise.resolve();
    try {
        const reflex = await readFile(join(REPOSITORY_ROOT, 'crates/starplayer-s3m/tests/fixtures/REFLEX.S3M'));
        const petri = await readFile(join(REPOSITORY_ROOT, 'crates/starplayer-s3m/tests/fixtures/PETRI.S3M'));
        const zip = buildZip([{ name: 'PETRI.S3M', bytes: petri }, { name: 'REFLEX.S3M', bytes: reflex }]);
        upstream = createServer((request, response) => {
            upstreamRequests += 1;
            if (request.headers['user-agent']?.startsWith('StarPlayer-proxy')) agentRequests += 1;
            if (cors) {
                response.setHeader('Access-Control-Allow-Origin', '*');
                response.setHeader('Cross-Origin-Resource-Policy', 'cross-origin');
            }
            response.setHeader('Cache-Control', 'no-store');
            const bytes = request.url === '/loose/REFLEX.S3M' ? reflex : request.url === '/disks/disk1.zip' ? zip : null;
            response.writeHead(bytes ? 200 : 404, { 'Content-Length': bytes?.length ?? 0 });
            response.end(bytes);
        });
        const base = await listen(upstream);
        server = await startDevServer(mode !== 'plain');
        const origin = `http://127.0.0.1:${server.port}`;
        proxy = createServer(async (request, response) => {
            proxyRequests += 1;
            try {
                const result = await remoteWorker.fetch(new Request(`http://${request.headers.host}${request.url}`, { method: request.method, headers: request.headers }), { UPSTREAM: base, ALLOWED_ORIGINS: origin });
                response.writeHead(result.status, Object.fromEntries(result.headers));
                response.end(Buffer.from(await result.arrayBuffer()));
            } catch (error) { response.writeHead(500); response.end(error.message); }
        });
        const proxyBase = await listen(proxy);
        page = await Page.open(executable, origin + '/');
        await page.waitFor('remote startup', "document.documentElement.dataset.playerReady === 'true'");
        // Exercise a genuine v1 -> v2 upgrade using the same database and public helpers.
        await page.evaluate(`
            await new Promise((resolve, reject) => { const request = indexedDB.deleteDatabase('starplayer-library'); request.onsuccess = resolve; request.onerror = () => reject(request.error); });
            const database = await new Promise((resolve, reject) => { const request = indexedDB.open('starplayer-library', 1); request.onupgradeneeded = () => request.result.createObjectStore('handles'); request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error); });
            await new Promise(resolve => { const transaction = database.transaction('handles', 'readwrite'); transaction.objectStore('handles').put({ name: 'preserved root' }, 'root'); transaction.oncomplete = resolve; });
            database.close();
            const { storedDirectoryHandle } = await import('./library-source.js');
            if ((await storedDirectoryHandle()).name !== 'preserved root') throw new Error('v1 handle lost');
        `);
        const csv = `source_path,member_path,title,source_url,sha256\nloose/REFLEX.S3M,,Loose Reflex,${base}loose/REFLEX.S3M,${'1'.repeat(64)}\ndisks/disk1.zip,PETRI.S3M,Disk Petri,${base}disks/disk1.zip,\ndisks/disk1.zip,REFLEX.S3M,Disk Reflex,${base}disks/disk1.zip,`;
        await page.evaluate(`
            if (${JSON.stringify(mode)} === 'fallback') document.getElementById('force-fallback').checked = true;
            const transfer = new DataTransfer(); transfer.items.add(new File([${JSON.stringify(csv)}], 'remote-synthetic.csv'));
            const input = document.getElementById('library-index'); input.files = transfer.files; input.dispatchEvent(new Event('change'));
        `);
        await page.waitFor('stream offered', "!document.getElementById('library-stream').hidden && !document.getElementById('library-proxy-field').hidden");
        await page.evaluate("document.getElementById('library-stream').click();");
        await page.waitFor('remote rows', "document.getElementById('library-count').textContent === '3 of 3' && document.getElementById('library-summary').textContent.includes('streaming 3 tracks from 127.0.0.1')");
        const play = async (id, title) => {
            await page.waitFor('remote row painted', `document.querySelector('[data-row-id="${id}"]') !== null`);
            await page.evaluate(`document.querySelector('[data-row-id="${id}"]').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));`);
            await page.waitFor('remote playback', `document.getElementById('title').textContent.toLowerCase().includes(${JSON.stringify(title.toLowerCase())}) && document.getElementById('transport-chip').textContent === 'playing'`);
        };
        await play(0, 'reflex');
        await play(1, 'petrified');
        await page.evaluate(`
            const transfer = new DataTransfer(); transfer.items.add(new File(['sha256,instrument_names\\n${'1'.repeat(64)},Remote composer'], 'remote-comments.csv'));
            const input = document.getElementById('library-comments'); input.files = transfer.files; input.dispatchEvent(new Event('change'));
            document.querySelector('[data-row-id="0"]').dispatchEvent(new MouseEvent('click', { bubbles: true }));
            document.getElementById('library-add-selected').click();
        `);
        await page.waitFor('remote comments saved', "(await (await import('./library-source.js')).storedRemoteIndex())?.commentsName === 'remote-comments.csv'");
        await page.waitFor('remote playlist saved', "await (async () => { const store = await (await import('./playlist-store.js')).openPlaylistStore(); const saved = store.state.entries.length; store.close(); return saved === 1; })()");
        await page.evaluate(`
            const { storedRemoteIndex } = await import('./library-source.js');
            const record = await storedRemoteIndex();
            await storedRemoteIndex({ ...record, proxy: 'http://127.0.0.1:1/' });
            localStorage.setItem('starplayer.libraryProxy', '');
        `);
        await page.send('Page.reload');
        await page.waitFor('remote reopen', "document.documentElement.dataset.playerReady === 'true' && !document.getElementById('library-reopen').hidden");
        assert.equal(await page.evaluate("return document.getElementById('library-reopen').textContent;"), 'Reopen remote-synthetic.csv (streaming)');
        await page.evaluate(`if (${JSON.stringify(mode)} === 'fallback') document.getElementById('force-fallback').checked = true; document.getElementById('library-reopen').click();`);
        await page.waitFor('remote comments restored', "document.getElementById('library-summary').textContent.includes('1 tracks matched')");
        await page.evaluate("document.querySelector('#playlist-list li').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));");
        await page.waitFor('remote playlist plays', "document.getElementById('title').textContent.toLowerCase().includes('reflex') && document.getElementById('transport-chip').textContent === 'playing'");
        cors = false;
        await page.evaluate(`const input = document.getElementById('library-proxy'); input.value = ${JSON.stringify(proxyBase.slice(0, -1))}; input.dispatchEvent(new Event('change'));`);
        await play(1, 'petrified');
        assert.ok(proxyRequests > 0 && agentRequests === proxyRequests, 'proxy user agent reached upstream');
        assert.ok(upstreamRequests > proxyRequests, 'direct requests also reached upstream');
        await page.waitFor('proxy setting saved', `(await (await import('./library-source.js')).storedRemoteIndex())?.proxy === ${JSON.stringify(proxyBase)}`);
        await page.evaluate("localStorage.removeItem('starplayer.libraryProxy');");
        await page.send('Page.reload');
        await page.waitFor('proxy reopen available', "document.documentElement.dataset.playerReady === 'true' && !document.getElementById('library-reopen').hidden");
        await page.evaluate(`if (${JSON.stringify(mode)} === 'fallback') document.getElementById('force-fallback').checked = true; document.getElementById('library-reopen').click();`);
        await page.waitFor('stored proxy restored', `document.getElementById('library-proxy').value === ${JSON.stringify(proxyBase)} && document.getElementById('library-count').textContent === '3 of 3'`);
        await play(1, 'petrified');
        // Clearing the proxy must also clear the ZIP cache: this member used to be cached.
        await page.evaluate("const input = document.getElementById('library-proxy'); input.value = ''; input.dispatchEvent(new Event('change'));");
        await page.evaluate("document.querySelector('[data-row-id=\"2\"]').dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));");
        await page.waitFor('CORS failure diagnostic', "document.getElementById('error').textContent.includes('may not allow cross-origin requests — set a Proxy URL')");
        const surviving = await page.evaluate(READ_STATE);
        assert.equal(surviving.title, 'Petrified');
        assert.equal(surviving.chip, 'playing');
        await page.waitFor('remote playback survives CORS failure', `document.getElementById('row').textContent !== ${JSON.stringify(surviving.row)}`);
        // Chromium logs the intentionally blocked CORS fetch. All other errors fail.
        assert.deepEqual(page.consoleErrors.filter(error => !/CORS policy|net::ERR_FAILED/.test(error)), []);
        return { upstreamRequests, proxyRequests, agentRequests, reopenedComments: true, upgradedHandle: true, failure: surviving.errorText };
    } finally {
        if (page) await page.close();
        server?.stop();
        await close(proxy);
        await close(upstream);
    }
}

const executable = await findChromium();
if (executable === null) {
    const required = process.env.STARPLAYER_REQUIRE_CHROMIUM === '1';
    console.log(`headless: ${required ? 'failed' : 'skipped'} — no Chromium found (set CHROME=/path/to/chrome to run it)`);
    process.exit(required ? 1 : 0);
}

await verifyPackagedSources();
if (options.pagesSmoke) {
    console.log(`pages-smoke ${JSON.stringify(await runPagesSmoke({ Page, executable, dist: DIST, syntheticMod, buildZip }))}`);
} else for (const mode of options.modes) {
    if (!options.libraryOnly && !options.w15Only) {
        const report = await run(executable, mode);
        console.log(`headless[${mode}] ${JSON.stringify(report, null, 2)}`);
    }
    if (options.w15Case === 'background') console.log(`w15-background[${mode}] ${JSON.stringify(await runBackground({ Page, executable, mode, startDevServer }))}`);
    if (options.w15Case === 'enumeration') console.log(`w15-enumeration[${mode}] ${JSON.stringify(await runEnumerationDiagnostic({ Page, executable, mode, startDevServer }))}`);
    if (options.w15Case === 'visibility') console.log(`w15-visibility[${mode}] ${JSON.stringify(await runVisibilityReturn({ Page, executable, mode, startDevServer }))}`);
    if (options.w15Case === 'performance') console.log(`w15-performance[${mode}] ${JSON.stringify(await runPerformance({ Page, executable, mode, startDevServer }))}`);
    if (options.w15Case === 'digest') console.log(`w15-digest[${mode}] ${JSON.stringify(await runDigestMeasurements({ Page, executable, mode, startDevServer, syntheticMod, buildZip }))}`);
    if (!options.w15Case || options.w15Case === 'remediation') console.log(`w15-remediation[${mode}] ${JSON.stringify(await runRemediation({ Page, executable, mode, startDevServer, syntheticMod, buildZip }))}`);
    if (!options.w15Case || options.w15Case === 'app') console.log(`w15[${mode}] ${JSON.stringify(await runW15({ Page, executable, mode, startDevServer, syntheticMod, buildZip, verifyPlaylistStore }))}`);
    if (!options.w15Case || options.w15Case === 'view') console.log(`w15-dom[${mode}] ${JSON.stringify(await runW15View({ Page, executable, mode, startDevServer }))}`);
    if (!options.w15Case || options.w15Case === 'stress') console.log(`w15-stress[${mode}] ${JSON.stringify(await runW15Stress({ Page, executable, mode, startDevServer }))}`);
    if (!options.w15Case || options.w15Case === 'playlist') console.log(`w15-playlist[${mode}] ${JSON.stringify(await runW15Playlist({ Page, executable, mode, startDevServer, syntheticMod }))}`);
    if (!options.w15Case || options.w15Case === 'runtime') console.log(`w15-runtime[${mode}] ${JSON.stringify(await runW15Runtime({ Page, executable, mode, startDevServer, syntheticMod }))}`);
    if (!options.w15Only) console.log(`remote-library[${mode}] ${JSON.stringify(await runRemoteLibrary(executable, mode))}`);
    if (!options.w15Only) console.log(`library[${mode}] ${JSON.stringify(await runLibrary(executable, mode))}`);
}
console.log(options.pagesSmoke ? 'headless harness: Pages smoke passed' : 'headless harness: all requested modes passed');
