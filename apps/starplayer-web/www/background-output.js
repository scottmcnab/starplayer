// One media element survives graph rebuilds; each context owns its stream destination.
export function createBackgroundOutput({ createAudio, enabled = false, report = () => {} }) {
    const audio = createAudio();
    audio.hidden = true;
    audio.autoplay = false;
    audio.setAttribute('playsinline', '');
    const destinations = new WeakMap();
    let node = null;
    let context = null;
    let playing = false;
    let failed = false;
    let revision = 0;
    let chosenSinkId = '';
    const output = {
        audio,
        get active() { return enabled && !failed; },
        get sinkId() { return output.target?.sinkId ?? ''; },
        get selectedSinkId() { return chosenSinkId; },
        get target() { return output.active ? audio : context; },
        get sinkSupported() { return typeof output.target?.setSinkId === 'function'; },
        async setSinkId(sinkId) {
            chosenSinkId = sinkId;
            if (output.sinkSupported) await output.target.setSinkId(sinkId);
        },
        // Called beside context.resume(), before any await in the originating gesture.
        start(nextContext) {
            playing = true;
            if (nextContext) context = nextContext;
            if (!output.active || !context) return;
            let destination = destinations.get(context);
            if (!destination) { destination = context.createMediaStreamDestination(); destinations.set(context, destination); }
            if (audio.srcObject !== destination.stream) { revision += 1; audio.srcObject = destination.stream; }
            const token = revision;
            const rejected = error => {
                if (token !== revision || !playing || !output.active) return;
                failed = true;
                output.connectOutput(node, context);
                output.setSinkId(chosenSinkId).catch(() => {});
                report(`Background playback unavailable (${error.name || error.message}); using direct output.`);
            };
            try { Promise.resolve(audio.play()).catch(rejected); } catch (error) { rejected(error); }
        },
        pause() { playing = false; revision += 1; audio.pause(); },
        connectOutput(nextNode, nextContext) {
            if (node === nextNode) node?.disconnect();
            node = nextNode;
            context = nextContext;
            if (output.active) {
                let destination = destinations.get(context);
                if (!destination) { destination = context.createMediaStreamDestination(); destinations.set(context, destination); }
                if (audio.srcObject !== destination.stream) { revision += 1; audio.srcObject = destination.stream; }
                node?.connect(destination);
                if (playing) output.start(context);
            } else {
                revision += 1;
                audio.pause();
                audio.srcObject = null;
                node?.connect(context.destination);
            }
        },
        setEnabled(value, nextNode, nextContext, shouldPlay) {
            enabled = value;
            failed = false;
            playing = shouldPlay;
            if (nextContext) output.connectOutput(nextNode, nextContext);
            output.setSinkId(chosenSinkId).catch(error => report(`Could not restore output device (${error.name || error.message}).`));
        },
    };
    return output;
}

export function backgroundPlaybackDefault(navigator, matchMedia) {
    return navigator.userAgentData?.mobile ?? matchMedia('(pointer: coarse)').matches;
}
