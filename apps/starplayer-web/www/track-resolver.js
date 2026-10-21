import { checkAbort } from './library-source.js';

export function createResolver({ source, loader, blobStore, cacheBytes = 64 * 1024 * 1024, maxFileBytes = 128 * 1024 * 1024 }) {
    const cache = new Map();
    let totalBytes = 0;
    let generation = 0;
    let queue = Promise.resolve();
    const enqueue = operation => {
        const result = queue.then(operation);
        queue = result.catch(() => {});
        return result;
    };
    const copyBytes = bytes => bytes instanceof ArrayBuffer ? new Uint8Array(bytes).slice() : new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength).slice();
    async function execute(locator, signal, warmOnly) {
        try {
            checkAbort(signal);
            if (locator.kind === 'upload' && blobStore) {
                if (warmOnly) return;
                const blob = await blobStore.getBlob(locator.blobId);
                checkAbort(signal);
                if (!blob) throw new Error('missing — re-add');
                if (blob.size > maxFileBytes) throw new Error(`too large (${blob.size} bytes; limit ${maxFileBytes})`);
                const bytes = new Uint8Array(await blob.bytes.arrayBuffer());
                checkAbort(signal);
                if (bytes.byteLength > maxFileBytes) throw new Error('too large');
                const result = locator.memberIndex < 0 ? bytes : await loader.archive_extract_nested(bytes, locator.containerIndex, locator.memberIndex);
                checkAbort(signal);
                return copyBytes(result).buffer;
            }
            if (locator.kind !== 'library') throw new Error(`${locator.kind} locator not supported by this resolver`);
            if (warmOnly && !locator.memberPath) return;
            const currentGeneration = generation;
            const size = await source.size(locator.sourcePath);
            checkAbort(signal);
            if (size !== null && size > maxFileBytes) throw new Error(`too large (${size} bytes; limit ${maxFileBytes})`);
            let bytes = locator.memberPath ? cache.get(locator.sourcePath) : null;
            if (bytes) {
                cache.delete(locator.sourcePath);
                cache.set(locator.sourcePath, bytes);
            } else {
                const result = await source.readFile(locator.sourcePath, { signal });
                checkAbort(signal);
                if (result.byteLength > maxFileBytes) throw new Error(`too large (${result.byteLength} bytes; limit ${maxFileBytes})`);
                bytes = copyBytes(result);
                if (locator.memberPath && bytes.byteLength <= cacheBytes && currentGeneration === generation) {
                    while (totalBytes + bytes.byteLength > cacheBytes && cache.size) {
                        const oldest = cache.keys().next().value;
                        totalBytes -= cache.get(oldest).byteLength;
                        cache.delete(oldest);
                    }
                    cache.set(locator.sourcePath, bytes);
                    totalBytes += bytes.byteLength;
                }
            }
            if (warmOnly) return;
            if (!locator.memberPath) return bytes.buffer;
            // wasm-bindgen copies a `&[u8]` argument into wasm memory and never writes back, so
            // the cached archive is passed as is rather than duplicated per extraction.
            const extracted = await loader.archive_extract_path(bytes, locator.containerMember || '', locator.memberPath, locator.rawHex || '');
            checkAbort(signal);
            return copyBytes(extracted).buffer;
        } catch (error) {
            if (error.name === 'AbortError') throw error;
            const row = [locator.sourcePath, locator.containerMember, locator.memberPath].filter(Boolean).join(' → ');
            throw new Error(`${row || locator.kind}: ${error.message ?? error}`, { cause: error });
        }
    }
    return {
        resolve: (locator, { signal } = {}) => enqueue(() => execute(locator, signal, false)),
        prefetch: locator => enqueue(() => execute(locator, undefined, true)).catch(() => {}),
        clear() { generation += 1; cache.clear(); totalBytes = 0; },
    };
}
