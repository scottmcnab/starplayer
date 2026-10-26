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
    } else if (kind === 'member') {
        valid = payload && typeof payload.sha256 === 'string' && /^[0-9a-f]{64}$/.test(payload.sha256);
        if (valid) {
            const metadata = payload.metadata;
            valid = textFields(metadata, ['format', 'title', 'instrumentNames', 'sampleNames', 'songMessage'])
                && /^(mod|s3m|mtm|xm|it)$/.test(metadata.format) && Number.isSafeInteger(metadata.channels) && metadata.channels > 0;
        }
    }
    if (!valid) throw libraryWorkerError('job-failed', `Invalid ${kind} result payload`);
    return payload;
}

const wasmTrap = error => error instanceof WebAssembly.RuntimeError || error?.name === 'RuntimeError';
const failure = (error, code, cause) => ({ name: 'Error', code, message: String(error?.message ?? error), ...(cause === undefined ? {} : { cause }) });

export async function executeLibraryJob(job, { loader, digest = bytes => globalThis.crypto.subtle.digest('SHA-256', bytes), onMember = () => {} }) {
    let bytes = new Uint8Array(job.bytes);
    job.bytes = null;
    if (bytes.byteLength > MAX_SOURCE_BYTES) throw libraryWorkerError('source-too-large', `Source exceeds ${MAX_SOURCE_BYTES} bytes`);
    if (job.kind === 'catalog') return validateLibraryPayload('catalog', JSON.parse(loader.archive_catalog(bytes)));
    if (job.kind !== 'inspect-group') throw libraryWorkerError('job-failed', 'Unknown library worker job');
    let session;
    let trapped = false;
    try {
        if (job.members.some(member => member.locator.memberPath)) {
            session = new loader.ArchiveInspectionSession(bytes);
            // Rust owns the snapshot now. Release the transferred JS outer buffer before hashing.
            bytes = null;
        }
        for (const { token, locator } of job.members) {
            let result;
            try {
                const moduleBytes = session ? session.extract_member(locator.containerMember || '', locator.memberPath, locator.rawHex || '') : bytes;
                if (session && moduleBytes.byteLength > MAX_EXTRACTED_BYTES) throw libraryWorkerError('source-too-large', `Extracted module exceeds ${MAX_EXTRACTED_BYTES} bytes`);
                loader.inspect_s3m(moduleBytes);
                // Capture the accessor before hashing: the loader's current module is mutable.
                const metadata = JSON.parse(loader.module_metadata());
                const sha256 = await hash(moduleBytes, digest);
                result = { token, payload: validateLibraryPayload('member', { metadata, sha256 }) };
            } catch (error) {
                if (wasmTrap(error)) throw error;
                result = { token, error: failure(error, error?.code === 'source-too-large' ? 'source-too-large' : 'job-failed') };
            }
            onMember(result);
        }
        return {};
    } catch (error) {
        trapped = wasmTrap(error);
        throw error;
    } finally {
        // A trap can poison wasm-bindgen's borrow guard. Worker retirement releases the instance.
        if (!trapped) session?.free();
    }
}

async function hash(bytes, digest) {
    const result = new Uint8Array(await digest(bytes));
    return Array.from(result, byte => byte.toString(16).padStart(2, '0')).join('');
}

// Keep the browser entry point thin; the same endpoint is exercised with controlled bindings.
export async function startLibraryWorker({ scope, loadLoader }) {
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
        const tag = { instanceId: job.instanceId, id: job.id, generation: job.generation, attempt: job.attempt, kind: job.kind };
        try {
            const payload = await executeLibraryJob(job, { loader, onMember: result => scope.postMessage({ type: 'member-result', ...tag, ...result }) });
            scope.postMessage({ type: job.kind === 'inspect-group' ? 'group-result' : 'result', ...tag, payload });
        } catch (error) {
            const trapped = wasmTrap(error);
            const code = trapped ? 'worker-failed' : error?.code === 'source-too-large' ? 'source-too-large' : 'job-failed';
            scope.postMessage({ type: 'error', ...tag, error: failure(error, code, trapped ? 'wasm-trap' : undefined) });
        }
    };
}
