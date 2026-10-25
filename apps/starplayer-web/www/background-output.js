// The silent clip keeps a persistent media player alongside the direct Web Audio output.
export function createBackgroundOutput({ createAudio, createObjectURL = blob => URL.createObjectURL(blob), enabled = false, report = () => {} }) {
    const sampleRate = 8000;
    const sampleCount = sampleRate * 10;
    const bytes = new Uint8Array(44 + sampleCount);
    const header = new DataView(bytes.buffer);
    const text = (offset, value) => { for (let i = 0; i < value.length; i += 1) bytes[offset + i] = value.charCodeAt(i); };
    text(0, 'RIFF'); header.setUint32(4, bytes.length - 8, true);
    text(8, 'WAVE'); text(12, 'fmt '); header.setUint32(16, 16, true);
    header.setUint16(20, 1, true); header.setUint16(22, 1, true);
    header.setUint32(24, sampleRate, true); header.setUint32(28, sampleRate, true);
    header.setUint16(32, 1, true); header.setUint16(34, 8, true);
    text(36, 'data'); header.setUint32(40, sampleCount, true);
    bytes.fill(0x80, 44);
    const audio = createAudio();
    audio.hidden = true;
    audio.autoplay = false;
    audio.loop = true;
    audio.setAttribute('playsinline', '');
    audio.src = createObjectURL(new Blob([bytes], { type: 'audio/wav' }));
    let revision = 0;
    const output = {
        audio,
        // Called beside context.resume(), before any await in the originating gesture.
        start() {
            if (!enabled) return;
            const token = ++revision;
            const rejected = error => {
                if (token !== revision || !enabled) return;
                report(`Background playback unavailable (${error.name || error.message}).`);
            };
            try { Promise.resolve(audio.play()).catch(rejected); } catch (error) { rejected(error); }
        },
        pause() { revision += 1; audio.pause(); },
        setEnabled(value, shouldPlay) {
            enabled = value;
            if (enabled && shouldPlay) output.start();
            else output.pause();
        },
    };
    return output;
}

export function backgroundPlaybackDefault(navigator, matchMedia) {
    return navigator.userAgentData?.mobile ?? matchMedia('(pointer: coarse)').matches;
}
