import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createMediaSession } from '../www/media-session.js';
function setup() {
    const handlers = new Map();
    const positions = [];
    const states = [];
    const calls = [];
    const session = { setActionHandler(action, callback) { handlers.set(action, callback); }, setPositionState(value) { positions.push(value); }, set playbackState(value) { states.push(value); } };
    const media = createMediaSession({ mediaSession: session, createMetadata: value => value, ...Object.fromEntries(['play', 'stop', 'next', 'previous', 'seek'].map(action => [action, value => calls.push([action, value])])) });
    return { media, session, handlers, positions, states, calls };
}
test('metadata uses module title, file label, library folder and archive label', () => {
    const { media, session } = setup();
    for (const [input, title, artist] of [[{ title: 'Song', label: 'song.mod' }, 'Song', 'StarPlayer'], [{ title: '', label: 'file.mod', folder: 'Composer' }, 'file.mod', 'Composer'], [{ title: 'Archive song', archiveLabel: 'songs.zip' }, 'Archive song', 'songs.zip']]) {
        media.metadata(input);
        assert.deepEqual(session.metadata, { title, artist, album: 'StarPlayer' });
    }
});
test('playback state changes only at transitions', () => {
    const { media, states } = setup();
    for (const value of [true, true, false, false, true]) media.playing(value);
    assert.deepEqual(states, ['playing', 'paused', 'playing']);
});
test('position converts frames, clamps seeks, clears unknown duration, tolerates TypeError', () => {
    const { media, session, positions } = setup();
    media.position({ lengthFrames: 480000, frame: 96000, sampleRate: 48000 });
    media.position({ lengthFrames: 480000, frame: 999999, sampleRate: 48000 });
    media.position({ lengthFrames: null, sampleRate: 48000 });
    assert.deepEqual(positions, [{ duration: 10, position: 2, playbackRate: 1 }, { duration: 10, position: 10, playbackRate: 1 }, undefined]);
    session.setPositionState = () => { throw new TypeError('out of range'); };
    assert.doesNotThrow(() => media.position({ lengthFrames: 1, sampleRate: 1 }));
});
test('handlers share transport callbacks and follow playlist and length availability', () => {
    const { media, handlers, calls } = setup();
    media.refresh({ hasEntries: true, nextEnabled: true, previousEnabled: true, lengthKnown: true });
    for (const action of ['play', 'pause', 'stop', 'nexttrack', 'previoustrack']) handlers.get(action)();
    handlers.get('seekto')({ seekTime: 7 });
    assert.deepEqual(calls, [['play', undefined], ['stop', undefined], ['stop', undefined], ['next', undefined], ['previous', undefined], ['seek', 7]]);
    media.refresh({ hasEntries: true, nextEnabled: false, previousEnabled: false, lengthKnown: false });
    assert.equal(handlers.get('nexttrack'), null);
    assert.equal(handlers.get('previoustrack'), null);
    assert.equal(handlers.get('seekto'), null);
    media.refresh({ hasEntries: false, nextEnabled: true, previousEnabled: true });
    assert.equal(handlers.get('nexttrack'), null);
});
test('absent media session is a no-op', () => {
    const media = createMediaSession({});
    media.metadata({}); media.playing(true); media.position({}); media.refresh({});
});

test('interactive Media Pause has a distinct sequencer-pause action from clear-and-hold Stop', () => {
    const handlers = new Map(), calls = [];
    const media = createMediaSession({ mediaSession: { setActionHandler(action, callback) { handlers.set(action, callback); } }, play: () => calls.push('resume'), pause: () => calls.push('pause'), stop: () => calls.push('clear-and-hold') });
    handlers.get('pause')(); handlers.get('play')(); handlers.get('stop')();
    assert.deepEqual(calls, ['pause', 'resume', 'clear-and-hold']);
    media.playing(false);
});
