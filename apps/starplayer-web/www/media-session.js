export function createMediaSession({ mediaSession, createMetadata = metadata => new MediaMetadata(metadata), play, stop, next, previous, seek }) {
    let playbackState = null;
    const handlers = new Map();
    function handler(action, callback) {
        if (!mediaSession || handlers.get(action) === callback) return;
        try { mediaSession.setActionHandler(action, callback); } catch (_) { /* Optional actions vary by browser. */ }
        handlers.set(action, callback);
    }
    handler('play', play);
    handler('pause', stop);
    handler('stop', stop);
    const seekTo = details => seek(details.seekTime);
    return {
        metadata({ title, label, folder, archiveLabel }) {
            if (mediaSession) mediaSession.metadata = createMetadata({ title: title?.trim() || label, artist: folder || archiveLabel || 'StarPlayer', album: 'StarPlayer' });
        },
        playing(value) {
            const nextState = value ? 'playing' : 'paused';
            if (mediaSession && playbackState !== nextState) { mediaSession.playbackState = nextState; playbackState = nextState; }
        },
        position({ lengthFrames, frame = 0, sampleRate }) {
            if (!mediaSession?.setPositionState) return;
            const duration = lengthFrames / sampleRate;
            try {
                if (lengthFrames === null || !Number.isFinite(duration) || duration <= 0) mediaSession.setPositionState();
                else mediaSession.setPositionState({ duration, position: Math.max(0, Math.min(duration, frame / sampleRate)), playbackRate: 1 });
            } catch (error) { if (!(error instanceof TypeError)) throw error; }
        },
        refresh({ hasEntries, nextEnabled, previousEnabled, lengthKnown }) {
            handler('nexttrack', hasEntries && nextEnabled ? next : null);
            handler('previoustrack', hasEntries && previousEnabled ? previous : null);
            handler('seekto', lengthKnown ? seekTo : null);
        },
    };
}
