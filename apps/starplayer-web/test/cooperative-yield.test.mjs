import test from 'node:test';
import assert from 'node:assert/strict';
import { cooperativeYieldMethod, createCooperativeYield, yieldCooperatively } from '../www/cooperative-yield.js';

function controlledChannel() {
    const channels = [];
    class MessageChannel {
        constructor() {
            this.closed = [];
            this.port1 = { onmessage: null, close: () => this.closed.push(1) };
            this.port2 = { postMessage: () => { this.callback = this.port1.onmessage; }, close: () => this.closed.push(2) };
            channels.push(this);
        }
    }
    return { channels, MessageChannel };
}

function trackedController() {
    const controller = new AbortController();
    const listeners = new Set();
    const signal = {
        get aborted() { return controller.signal.aborted; },
        addEventListener(type, listener, options) { listeners.add(listener); controller.signal.addEventListener(type, listener, options); },
        removeEventListener(type, listener) { listeners.delete(listener); controller.signal.removeEventListener(type, listener); },
    };
    return { signal, listeners, abort: () => controller.abort() };
}

test('capability diagnostics name the selected native, channel or timer branch', () => {
    assert.equal(cooperativeYieldMethod({ scheduler: { postTask() {}, yield() {} }, MessageChannel: null }), 'scheduler.postTask(background) + scheduler.yield');
    assert.equal(cooperativeYieldMethod({ scheduler: {}, MessageChannel: class {} }), 'MessageChannel');
    assert.equal(cooperativeYieldMethod({ scheduler: null, MessageChannel: null }), 'setTimeout');
    assert.ok(['scheduler.postTask(background) + scheduler.yield', 'MessageChannel', 'setTimeout'].includes(cooperativeYieldMethod()));
});

test('native yield runs only inside a background postTask and preserves both receivers', async () => {
    const controller = trackedController();
    let runTask, complete, postCalls = 0, yieldCalls = 0;
    const scheduler = {
        postTask(callback, options) {
            assert.equal(this, scheduler);
            assert.deepEqual(options, { priority: 'background' });
            postCalls += 1;
            return new Promise((resolve, reject) => { runTask = () => Promise.resolve().then(callback).then(resolve, reject); });
        },
        yield() { assert.equal(this, scheduler); yieldCalls += 1; return new Promise(resolve => { complete = resolve; }); },
    };
    const yielding = createCooperativeYield({ scheduler, MessageChannel: class { constructor() { throw new Error('unexpected channel'); } } });
    let settled = false;
    const result = yielding({ signal: controller.signal }).then(() => { settled = true; });
    await Promise.resolve();
    assert.equal(settled, false);
    assert.equal(postCalls, 1);
    assert.equal(yieldCalls, 0);
    const task = runTask();
    await Promise.resolve();
    assert.equal(yieldCalls, 1);
    complete();
    await task;
    await result;
    assert.equal(controller.listeners.size, 0);
});

test('an already aborted request opens no scheduling resources', async () => {
    const controller = trackedController();
    controller.abort();
    let calls = 0;
    await assert.rejects(createCooperativeYield({ scheduler: { postTask() { calls += 1; }, yield() { calls += 1; } } })({ signal: controller.signal }), { name: 'AbortError' });
    assert.equal(calls, 0);
    assert.equal(controller.listeners.size, 0);
});

test('native pending yield aborts promptly and tolerates a late native rejection', async () => {
    const controller = trackedController();
    let fail;
    const result = createCooperativeYield({ scheduler: { postTask: callback => callback(), yield: () => new Promise((resolve, reject) => { fail = reject; }) } })({ signal: controller.signal });
    controller.abort();
    await assert.rejects(result, { name: 'AbortError' });
    assert.equal(controller.listeners.size, 0);
    fail(new Error('late scheduler failure'));
    await Promise.resolve();
});

test('abort after task resolution but before the continuation still rejects', async () => {
    const { channels, MessageChannel } = controlledChannel();
    const controller = trackedController();
    const result = createCooperativeYield({ scheduler: null, MessageChannel })({ signal: controller.signal });
    channels[0].callback();
    controller.abort();
    await assert.rejects(result, { name: 'AbortError' });
    assert.deepEqual(channels[0].closed, [1, 2]);
    assert.equal(controller.listeners.size, 0);
});

test('channel task completes once and closes both ports and the abort listener', async () => {
    const { channels, MessageChannel } = controlledChannel();
    const controller = trackedController();
    const result = createCooperativeYield({ scheduler: null, MessageChannel, setTimeout() { throw new Error('unexpected timer'); } })({ signal: controller.signal });
    assert.equal(channels.length, 1);
    assert.equal(channels[0].closed.length, 0);
    channels[0].callback();
    await result;
    channels[0].callback();
    assert.deepEqual(channels[0].closed, [1, 2]);
    assert.equal(channels[0].port1.onmessage, null);
    assert.equal(controller.listeners.size, 0);
});

test('aborting channel task settles without a message and ignores late delivery', async () => {
    const { channels, MessageChannel } = controlledChannel();
    const controller = trackedController();
    const result = createCooperativeYield({ scheduler: null, MessageChannel })({ signal: controller.signal });
    controller.abort();
    await assert.rejects(result, { name: 'AbortError' });
    channels[0].callback();
    assert.deepEqual(channels[0].closed, [1, 2]);
    assert.equal(controller.listeners.size, 0);
});

