// A4-N1's Chrome sender. Classic (non-module) script, deliberately: it is loaded right
// after `cast_sender.js?loadCastFramework=1`, and `window.__onGCastApiAvailable` has to be
// in place before that script's own async framework bootstrap finishes — a classic script
// right after another classic script is the one ordering guarantee that needs no timing
// assumptions (both are parser-blocking and run in document order; the alternative, a
// deferred module script, could run either before or after the async callback fires).
//
// It needs no COOP/COEP: it neither instantiates the wasm host nor uses
// `SharedArrayBuffer`. `coi-serviceworker.js` is deliberately not included here — see
// `apps/starplayer-cast-probe/README.md`.
'use strict';

const NAMESPACE = 'urn:x-cast:com.starplayer.probe';
const APP_ID_STORAGE_KEY = 'starplayer-cast-probe-app-id';
const DEFAULT_QUANTA = 512;
const DEFAULT_CHUNK_SIZE = 65536;

const elements = {
    appId: document.getElementById('app-id'),
    connectionStatus: document.getElementById('connection-status'),
    probeButton: document.getElementById('probe-button'),
    quanta: document.getElementById('quanta'),
    benchButton: document.getElementById('bench-button'),
    fileInput: document.getElementById('file-input'),
    chunkSize: document.getElementById('chunk-size'),
    sendFileButton: document.getElementById('send-file-button'),
    findLimitButton: document.getElementById('find-limit-button'),
    transcript: document.getElementById('transcript'),
    copyButton: document.getElementById('copy-button'),
};

elements.appId.value = window.localStorage ? (localStorage.getItem(APP_ID_STORAGE_KEY) ?? '') : '';
elements.appId.addEventListener('change', () => {
    const appId = elements.appId.value.trim();
    try {
        localStorage.setItem(APP_ID_STORAGE_KEY, appId);
    } catch (error) {
        // Storage blocked (private tab, quota) — the page still works, it just forgets.
    }
    applyAppId();
});

let castContext = null;
let listenedSession = null;
// Replies are delivered in the order they were sent (one namespace, one reliable
// channel), so a simple FIFO queue of waiters — one per in-flight request — is enough to
// match each reply to the call that is awaiting it, without tagging messages with an id
// the receiver does not add.
let pendingReplies = [];

function appendTranscript(direction, payload) {
    const rendered = typeof payload === 'string' ? payload : JSON.stringify(payload, null, 2);
    const line = `── ${direction} · ${new Date().toLocaleTimeString()} ──\n${rendered}\n`;
    elements.transcript.textContent += (elements.transcript.textContent ? '\n' : '') + line;
    elements.transcript.scrollTop = elements.transcript.scrollHeight;
}

function nextReply() {
    let resolve;
    const promise = new Promise((resolveFn) => {
        resolve = resolveFn;
    });
    pendingReplies.push(resolve);
    const cancel = () => {
        const index = pendingReplies.indexOf(resolve);
        if (index !== -1) pendingReplies.splice(index, 1);
    };
    return { promise, cancel };
}

function timeoutAfter(milliseconds) {
    return new Promise((_, reject) => setTimeout(() => reject(new Error(`timed out after ${milliseconds}ms`)), milliseconds));
}

function onReceiverMessage(_namespace, message) {
    let parsed = message;
    if (typeof message === 'string') {
        try {
            parsed = JSON.parse(message);
        } catch (error) {
            appendTranscript('receiver (unparseable)', message);
            return;
        }
    }
    appendTranscript('receiver', parsed);
    const waiter = pendingReplies.shift();
    if (waiter) waiter(parsed);
}

function updateConnectionStatus() {
    const session = castContext ? castContext.getCurrentSession() : null;
    const connected = session !== null && session !== undefined;
    elements.connectionStatus.textContent = connected
        ? `connected: ${session.getCastDevice().friendlyName}`
        : 'not connected';
    elements.probeButton.disabled = !connected;
    elements.benchButton.disabled = !connected;
    elements.sendFileButton.disabled = !connected;
    elements.findLimitButton.disabled = !connected;

    if (connected && session !== listenedSession) {
        session.addMessageListener(NAMESPACE, onReceiverMessage);
        listenedSession = session;
    }
    if (!connected) listenedSession = null;
}

function applyAppId() {
    const appId = elements.appId.value.trim();
    if (appId === '' || typeof cast === 'undefined') return;
    castContext = cast.framework.CastContext.getInstance();
    castContext.setOptions({
        receiverApplicationId: appId,
        autoJoinPolicy: chrome.cast.AutoJoinPolicy.ORIGIN_SCOPED,
    });
    castContext.addEventListener(cast.framework.CastContextEventType.SESSION_STATE_CHANGED, updateConnectionStatus);
    updateConnectionStatus();
}

