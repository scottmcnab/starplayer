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
//
// Exits 0 and prints `headless: skipped` when no Chromium is installed.

import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { access, readFile, readdir } from 'node:fs/promises';
import { createServer } from 'node:http';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { crc32, deflateRawSync } from 'node:zlib';

const REPOSITORY_ROOT = fileURLToPath(new URL('../../../', import.meta.url));
const DEV_SERVER = join(REPOSITORY_ROOT, 'apps/starplayer-web/dev-server.mjs');
const DIST = join(REPOSITORY_ROOT, 'apps/starplayer-web/dist');
const FIRST_MODULE = 'REFLEX.S3M';
const SECOND_MODULE = 'MOVEMENT.S3M';
const PHONE_VIEWPORT = { width: 390, height: 844 };

const options = parseArguments(process.argv.slice(2));

function parseArguments(argv) {
    const parsed = { seconds: 60, modes: ['sab', 'fallback', 'plain'] };
    for (let index = 0; index < argv.length; index += 1) {
        if (argv[index] === '--seconds') {
            parsed.seconds = Number(argv[index + 1]);
            index += 1;
        } else if (argv[index] === '--mode') {
            parsed.modes = [argv[index + 1]];
            index += 1;
        } else {
            throw new Error(`unexpected argument \`${argv[index]}\``);
        }
    }
    return parsed;
}

// ── environment ─────────────────────────────────────────────────────────────────────

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
        const compressed = deflateRawSync(bytes);
        const checksum = crc32(bytes) >>> 0;

        const localHeader = Buffer.alloc(30);
        localHeader.writeUInt32LE(0x04034B50, 0);
        localHeader.writeUInt16LE(20, 4);
        localHeader.writeUInt16LE(0x0800, 6);
        localHeader.writeUInt16LE(8, 8);
        localHeader.writeUInt32LE(checksum, 14);
        localHeader.writeUInt32LE(compressed.length, 18);
        localHeader.writeUInt32LE(bytes.length, 22);
        localHeader.writeUInt16LE(name.length, 26);
        localRecords.push(localHeader, name, compressed);

        const centralHeader = Buffer.alloc(46);
        centralHeader.writeUInt32LE(0x02014B50, 0);
        centralHeader.writeUInt16LE(20, 4);
        centralHeader.writeUInt16LE(20, 6);
        centralHeader.writeUInt16LE(0x0800, 8);
        centralHeader.writeUInt16LE(8, 10);
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
/// modules that are distinguishable in telemetry by their channel count alone.
function syntheticMod({ channels = 4, tag = 'M.K.', title = 'headphone test' } = {}) {
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
    for (let index = headerBytes + patternBytes * 2; index < bytes.length; index += 1) {
        bytes[index] = index & 1 ? 0x80 : 0x7F;
    }
    return bytes;
}

