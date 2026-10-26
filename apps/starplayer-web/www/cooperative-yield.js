function checkAbort(signal) {
    if (signal?.aborted) throw new DOMException('Library request cancelled', 'AbortError');
}

// Also usable by worker consumers and browser evidence without touching page visibility.
export function cooperativeYieldMethod({ scheduler = globalThis.scheduler, MessageChannel = globalThis.MessageChannel } = {}) {
    if (typeof scheduler?.yield === 'function') return 'scheduler.yield';
    return typeof MessageChannel === 'function' ? 'MessageChannel' : 'setTimeout';
}

// Dependencies are injectable so every branch and cancellation race can be exercised.
export function createCooperativeYield({ scheduler = globalThis.scheduler, MessageChannel = globalThis.MessageChannel,
    setTimeout: scheduleTimer = globalThis.setTimeout, clearTimeout: cancelTimer = globalThis.clearTimeout } = {}) {
    return async function yieldCooperatively({ signal } = {}) {
        checkAbort(signal);
        await new Promise((resolve, reject) => {
            let channel, timer, settled = false;
            const finish = (error, failed = false) => {
                if (settled) return;
                settled = true;
                signal?.removeEventListener('abort', abort);
                if (channel) {
                    channel.port1.onmessage = null;
                    channel.port1.close();
                    channel.port2.close();
                }
                if (timer !== undefined) cancelTimer(timer);
                if (failed) reject(error);
                else resolve();
            };
            const abort = () => finish(new DOMException('Library request cancelled', 'AbortError'), true);
            signal?.addEventListener('abort', abort, { once: true });
            try {
                // Recheck after subscribing, before opening any task resources.
                checkAbort(signal);
                if (typeof scheduler?.yield === 'function') {
                    Promise.resolve(scheduler.yield()).then(() => finish(), error => finish(error, true));
                } else if (typeof MessageChannel === 'function') {
                    channel = new MessageChannel();
                    channel.port1.onmessage = () => finish();
                    channel.port2.postMessage(null);
                } else timer = scheduleTimer(() => finish(), 0);
            } catch (error) { finish(error, true); }
        });
        checkAbort(signal);
    };
}

export const yieldCooperatively = options => createCooperativeYield()(options);