async function sendMessage(message) {
    const session = castContext ? castContext.getCurrentSession() : null;
    if (!session) throw new Error('not connected to a receiver');
    appendTranscript('sender', message);
    await session.sendMessage(NAMESPACE, message);
}

window.__onGCastApiAvailable = function onGCastApiAvailable(isAvailable) {
    if (!isAvailable) {
        appendTranscript('sender (error)', 'the Cast API did not become available — is this Chrome?');
        return;
    }
    applyAppId();
};

elements.probeButton.addEventListener('click', async () => {
    try {
        await sendMessage({ type: 'probe' });
        const waiter = nextReply();
        await waiter.promise;
    } catch (error) {
        appendTranscript('sender (error)', String(error.message || error));
    }
});

elements.benchButton.addEventListener('click', async () => {
    try {
        const quanta = Number(elements.quanta.value) || DEFAULT_QUANTA;
        await sendMessage({ type: 'bench', quanta });
        const waiter = nextReply();
        await waiter.promise;
    } catch (error) {
        appendTranscript('sender (error)', String(error.message || error));
    }
});

function bytesToBase64(bytes) {
    let binary = '';
    const blockSize = 0x8000;
    for (let offset = 0; offset < bytes.length; offset += blockSize) {
        binary += String.fromCharCode(...bytes.subarray(offset, offset + blockSize));
    }
    return btoa(binary);
}

async function sendChunked(bytes, chunkSize, { label = 'chunk transfer', requireAck = true } = {}) {
    const total = Math.max(1, Math.ceil(bytes.length / chunkSize));
    let lastReply = null;
    for (let index = 0; index < total; index += 1) {
        const start = index * chunkSize;
        const slice = bytes.subarray(start, Math.min(start + chunkSize, bytes.length));
        await sendMessage({ type: 'chunk', index, total, size: slice.length, data: bytesToBase64(slice) });
        if (!requireAck) continue;
        const waiter = nextReply();
        lastReply = await waiter.promise;
        if (lastReply.type === 'error') throw new Error(`receiver rejected ${label} chunk ${index}: ${lastReply.message}`);
    }
    return lastReply;
}

elements.sendFileButton.addEventListener('click', async () => {
    const file = elements.fileInput.files[0];
    if (!file) {
        appendTranscript('sender (error)', 'no file chosen');
        return;
    }
    const chunkSize = Math.max(1024, Number(elements.chunkSize.value) || DEFAULT_CHUNK_SIZE);
    elements.sendFileButton.disabled = true;
    try {
        const bytes = new Uint8Array(await file.arrayBuffer());
        await sendChunked(bytes, chunkSize, { label: file.name });
    } catch (error) {
        appendTranscript('sender (error)', String(error.message || error));
    } finally {
        elements.sendFileButton.disabled = false;
    }
});

// Research point 2: walk the chunk size up until the receiver stops acknowledging, and
// report the last size that worked. Each attempt is a disposable one-chunk transfer of
// random bytes — never the file the owner picked — so it never pollutes `bench`'s
// "uploaded" module state.
elements.findLimitButton.addEventListener('click', async () => {
    elements.findLimitButton.disabled = true;
    let largestAcceptedBytes = 0;
    try {
        for (let size = 4 * 1024; size <= 16 * 1024 * 1024; size *= 2) {
            const probe = new Uint8Array(size);
            crypto.getRandomValues(probe.subarray(0, Math.min(size, 65536)));
            await sendMessage({ type: 'chunk', index: 0, total: 1, size: probe.length, data: bytesToBase64(probe) });
            const waiter = nextReply();
            let reply;
            try {
                reply = await Promise.race([waiter.promise, timeoutAfter(5000)]);
            } catch (error) {
                waiter.cancel();
                appendTranscript('sender (find-limit)', `no reply at ${size} bytes — stopping`);
                break;
            }
            if (reply.type === 'error') {
                appendTranscript('sender (find-limit)', `receiver rejected ${size} bytes: ${reply.message}`);
                break;
            }
            largestAcceptedBytes = size;
        }
    } catch (error) {
        appendTranscript('sender (error)', String(error.message || error));
    } finally {
        appendTranscript('sender (find-limit)', { largestAcceptedChunkBytes: largestAcceptedBytes });
        elements.findLimitButton.disabled = false;
    }
});

elements.copyButton.addEventListener('click', async () => {
    try {
        await navigator.clipboard.writeText(elements.transcript.textContent);
        appendTranscript('sender', 'transcript copied to clipboard');
    } catch (error) {
        appendTranscript('sender (error)', `clipboard copy failed: ${error.message || error}`);
    }
});
