// Shared worker payload checks keep binding/transport failures local to one job.
export const MAX_SOURCE_BYTES = 128 * 1024 * 1024;
export const MAX_EXTRACTED_BYTES = 64 * 1024 * 1024;

export function libraryWorkerError(code, message) {
    return Object.assign(new Error(String(message)), { name: code === 'aborted' || code === 'disposed' ? 'AbortError' : 'Error', code });
}

const textFields = (value, fields) => value && fields.every(field => typeof value[field] === 'string');
export function validateLibraryPayload(kind, payload) {
    let valid = false;
    if (kind === 'catalog') {
        valid = payload && Array.isArray(payload.entries) && Array.isArray(payload.issues)
            && payload.entries.every(entry => textFields(entry, ['containerMember', 'memberPath', 'rawHex', 'format', 'error'])
                && /^(mod|s3m|mtm|xm|it)$/.test(entry.format) && Number.isSafeInteger(entry.size) && entry.size >= 0 && typeof entry.playable === 'boolean')
            && payload.issues.every(issue => textFields(issue, ['path', 'reason']));
    } else if (kind === 'inspect' || kind === 'hash') {
        valid = payload && typeof payload.sha256 === 'string' && /^[0-9a-f]{64}$/.test(payload.sha256);
        if (valid && kind === 'inspect') {
            const metadata = payload.metadata;
            valid = textFields(metadata, ['format', 'title', 'instrumentNames', 'sampleNames', 'songMessage'])
                && /^(mod|s3m|mtm|xm|it)$/.test(metadata.format) && Number.isSafeInteger(metadata.channels) && metadata.channels > 0;
        }
    }
    if (!valid) throw libraryWorkerError('job-failed', `Invalid ${kind} result payload`);
    return payload;
}

export async function executeLibraryJob(job, { loader, digest = bytes => globalThis.crypto.subtle.digest('SHA-256', bytes) }) {
    let bytes = new Uint8Array(job.bytes);
    if (bytes.byteLength > MAX_SOURCE_BYTES) throw libraryWorkerError('source-too-large', `Source exceeds ${MAX_SOURCE_BYTES} bytes`);
    if (job.kind === 'catalog') return validateLibraryPayload('catalog', JSON.parse(loader.archive_catalog(bytes)));
    if (job.kind === 'inspect') {
        const locator = job.locator;
        if (locator.memberPath) {
            bytes = loader.archive_extract_path(bytes, locator.containerMember || '', locator.memberPath, locator.rawHex || '');
            if (bytes.byteLength > MAX_EXTRACTED_BYTES) throw libraryWorkerError('source-too-large', `Extracted module exceeds ${MAX_EXTRACTED_BYTES} bytes`);
        }
        loader.inspect_s3m(bytes);
        // Capture the accessor before awaiting hashing; this instance's loader is mutable.
        const metadata = JSON.parse(loader.module_metadata());
        const sha256 = await hash(bytes, digest);
        return validateLibraryPayload('inspect', { metadata, sha256 });
    }
    if (job.kind === 'hash') return validateLibraryPayload('hash', { sha256: await hash(bytes, digest) });
    throw libraryWorkerError('job-failed', 'Unknown library worker job');
}

async function hash(bytes, digest) {
    const result = new Uint8Array(await digest(bytes));
    return Array.from(result, byte => byte.toString(16).padStart(2, '0')).join('');
}

// Keep the browser entry point thin; the same endpoint is exercised with controlled bindings.
export async function startLibraryWorker({ scope, loadLoader }) {
    const failure = (error, code, cause) => ({ name: 'Error', code, message: String(error?.message ?? error), ...(cause === undefined ? {} : { cause }) });
    let loader;
    try {
        loader = await loadLoader();
        scope.postMessage({ type: 'ready' });
    } catch (error) {
        scope.postMessage({ type: 'startup-error', error: failure(error, 'worker-failed') });
        return;
    }
    scope.onmessage = async event => {
        const job = event.data;
        const tag = { id: job.id, generation: job.generation, kind: job.kind };
        try {
            const payload = await executeLibraryJob(job, { loader });
            scope.postMessage({ type: 'result', ...tag, payload });
        } catch (error) {
            const wasmTrap = error instanceof WebAssembly.RuntimeError || error?.name === 'RuntimeError';
            const code = wasmTrap ? 'worker-failed' : error?.code === 'source-too-large' ? 'source-too-large' : 'job-failed';
            scope.postMessage({ type: 'error', ...tag, error: failure(error, code, wasmTrap ? 'wasm-trap' : undefined) });
        }
    };
}
