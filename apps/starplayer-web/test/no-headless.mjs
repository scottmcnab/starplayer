import { runCompactInstruments } from './w17-headless.mjs';
import assert from 'node:assert/strict';

// Uses the repository's existing Chromium/CDP runner and packaged app.
export async function runInteractive({ Page, executable, mode, startDevServer, loadFile, syntheticMod }) {
    const server = await startDevServer(mode !== 'plain');
    const page = await Page.open(executable, `http://127.0.0.1:${server.port}/`);
    const report = { mode };
    const value = expression => page.evaluate(`return ${expression};`);
    try {
        await page.waitFor('the app listeners', "document.documentElement.dataset.playerReady === 'true'");
        if (mode === 'fallback') await page.evaluate("document.getElementById('force-fallback').checked = true;");
        await page.evaluate(`
            window.__failedLoadAcks = [];
            window.__interactiveLoadsApplied = 0;
            window.__appliedPolicies = [];
            const Original = window.AudioWorkletNode;
            window.AudioWorkletNode = class extends Original {
                constructor(...arguments_) {
                    super(...arguments_);
                    const post = this.port.postMessage.bind(this.port);
                    this.port.postMessage = (message, ...transfer) => {
                        if (window.__failNextAudioLoad && message.type === 'loadModule') {
                            window.__failNextAudioLoad = false;
                            message = { ...message, bytes: new Uint8Array(64).fill(65).buffer };
                        }
                        post(message, ...transfer);
                    };
                    this.port.addEventListener('message', event => {
                        if (event.data.type === 'moduleLoaded' && event.data.accepted) { window.__interactiveLoadsApplied++; window.__appliedPolicies.push([event.data.overlapEnabled, event.data.uniqueNotesAcrossChannels]); }
                        if (event.data.type === 'interactiveApplied') window.__failedLoadAcks.push({ accepted: event.data.accepted, identityWords: Array.from(event.data.words).slice(2, 6), commands: Array.from(event.data.commands).slice(0, event.data.count * 3) });
                    });
                }
            };
        `);

        const original = syntheticMod();
        const bytes = Buffer.alloc(original.length - 256 + 32768);
        original.copy(bytes, 0, 0, original.length - 256);
        bytes.writeUInt16BE(16384, 42);
        for (let index = original.length - 256; index < bytes.length; index++) bytes[index] = index % 37 < 19 ? 70 : 186;
        await loadFile(page, bytes, 'audition.mod');
        await page.waitFor('the applied interactive load', "!document.getElementById('pause').disabled && document.getElementById('transport-chip').textContent === 'playing'");
        assert.equal(await value("document.getElementById('note-overlap-enable').checked"), false);
        assert.equal(await value("document.getElementById('unique-notes-across-channels').checked"), false);
        assert.equal(await value("document.querySelectorAll('#channel-body tr').length"), 4);
        await page.evaluate("document.getElementById('pause').click();");
        await page.waitFor('Pause', "document.getElementById('transport-chip').textContent === 'paused'");
        const held = await value("document.getElementById('row').textContent");
        await new Promise(resolve => setTimeout(resolve, 200));
        assert.equal(await value("document.getElementById('row').textContent"), held);
        assert.equal(await value("document.getElementById('pause').getAttribute('aria-pressed')"), 'true');
        assert.match(await value("document.getElementById('play').textContent"), /Resume/);
        await page.evaluate("const input = document.querySelector('#instrument-list input'); input.checked = true; input.dispatchEvent(new Event('change'));");
        await page.waitFor('checkbox acknowledgement while bypassed', "document.querySelector('#instrument-list input').checked && !document.querySelector('#instrument-list input').disabled");
        await page.evaluate("document.getElementById('note-overlap-enable').click();");
        await new Promise(resolve => setTimeout(resolve, 100));
        await page.evaluate("document.getElementById('note-overlap-enable').click();");
        await new Promise(resolve => setTimeout(resolve, 100));
        assert.equal(await value("document.querySelector('#instrument-list input').checked"), true);
        const uniqueBefore = await value('window.__failedLoadAcks.length');
        await page.evaluate("document.getElementById('unique-notes-across-channels').click();");
        await page.waitFor('unique policy audio acknowledgement', `window.__failedLoadAcks.slice(${uniqueBefore}).some(message => message.accepted && message.commands.some((opcode, index) => index % 3 === 0 && opcode === StarPlayerRing.OPCODE_UNIQUE_NOTES && message.commands[index + 1] === 1))`);
        // Inspection succeeds; only the posted audio preparation sees malformed bytes.
        // The retained identity must restore usable controls without a graph rebuild.
        await page.evaluate('window.__failNextAudioLoad = true;');
        await loadFile(page, bytes, 'audio-rejected.mod');
        await page.waitFor('posted audio load failure', "!document.getElementById('error').hidden && document.getElementById('error').textContent.includes('audio-rejected.mod')");
        assert.match(await value("document.getElementById('module-detail').textContent"), /audition\.mod/, 'failed audio preparation retains committed metadata');
        assert.equal(await value("document.getElementById('transport-chip').textContent"), 'paused');
        assert.equal(await value("document.querySelector('#instrument-list input').disabled"), false);
        assert.equal(await value("document.querySelector('#instrument-list input').checked"), true);
        const beforeFailureRetry = await value('window.__failedLoadAcks.length');
        await page.evaluate("document.querySelector('#instrument-list input').click();");
        await page.waitFor('retained checkbox applies after posted failure', `window.__failedLoadAcks.slice(${beforeFailureRetry}).some(message => message.accepted && message.commands.some((opcode, index) => index % 3 === 0 && opcode === StarPlayerRing.OPCODE_LET_NOTES_RING && message.commands[index + 2] === 0))`);
        await page.evaluate("document.querySelector('#instrument-list input').click();");
        await page.waitFor('retained checkbox retry reenabled', `window.__failedLoadAcks.slice(${beforeFailureRetry}).some(message => message.accepted && message.commands.some((opcode, index) => index % 3 === 0 && opcode === StarPlayerRing.OPCODE_LET_NOTES_RING && message.commands[index + 2] === 1))`);
        // The following real Preview audio, native keyboard and rebuild assertions all
        // use this retained installation; the expected diagnostic stays until a new load.
        assert.equal(await value("document.getElementById('unique-notes-across-channels').checked"), true);
        assert.deepEqual(await value('window.__appliedPolicies.at(-1)'), [false, true], 'failed load reconciles both policies with retained audio');
        report.postedFailureRetainedControls = true;
        await page.waitFor('the song one-shot to finish while paused', "parseFloat(document.getElementById('master-peak').style.width) < 0.1");
        await page.evaluate("document.querySelector('#instrument-list .instrument-play').click();");
        await page.waitFor('audition while paused', "parseFloat(document.getElementById('master-peak').style.width) > 1");
        assert.equal(await value("document.getElementById('row').textContent"), held);
        await page.evaluate("document.getElementById('stop-preview').click();");
        await page.evaluate("document.querySelector('#instrument-list .instrument-select').click();");
        await page.waitFor('selected live destination', "document.getElementById('live-input-status').textContent.includes('selected instrument')");
        await page.evaluate("window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ', bubbles: true }));");
        await page.waitFor('selected keyboard note', "parseFloat(document.getElementById('master-peak').style.width) > 1");
        await page.evaluate("window.dispatchEvent(new KeyboardEvent('keyup', { code: 'KeyZ', bubbles: true })); window.dispatchEvent(new Event('blur'));");
        // A real enhancement rebuild must retain paused mode, full cursor and settings.
        const beforeEnhancementLoad = await value('window.__interactiveLoadsApplied');
        await page.evaluate("document.getElementById('note-overlap-enable').click(); document.querySelector('#enhancement-options input').click();");
        await page.waitFor('same-session enhancement applied', `!document.querySelector('#enhancement-options input').disabled && window.__interactiveLoadsApplied > ${beforeEnhancementLoad}`);
        assert.equal(await value("document.getElementById('transport-chip').textContent"), 'paused');
        assert.equal(await value("document.querySelector('#instrument-list input').checked"), true);
        assert.equal(await value("document.getElementById('note-overlap-enable').checked"), true);
        assert.equal(await value("document.getElementById('note-overlap-enable').disabled"), false, 'committed same-session settings remain editable');
        assert.equal(await value("document.getElementById('unique-notes-across-channels').checked"), true);
        assert.deepEqual(await value('window.__appliedPolicies.at(-1)'), [true, true]);
        assert.equal(await value("document.getElementById('row').textContent"), held);
        await page.evaluate("document.getElementById('sample-rate').value = '22050'; document.getElementById('apply-sample-rate').click();");
        await page.waitFor('replacement WASM Player at22050Hz', "document.getElementById('worklet-rate').textContent.replace(/[^0-9]/g, '') === '22050'");
        assert.equal(await value("document.getElementById('unique-notes-across-channels').checked"), true);
        assert.deepEqual(await value('window.__appliedPolicies.at(-1)'), [true, true]);
        assert.equal(await value("document.getElementById('transport-chip').textContent"), 'paused');
        assert.equal(await value("document.querySelector('#instrument-list input').checked"), true);
        assert.equal(await value("document.getElementById('row').textContent"), held);
        await page.evaluate("document.getElementById('stop').click();");
        await page.waitFor('clear-and-hold Stop', "document.getElementById('transport-chip').textContent === 'stopped'");
        assert.equal(await value("document.getElementById('row').textContent"), held);
        await page.waitFor('Stop to settle to silence', "parseFloat(document.getElementById('master-peak').style.width) < 0.1");
        await page.evaluate("document.querySelector('#instrument-list .instrument-play').click();");
        await page.waitFor('audition while stopped', "parseFloat(document.getElementById('master-peak').style.width) > 1");
        await loadFile(page, bytes, 'audition-reload.mod');
        await page.waitFor('explicit same-byte reload', "document.getElementById('module-detail').textContent.includes('audition-reload.mod')");
        assert.equal(await value("document.querySelector('#instrument-list input').checked"), false);
        assert.equal(await value("document.getElementById('note-overlap-enable').checked"), true, 'global overlap retains page intent on an explicit reload');
        assert.equal(await value("document.getElementById('unique-notes-across-channels').checked"), true, 'unique policy retains page intent on an explicit reload');
        assert.deepEqual(await value('window.__appliedPolicies.at(-1)'), [true, true]);
        assert.equal(await value("document.getElementById('error').hidden"), true, await value("document.getElementById('error').textContent"));
        report.pausedRow = held;
        report.acknowledgedRebuild = true;
        report.keyboardCleanup = true;
        report.explicitReloadResetsRing = true;
        // The compact-table regression explicitly exercises Ring editing while bypassed.
        const bypassBefore = await value('window.__failedLoadAcks.length');
        await page.evaluate("document.getElementById('note-overlap-enable').click();");
        await page.waitFor('explicit bypass before compact table', `window.__failedLoadAcks.slice(${bypassBefore}).some(message => message.accepted && message.commands.some((opcode, index) => index % 3 === 0 && opcode === StarPlayerRing.OPCODE_NOTE_OVERLAP && message.commands[index + 1] === 0))`);
        report.compactInstruments = await runCompactInstruments({ page, loadFile, mode });
        if (process.env.STARPLAYER_NO07_ARTIFACTS) {
            const { readFile } = await import('node:fs/promises');
            report.ownerFixtures = [];
            for (const name of ['percussion.mod', 'piano.mod', 'mixed-map.xm', 'dense-native.it']) {
                const fixture = await readFile(`${process.env.STARPLAYER_NO07_ARTIFACTS}/owner/${name}`);
                await loadFile(page, fixture, name);
                await page.waitFor(`applied ${name}`, "!document.getElementById('pause').disabled && document.getElementById('transport-chip').textContent === 'playing'");
                if (name === 'mixed-map.xm') assert.ok(await value("!!document.querySelector('#instrument-list [aria-label*=\"Mixed map\"]')"));
                if (name === 'dense-native.it') assert.equal(await value("document.querySelectorAll('#channel-body tr').length"), 64);
                await page.evaluate("document.getElementById('pause').click();");
                await page.waitFor(`paused ${name}`, "document.getElementById('transport-chip').textContent === 'paused'");
                await page.evaluate("document.querySelector('#instrument-list .instrument-play:not(:disabled)').click();");
                await page.waitFor(`audible ${name}`, "parseFloat(document.getElementById('master-peak').style.width) > 1");
                await page.evaluate("document.getElementById('stop-preview').click();");
                assert.equal(await value("document.getElementById('error').hidden"), true, await value("document.getElementById('error').textContent"));
                report.ownerFixtures.push(name);
            }
        }
        if (mode === 'plain') {
            await page.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1500, deviceScaleFactor: 1, mobile: false });
            await page.evaluate("if (document.getElementById('transport-chip').textContent === 'playing') document.getElementById('pause').click(); document.querySelector('#instrument-list input:not(:disabled)')?.click(); document.querySelector('.controls').scrollIntoView({ block: 'start' });");
            await page.waitFor('the screenshot paused state', "document.getElementById('transport-chip').textContent === 'paused'");
            const image = await page.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
            const { writeFile } = await import('node:fs/promises');
            await writeFile(process.env.STARPLAYER_NO07_ARTIFACTS ? `${process.env.STARPLAYER_NO07_ARTIFACTS}/controls.png` : '/tmp/no-w1-controls.png', Buffer.from(image.data, 'base64'));
        }
        await page.evaluate("document.getElementById('reference-note').value = '61'; document.getElementById('reference-note').dispatchEvent(new Event('change'));");
        await page.send('Page.reload');
        await page.waitFor('page-session preference reset', "document.documentElement.dataset.playerReady === 'true'");
        assert.equal(await value("document.getElementById('note-overlap-enable').checked"), false);
        assert.equal(await value("document.getElementById('unique-notes-across-channels').checked"), false);
        assert.equal(await value("document.getElementById('reference-note').value"), '60');
        report.pagePoliciesAndReload = true;
        return report;
    } finally { await page.close(); server.stop(); }
}