async function startDevServer(isolate) {
    const port = await freePort();
    const argv = [DEV_SERVER, '--port', String(port), '--root', DIST];
    if (!isolate) argv.push('--no-isolation');
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
        const browser = spawn(executable, [
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
        ], { stdio: ['ignore', 'ignore', 'ignore'] });

        let version = null;
        for (let attempt = 0; attempt < 100 && version === null; attempt += 1) {
            await delay(100);
            version = await fetch(`http://127.0.0.1:${port}/json/version`).then((r) => r.json()).catch(() => null);
        }
        if (version === null) {
            browser.kill('SIGKILL');
            throw new Error('Chromium never opened its DevTools endpoint');
        }
        const target = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, { method: 'PUT' })
            .then((response) => response.json());
        const socket = new WebSocket(target.webSocketDebuggerUrl);
        await new Promise((resolve, reject) => {
            socket.addEventListener('open', resolve, { once: true });
            socket.addEventListener('error', () => reject(new Error('could not attach to the page')), { once: true });
        });
        const page = new Page(socket);
        page.browser = browser;
        page.version = version.Browser;
        await page.send('Runtime.enable');
        await page.send('Log.enable');
        await page.send('Page.enable');
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
            this.consoleErrors.push(message.params.entry.text);
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
                const shown = await this.evaluate("return document.getElementById('message').textContent + ' | ' + document.getElementById('error').textContent;").catch((error) => `unreadable (${error.message})`);
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
        await page.evaluate("localStorage.setItem('starplayer.output-and-mixer.v1', JSON.stringify({ path: 'float' })); return true;");
        await page.send('Page.reload', { ignoreCache: true });
        await page.waitFor('the reloaded page module', "document.documentElement.dataset.playerReady === 'true'");
        assert.equal(await page.evaluate("return document.getElementById('mod-headphone-panning').checked;"), false, 'an older v1 preference object restores as unchecked');
        if (mode === 'fallback') {
            await page.evaluate("document.getElementById('force-fallback').checked = true; return true;");
        }

        await page.evaluate("document.getElementById('start-audio').click(); return true;");
        await page.waitFor('the AudioWorklet', "document.getElementById('transport-chip').textContent === 'audio ready'");

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

        // ── Repeat off fades out at the loop point and stops, resetting to 0:00 ─────
        await page.evaluate("const repeat = document.getElementById('repeat'); repeat.checked = false; repeat.dispatchEvent(new Event('change')); return true;");
        const rate = rateDigits(settledSliderSeek.workletRate);
        await page.waitFor('the slider to grow by the fade with Repeat off', `Number(document.getElementById('progress').max) > ${settledSliderSeek.progressMax}`);
        const repeatOff = await page.evaluate(READ_STATE);
        assert.equal(repeatOff.progressMax, settledSliderSeek.progressMax + 5 * rate, 'with Repeat off the slider covers the five-second fade as well');
        const nearEndFrame = Math.max(0, settledSliderSeek.progressMax - rate);
        await page.evaluate(`
            const progress = document.getElementById('progress');
            progress.value = '${nearEndFrame}';
            progress.dispatchEvent(new Event('change'));
            return true;
        `);
        await page.waitFor('the transport to stop after the fade-out', "document.getElementById('transport-chip').textContent === 'stopped'", 30_000);
        const afterFadeStop = await page.evaluate(READ_STATE);
        assert.equal(afterFadeStop.chip, 'stopped', 'Repeat off fades out and stops at the loop point instead of wrapping');
        assert.equal(afterFadeStop.elapsed, '0:00', 'elapsed reads 0:00 once the fade-out stop lands');
        assert.equal(afterFadeStop.progressValue, 0, 'the slider rewinds to 0 once the fade-out stop lands');
        report.fadeStop = `stopped at ${afterFadeStop.elapsed}`;

        // Leave Repeat checked and the transport playing again, as the rest of this run
        // (and the next mode's run of the same page) expects.
        await page.evaluate("const repeat = document.getElementById('repeat'); repeat.checked = true; repeat.dispatchEvent(new Event('change')); return true;");
        await page.evaluate("document.getElementById('play').click(); return true;");
        await page.waitFor('playback to resume after the fade-out test', "document.getElementById('transport-chip').textContent === 'playing'");

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
        assert.match(pickerEntries[1], /MOVEMENT\.S3M/);
        await page.waitFor('playback to advance behind the picker', `document.getElementById('row').textContent !== ${JSON.stringify(pickerState.row)}`);
        await page.evaluate("document.getElementById('archive-cancel').click(); return true;");
        const afterCancel = await page.evaluate(READ_STATE);
        assert.equal(afterCancel.archivePickerVisible, false);
        assert.equal(afterCancel.moduleDetail, beforePicker.moduleDetail, 'Cancel leaves the current module unchanged');
        await loadFile(page, twoModuleZip, 'two-modules.zip');
        await page.waitFor('the reopened archive picker', "document.getElementById('archive-picker').hidden === false");
        await page.evaluate("document.getElementById('archive-entries').selectedIndex = 1; document.getElementById('archive-load').click(); return true;");
        await page.waitFor('the selected second ZIP module', "document.getElementById('module-detail').textContent.includes('MOVEMENT.S3M (from two-modules.zip)')");
        await page.waitFor('the retired module to be collected', "parseInt(document.getElementById('retired').textContent, 10) >= 1");
        await delay(2000);
        const swapped = await page.evaluate(READ_STATE);
        assert.equal(swapped.errorShown, false, `the ZIP hot swap raised an error: ${swapped.errorText}`);
        assert.equal(swapped.health, 'healthy');
        report.pickerEntries = pickerEntries;
        report.multiZipLabel = swapped.moduleDetail.split(' · ')[0];
        report.retiredAfterSwap = swapped.retired;
        report.secondTitle = swapped.title;

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
        assert.match(trackPicker.selected, /MOVEMENT\.S3M/, 'the playing entry is selected in the dropdown');

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

        // ── phone width ─────────────────────────────────────────────────────────────
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
            };
        `);
        assert.ok(layout.scrollWidth <= layout.innerWidth, `the document overflows at ${PHONE_VIEWPORT.width}px: ${layout.scrollWidth} > ${layout.innerWidth}`);
        assert.ok(layout.bodyScrollWidth <= layout.innerWidth, `the body overflows at ${PHONE_VIEWPORT.width}px: ${layout.bodyScrollWidth}`);
        assert.ok(layout.controlsVisible, 'the transport controls are still laid out at phone width');
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

const executable = await findChromium();
if (executable === null) {
    console.log('headless: skipped — no Chromium found (set CHROME=/path/to/chrome to run it)');
    process.exit(0);
}

for (const mode of options.modes) {
    const report = await run(executable, mode);
    console.log(`headless[${mode}] ${JSON.stringify(report, null, 2)}`);
}
console.log('headless harness: all modes passed');
