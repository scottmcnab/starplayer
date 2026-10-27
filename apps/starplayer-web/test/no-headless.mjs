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
        const original = syntheticMod();
        const bytes = Buffer.alloc(original.length - 256 + 32768);
        original.copy(bytes, 0, 0, original.length - 256);
        bytes.writeUInt16BE(16384, 42);
        for (let index = original.length - 256; index < bytes.length; index++) bytes[index] = index % 37 < 19 ? 70 : 186;
        await loadFile(page, bytes, 'audition.mod');
        await page.waitFor('the applied interactive load', "!document.getElementById('pause').disabled && document.getElementById('transport-chip').textContent === 'playing'");
        assert.equal(await value("document.getElementById('note-overlap-enable').checked"), false);
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
        await page.waitFor('the song one-shot to finish while paused', "parseFloat(document.getElementById('master-peak').style.width) < 0.1");
        await page.evaluate("document.querySelector('#instrument-list li button:nth-of-type(2)').click();");
        await page.waitFor('audition while paused', "parseFloat(document.getElementById('master-peak').style.width) > 1");
        assert.equal(await value("document.getElementById('row').textContent"), held);
        await page.evaluate("document.getElementById('stop-preview').click();");
        await page.evaluate("document.querySelector('#instrument-list .instrument-select').click();");
        await page.waitFor('selected live destination', "document.getElementById('live-input-status').textContent.includes('selected instrument')");
        await page.evaluate("window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ', bubbles: true }));");
        await page.waitFor('selected keyboard note', "parseFloat(document.getElementById('master-peak').style.width) > 1");
        await page.evaluate("window.dispatchEvent(new KeyboardEvent('keyup', { code: 'KeyZ', bubbles: true })); window.dispatchEvent(new Event('blur'));");
        // A real enhancement rebuild must retain paused mode, full cursor and settings.
        await page.evaluate("document.getElementById('note-overlap-enable').click(); document.querySelector('#enhancement-options input').click();");
        await page.waitFor('same-session enhancement applied', "!document.querySelector('#enhancement-options input').disabled && document.getElementById('message').textContent.includes('Sample enhancement choice applied')");
        assert.equal(await value("document.getElementById('transport-chip').textContent"), 'paused');
        assert.equal(await value("document.querySelector('#instrument-list input').checked"), true);
        assert.equal(await value("document.getElementById('note-overlap-enable').checked"), true);
        assert.equal(await value("document.getElementById('row').textContent"), held);
        await page.evaluate("document.getElementById('sample-rate').value = '22050'; document.getElementById('apply-sample-rate').click();");
        await page.waitFor('replacement WASM Player at22050Hz', "document.getElementById('worklet-rate').textContent.replace(/[^0-9]/g, '') === '22050'");
        assert.equal(await value("document.getElementById('transport-chip').textContent"), 'paused');
        assert.equal(await value("document.querySelector('#instrument-list input').checked"), true);
        assert.equal(await value("document.getElementById('row').textContent"), held);
        await page.evaluate("document.getElementById('stop').click();");
        await page.waitFor('clear-and-hold Stop', "document.getElementById('transport-chip').textContent === 'stopped'");
        assert.equal(await value("document.getElementById('row').textContent"), held);
        await page.waitFor('Stop to settle to silence', "parseFloat(document.getElementById('master-peak').style.width) < 0.1");
        await page.evaluate("document.querySelector('#instrument-list li button:nth-of-type(2)').click();");
        await page.waitFor('audition while stopped', "parseFloat(document.getElementById('master-peak').style.width) > 1");
        await loadFile(page, bytes, 'audition-reload.mod');
        await page.waitFor('explicit same-byte reload', "document.getElementById('module-detail').textContent.includes('audition-reload.mod')");
        assert.equal(await value("document.querySelector('#instrument-list input').checked"), false);
        assert.equal(await value("document.getElementById('note-overlap-enable').checked"), false);
        assert.equal(await value("document.getElementById('error').hidden"), true, await value("document.getElementById('error').textContent"));
        report.pausedRow = held;
        report.acknowledgedRebuild = true;
        report.keyboardCleanup = true;
        report.explicitReloadResets = true;
        if (mode === 'plain') {
            await page.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1500, deviceScaleFactor: 1, mobile: false });
            await page.evaluate("document.getElementById('pause').click(); document.querySelector('#instrument-list input').click(); document.querySelector('.controls').scrollIntoView({ block: 'start' });");
            await page.waitFor('the screenshot paused state', "document.getElementById('transport-chip').textContent === 'paused'");
            const image = await page.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true });
            const { writeFile } = await import('node:fs/promises');
            await writeFile('/tmp/no-w1-controls.png', Buffer.from(image.data, 'base64'));
        }
        return report;
    } finally { await page.close(); server.stop(); }
}
