// Playback intent is claimed before I/O. The activation adapter must check isCurrent
// after its own awaits, and before transferring bytes or committing UI state.
const SONG_FLAG_END_REACHED = 4; // ring.js snapshot ABI

export function createPlaybackController({ resolver, activate, ready = () => {}, readSnapshot, flushCommands = () => {}, stop = () => {}, repeat = () => true, onIntent = () => {}, captureContext = () => ({}), onTrackEnded = () => {}, clock = globalThis }) {
    const endedListeners = new Set([onTrackEnded]);
    let intent = 0;
    let controlRevision = 0;
    let abort = null;
    let current = null;
    let playlistMode = false;
    let blocked = false;
    let wasPlaying = false;
    let emitted = false;
    let sequence = -1;
    let engineFrame = null;
    let loadingToken = null;
    const controller = {
        get current() { return current; },
        get controlRevision() { return controlRevision; },
        get stopped() { return blocked; },
        get playlistMode() { return playlistMode; },
        // True while the newest intent is still resolving or activating; a cancel ends it.
        get loading() { return loadingToken === intent; },
        effectiveAtEnd: () => !playlistMode && repeat(),
        cancel() { intent += 1; abort?.abort(); wasPlaying = false; },
        async play(locator, { origin = 'file', entryId = null, folder = null, archiveLabel = null, label = locator.label || locator.memberPath || locator.sourcePath || 'module' } = {}) {
            controller.cancel();
            current = null;
            const token = intent;
            loadingToken = token;
            abort = new AbortController();
            const signal = abort.signal;
            const isCurrent = () => token === intent && !signal.aborted;
            playlistMode = origin === 'playlist';
            onIntent({ origin, entryId });
            // Preserve the originating locator/context across resolution and activation.
            // A later playlist selection or source re-open must not redirect enrichment.
            const capturedLocator = { ...locator, ...(locator.libraryIdentity ? { libraryIdentity: { ...locator.libraryIdentity } } : {}) };
            // Start audio synchronously within the user's gesture, even while fetching.
            let audioReady;
            try {
                const activationContext = { ...captureContext(capturedLocator), locator: capturedLocator, isCurrent, entryId, folder, archiveLabel };
                audioReady = Promise.resolve(ready());
                audioReady.catch(() => {});
                const buffer = locator.kind === 'bytes'
                    ? await (locator.read ? locator.read(signal) : locator.buffer)
                    : await resolver.resolve(capturedLocator, { signal });
                if (!isCurrent()) return { status: 'abandoned' };
                const activation = await activate(buffer, label, origin, { ...activationContext, audio: audioReady });
                if (!isCurrent() || !activation) return { status: 'abandoned' };
                current = { ...activation, origin, entryId };
                blocked = false;
                wasPlaying = false;
                emitted = false;
                sequence = -1;
                engineFrame = null;
                return { status: 'played', current };
            } catch (error) {
                if (!isCurrent() || error.name === 'AbortError') return { status: 'abandoned' };
                return { status: 'failed', error };
            } finally {
                if (loadingToken === token) loadingToken = null;
            }
        },
        stop() { controller.userControl('stop'); stop(); },
        onTrackEnded(listener) { endedListeners.add(listener); return () => endedListeners.delete(listener); },
        // Stop suppresses completion until Play. A seek only needs a fresh playing ->
        // stopped edge: scrubbing a playlist track must not stop the playlist advancing.
        userControl(action) {
            controlRevision += 1;
            if (action === 'play') { blocked = false; wasPlaying = false; emitted = false; }
            else if (action === 'seek' || action === 'pause') { wasPlaying = false; }
            else { blocked = true; controller.cancel(); }
        },
        // A graph rebuild/load-option reload preserves ownership and suppression, but
        // needs a fresh edge from snapshots of the new host/generation.
        rebind(graph, generation) {
            if (current) current = { ...current, graph, generation };
            wasPlaying = false;
            sequence = -1;
            engineFrame = null;
        },
        tick() {
            flushCommands();
            const observation = readSnapshot();
            const snapshot = observation?.snapshot;
            // `sequence` counts the engine's own publishes, but the host packs `playing` and
            // `engineFrame` live every quantum: the stop at a song's end arrives under the
            // sequence already seen. So a snapshot is new when either changed. Inequality,
            // not ordering, because `engineFrame` is an i32 that wraps after ~12 h.
            if (!current || !snapshot || observation.graph !== current.graph || snapshot.moduleGeneration !== current.generation) return;
            if (snapshot.sequence === sequence && snapshot.engineFrame === engineFrame) return;
            sequence = snapshot.sequence;
            engineFrame = snapshot.engineFrame;
            const ended = snapshot.naturalEnd === undefined ? wasPlaying && !snapshot.playing && (snapshot.songFlags & SONG_FLAG_END_REACHED) !== 0 : snapshot.naturalEnd && !snapshot.playing && snapshot.transport === 'stopped';
            wasPlaying = snapshot.playing;
            if (ended && !blocked && !emitted) {
                emitted = true;
                const entryId = current.entryId;
                for (const listener of endedListeners) listener(entryId);
            }
        },
        dispose() { controller.cancel(); clock.clearInterval(timer); },
    };
    const timer = clock.setInterval(() => controller.tick(), 100);
    return controller;
}