test('timer is the final fallback and is cleared on completion and abort', async () => {
    for (const abort of [false, true]) {
        const controller = trackedController(), cleared = [];
        let callback;
        const yielding = createCooperativeYield({ scheduler: null, MessageChannel: null,
            setTimeout(handler, delay) { callback = handler; assert.equal(delay, 0); return 42; }, clearTimeout: timer => cleared.push(timer) });
        const result = yielding({ signal: controller.signal });
        if (abort) { controller.abort(); await assert.rejects(result, { name: 'AbortError' }); }
        else { callback(); await result; }
        callback();
        assert.deepEqual(cleared, [42]);
        assert.equal(controller.listeners.size, 0);
    }
});

test('scheduling errors reject and remove listeners and opened ports', async () => {
    for (const asynchronous of [false, true]) {
        const controller = trackedController();
        const scheduler = { postTask: callback => callback(), yield() { if (asynchronous) return Promise.reject(new Error('scheduler failed')); throw new Error('scheduler failed'); } };
        await assert.rejects(createCooperativeYield({ scheduler })({ signal: controller.signal }), /scheduler failed/);
        assert.equal(controller.listeners.size, 0);
    }
    const controller = trackedController();
    const closed = [];
    class MessageChannel {
        port1 = { onmessage: null, close: () => closed.push(1) };
        port2 = { close: () => closed.push(2), postMessage() { throw new Error('post failed'); } };
    }
    await assert.rejects(createCooperativeYield({ scheduler: null, MessageChannel })({ signal: controller.signal }), /post failed/);
    assert.deepEqual(closed, [1, 2]);
    assert.equal(controller.listeners.size, 0);
});

test('real channel and timer yields allow another task to interleave', async () => {
    for (const yielding of [yieldCooperatively, createCooperativeYield({ scheduler: null, MessageChannel: null })]) {
        let heartbeat = false;
        setImmediate(() => { heartbeat = true; });
        for (let index = 0; index < 8; index += 1) await yielding();
        assert.equal(heartbeat, true);
    }
});


test('native yield rejection preserves falsy rejection values', async () => {
    for (const reason of [undefined, null, false, 0, '']) {
        for (const asynchronous of [false, true]) {
            const controller = trackedController();
            const scheduler = { postTask: callback => callback(), yield() { if (asynchronous) return Promise.reject(reason); throw reason; } };
            await assert.rejects(createCooperativeYield({ scheduler })({ signal: controller.signal }), error => error === reason);
            assert.equal(controller.listeners.size, 0);
        }
    }
});


test('either missing native method selects MessageChannel or timer without calling the other method', async () => {
    for (const scheduler of [
        { yield() { throw new Error('unprioritized yield forbidden'); } },
        { postTask() { throw new Error('postTask without yield forbidden'); } },
        { postTask: null, yield() { throw new Error('unprioritized yield forbidden'); } },
        { postTask() { throw new Error('postTask without yield forbidden'); }, yield: 'not callable' },
    ]) {
        const { channels, MessageChannel } = controlledChannel();
        assert.equal(cooperativeYieldMethod({ scheduler, MessageChannel }), 'MessageChannel');
        const channelResult = createCooperativeYield({ scheduler, MessageChannel })();
        assert.equal(channels.length, 1);
        channels[0].callback();
        await channelResult;
        assert.deepEqual(channels[0].closed, [1, 2]);
        assert.equal(cooperativeYieldMethod({ scheduler, MessageChannel: null }), 'setTimeout');
        let fireTimer, cleared = false;
        const timerResult = createCooperativeYield({ scheduler, MessageChannel: null,
            setTimeout(callback) { fireTimer = callback; return 42; }, clearTimeout(timer) { assert.equal(timer, 42); cleared = true; } })();
        fireTimer();
        await timerResult;
        assert.equal(cleared, true);
    }
});

test('abort before a queued postTask callback prevents yield and consumes the late callback rejection', async () => {
    const controller = trackedController();
    let runTask, yieldCalls = 0;
    const scheduler = {
        postTask(callback) { return new Promise((resolve, reject) => { runTask = () => Promise.resolve().then(callback).then(resolve, reject); }); },
        yield() { yieldCalls += 1; },
    };
    const result = createCooperativeYield({ scheduler })({ signal: controller.signal });
    controller.abort();
    await assert.rejects(result, { name: 'AbortError' });
    assert.equal(controller.listeners.size, 0);
    await runTask();
    assert.equal(yieldCalls, 0);
    assert.equal(controller.listeners.size, 0);
});

test('abort during native yield consumes late success as well as late rejection', async () => {
    const controller = trackedController();
    let complete;
    const scheduler = { postTask: callback => callback(), yield: () => new Promise(resolve => { complete = resolve; }) };
    const result = createCooperativeYield({ scheduler })({ signal: controller.signal });
    controller.abort();
    await assert.rejects(result, { name: 'AbortError' });
    complete();
    await Promise.resolve();
    assert.equal(controller.listeners.size, 0);
});

test('native postTask failures preserve Error and falsy rejection reasons and clean listeners', async () => {
    for (const reason of [new Error('postTask failed'), undefined, null, false, 0, '']) {
        for (const asynchronous of [false, true]) {
            const controller = trackedController();
            let yieldCalls = 0;
            const scheduler = {
                postTask() { if (asynchronous) return Promise.reject(reason); throw reason; },
                yield() { yieldCalls += 1; },
            };
            await assert.rejects(createCooperativeYield({ scheduler })({ signal: controller.signal }), error => error === reason);
            assert.equal(yieldCalls, 0);
            assert.equal(controller.listeners.size, 0);
        }
    }
});
