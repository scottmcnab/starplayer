import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';

// Native XM with 96 eligible instruments: bulk controls must cross the bounded
// bridge. Instrument 1 maps a loop and a one-shot, and auditions the loop at note 60.
function compactTableXm(lowTuning = false) {
    const header = Buffer.alloc(336);
    header.write('Extended Module: '); header.write('compact table', 17); header[37] = 0x1a;
    header.write('FastTracker v2.00', 38); header.writeUInt16LE(0x104, 58);
    header.writeUInt32LE(276, 60); header.writeUInt16LE(1, 64); header.writeUInt16LE(1, 68);
    header.writeUInt16LE(1, 70); header.writeUInt16LE(96, 72); header.writeUInt16LE(1, 74);
    header.writeUInt16LE(6, 76); header.writeUInt16LE(125, 78);
    const pattern = Buffer.alloc(9); pattern.writeUInt32LE(9); pattern.writeUInt16LE(64, 5);
    const chunks = [header, pattern];
    for (let index = 0; index < 96; index++) {
        const instrument = Buffer.alloc(263); instrument.writeUInt32LE(263);
        instrument.write(index ? `instrument ${index + 1}` : 'mixed map loop and hit', 4);
        const count = index === 0 ? 2 : 1;
        instrument.writeUInt16LE(count, 27); instrument.writeUInt32LE(40, 29);
        if (index === 0) { instrument[33] = 1; instrument[33 + 61] = 1; } // Selected playable one-shot alongside the reference loop.
        chunks.push(instrument);
        const pcm = [];
        for (let sample = 0; sample < count; sample++) {
            const descriptor = Buffer.alloc(40); descriptor.writeUInt32LE(256);
            descriptor[12] = 64; descriptor[15] = 128;
            if (lowTuning && index === 0 && sample === 0) descriptor.writeInt8(-80, 16);
            if (index === 0 && sample === 0) { descriptor.writeUInt32LE(256, 8); descriptor[14] = 1; }
            chunks.push(descriptor);
            const deltas = Buffer.alloc(256); let previous = 0;
            for (let frame = 0; frame < 256; frame++) {
                const next = frame % 32 < 16 ? 60 : -60;
                deltas[frame] = (next - previous) & 255; previous = next;
            }
            pcm.push(deltas);
        }
        chunks.push(...pcm);
    }
    return Buffer.concat(chunks);
}

export async function runCompactInstruments({ page, loadFile, mode }) {
    const value = expression => page.evaluate(`return ${expression};`);
    const screenshotPanel = async () => {
        const clip = await page.evaluate("const panel = document.querySelector('.instrument-scroll').closest('article'); const bounds = panel.getBoundingClientRect(); return { x: bounds.x + scrollX, y: bounds.y + scrollY, width: bounds.width, height: bounds.height, scale: 1 };");
        return page.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true, clip });
    };
    const selected = "document.querySelectorAll('#instrument-list tr[aria-selected=true]').length";
    const selectedIndex = "document.querySelector('#instrument-list tr[aria-selected=true]')?.dataset.instrument";
    const checked = "document.querySelectorAll('#instrument-list input:checked').length";
    const header = "document.getElementById('instrument-select-all')";
    const countExpression = opcode => `window.__failedLoadAcks.filter(message => message.accepted).flatMap(message => message.commands.filter((value, index) => index % 3 === 0 && value === StarPlayerRing.${opcode})).length`;
    const ackCount = opcode => value(countExpression(opcode));
    const key = async (code, character, virtualKeyCode) => {
        for (const type of ['keyDown', 'keyUp']) await page.send('Input.dispatchKeyEvent', { type, code, key: character, windowsVirtualKeyCode: virtualKeyCode, nativeVirtualKeyCode: virtualKeyCode });
    };
    const point = async selector => page.evaluate(`const element = document.querySelector(${JSON.stringify(selector)}); element.scrollIntoView({ block: 'center' }); const box = element.getBoundingClientRect(); return { x: box.x + box.width / 2, y: box.y + box.height / 2 };`);
    const click = async selector => {
        const coordinates = await point(selector);
        for (const type of ['mousePressed', 'mouseReleased']) await page.send('Input.dispatchMouseEvent', { type, button: 'left', clickCount: 1, ...coordinates });
    };
    const touch = async selector => {
        const coordinates = await point(selector);
        await page.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ ...coordinates, id: 1 }] });
        await page.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    };
    await loadFile(page, compactTableXm(), 'compact-table.xm');
    await page.waitFor('96 instrument table', "document.querySelectorAll('#instrument-list tr[data-instrument]').length === 96 && !document.getElementById('pause').disabled");
    await page.evaluate("document.getElementById('pause').click();");
    await page.waitFor('compact table paused', "document.getElementById('transport-chip').textContent === 'paused'");
    assert.deepEqual(await value("[...document.querySelectorAll('.instrument-table th')].map(cell => cell.textContent.trim())"), ['#', 'Name', '', '', '', 'Frames']);
    assert.equal(await value("document.getElementById('reference-note').options.length"), 128);
    assert.equal(await value("document.getElementById('reference-note').value"), '60');
    assert.match(await value("document.getElementById('reference-note').selectedOptions[0].textContent"), /C-5 \(60\).*middle C/);
    assert.deepEqual(await value("[...document.querySelectorAll('.instrument-table th')].map(cell => cell.getAttribute('aria-label')).filter(Boolean)"), ['Loop', 'Play']);
    assert.equal(await value("getComputedStyle(document.querySelector('.instrument-play')).borderTopWidth"), '0px');
    assert.equal(await value("document.querySelector('#instrument-list tr td:last-child').textContent"), '512');
    assert.equal(await value("document.querySelector('#instrument-list tr td:nth-child(3)').textContent"), '∞');
    assert.equal(await value("document.querySelector('#instrument-list tr:nth-child(2) td:nth-child(3)').textContent"), '');
    assert.equal(await value(`${header}.checked || ${header}.indeterminate`), false);
    const countBefore = await ackCount('OPCODE_LET_NOTES_RING');
    await page.evaluate(`${header}.click();`);
    await page.waitFor('all 96 overlap acknowledgements', `${checked} === 96 && ${header}.checked && !${header}.indeterminate`);
    assert.equal(await ackCount('OPCODE_LET_NOTES_RING'), countBefore + 96);
    assert.equal(await value("document.getElementById('note-overlap-enable').checked"), false);
    assert.equal(await value(selected), 0);
    await page.evaluate(`${header}.click();`);
    await page.waitFor('clear all acknowledgements', `${checked} === 0 && !${header}.checked && !${header}.indeterminate`);
    // All requests occur before any acknowledgement; the later row override must win.
    await page.evaluate(`${header}.click(); ${header}.click(); document.querySelector('#instrument-list tr:last-child input').click();`);
    await page.waitFor('rapid bulk clear and row override', `${checked} === 1 && document.querySelector('#instrument-list tr:last-child input').checked && ${header}.indeterminate`);
    // A native Space on mixed selects all once, then a native pointer clear.
    await page.evaluate(`${header}.focus();`); await key('Space', ' ', 32);
    await page.waitFor('native mixed-to-all Space', `${checked} === 96 && ${header}.checked`);
    await click('#instrument-select-all');
    await page.waitFor('native header pointer clear', `${checked} === 0`);
    await page.evaluate("document.querySelector('#instrument-list tr').focus();"); await key('Enter', 'Enter', 13);
    await page.waitFor('native row Enter selection', "document.querySelector('#instrument-list tr').getAttribute('aria-selected') === 'true'");
    assert.equal(await value(selected), 1);
    assert.match(await value("document.getElementById('live-input-status').textContent"), /selected instrument/);
    await page.evaluate("document.querySelector('#instrument-list tr:nth-child(2)').focus();"); await key('Space', ' ', 32);
    await page.waitFor('native row Space selection', `${selectedIndex} === '1'`);
    const beforeSpace = await ackCount('OPCODE_LET_NOTES_RING');
    await page.evaluate("document.querySelector('#instrument-list tr:nth-child(2) input').focus();"); await key('Space', ' ', 32);
    await page.waitFor('native row checkbox Space acknowledgement', `${checked} === 1 && ${countExpression('OPCODE_LET_NOTES_RING')} === ${beforeSpace + 1}`);
    assert.equal(await ackCount('OPCODE_LET_NOTES_RING'), beforeSpace + 1);
    assert.equal(await value(selected), 1);
    assert.equal(await value(selectedIndex), '1');
    // Holding a real looping Play pointer must survive an unrelated settings ack.
    const coordinates = await point('#instrument-list .instrument-play');
    await page.evaluate("window.__heldPlay = document.querySelector('#instrument-list .instrument-play'); window.__heldBlurs = 0; window.__heldPlay.addEventListener('blur', () => window.__heldBlurs++); window.__heldPlay.focus();");
    await page.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...coordinates });
    await page.waitFor('held looping Play', "parseFloat(document.getElementById('master-peak').style.width) > 1");
    const stopsBefore = await ackCount('OPCODE_STOP_PREVIEW');
    const overlapBefore = await ackCount('OPCODE_LET_NOTES_RING');
    await page.evaluate("document.querySelector('#instrument-list tr:nth-child(3) input').click();");
    await page.waitFor('unrelated ring acknowledgement', `${countExpression('OPCODE_LET_NOTES_RING')} === ${overlapBefore + 1} && document.querySelector('#instrument-list tr:nth-child(3) input').checked`);
    assert.equal(await value("window.__heldPlay === document.querySelector('#instrument-list .instrument-play') && document.activeElement === window.__heldPlay && window.__heldBlurs === 0"), true);
    assert.equal(await ackCount('OPCODE_STOP_PREVIEW'), stopsBefore);
    assert.equal(await value(selectedIndex), '1', 'Play and native controls do not select their instrument');
    await page.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...coordinates });
    const previewsBefore = await ackCount('OPCODE_PREVIEW_NOTE');
    await page.evaluate('window.__heldPlay.focus();'); await key('Enter', 'Enter', 13);
    await page.waitFor('native Play Enter once', `window.__failedLoadAcks.filter(message => message.accepted).flatMap(message => message.commands.filter((value, index) => index % 3 === 0 && value === StarPlayerRing.OPCODE_PREVIEW_NOTE)).length === ${previewsBefore + 1}`);
    const offscreenBefore = await ackCount('OPCODE_LET_NOTES_RING');
    await page.evaluate("document.getElementById('stop-preview').click(); document.querySelector('.instrument-scroll').scrollTop = 1800; const input = document.querySelector('#instrument-list tr:nth-child(60) input'); input.focus(); window.__tableFocus = input; window.__tableScroll = input.closest('.instrument-scroll').scrollTop; input.click();");
    await page.waitFor('offscreen focus acknowledgement', `${countExpression('OPCODE_LET_NOTES_RING')} === ${offscreenBefore + 1} && document.querySelector('#instrument-list tr:nth-child(60) input').checked`);
    assert.equal(await value("document.activeElement === window.__tableFocus && document.querySelector('.instrument-scroll').scrollTop === window.__tableScroll"), true);
    // Capture must drain bulk settings before a same-session enhancement replacement.
    const loadsBefore = await value('window.__interactiveLoadsApplied');
    await page.evaluate(`${header}.click(); document.querySelector('#enhancement-options input').click();`);
    await page.waitFor('bulk captured across rebuild', `window.__interactiveLoadsApplied > ${loadsBefore} && !document.querySelector('#enhancement-options input').disabled && ${checked} === 96 && ${header}.checked`);
    await page.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
    await page.send('Emulation.setTouchEmulationEnabled', { enabled: true });
    await touch('#instrument-select-all'); await page.waitFor('touch clear all', `${checked} === 0`);
    await touch('#instrument-list tr:nth-child(2) .instrument-checkbox'); await page.waitFor('touch row checkbox', `${checked} === 1 && ${header}.indeterminate`);
    await touch('#instrument-select-all'); await page.waitFor('touch mixed selects all', `${checked} === 96 && ${header}.checked`);
    assert.equal(await value(selected), 1, 'checkbox taps never change selected row');
    assert.equal(await value(selectedIndex), '1');
    assert.ok(await value("document.documentElement.scrollWidth <= window.innerWidth"), 'phone page has no horizontal overflow');
    assert.equal(await value("document.querySelector('#instrument-list tr').getBoundingClientRect().height"), 32);
    assert.equal(await value("document.querySelector('#instrument-list .instrument-checkbox').getBoundingClientRect().width"), 40);
    await page.evaluate("document.querySelector('.instrument-scroll').scrollTop = 0; document.querySelector('.instrument-scroll').scrollIntoView({ block: 'center' });");
    let screenshot = await screenshotPanel();
    await writeFile(`/tmp/w17-instruments-${mode}-phone.png`, Buffer.from(screenshot.data, 'base64'));
    // Real touch cancellation and focus loss retain scoped preview cleanup.
    const cancelPoint = await point('#instrument-list .instrument-play');
    const beforeCancel = await ackCount('OPCODE_STOP_PREVIEW');
    await page.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ ...cancelPoint, id: 2 }] });
    await page.send('Input.dispatchTouchEvent', { type: 'touchCancel', touchPoints: [] });
    await page.waitFor('touch cancellation cleanup', `window.__failedLoadAcks.filter(message => message.accepted).flatMap(message => message.commands.filter((value, index) => index % 3 === 0 && value === StarPlayerRing.OPCODE_STOP_PREVIEW)).length > ${beforeCancel}`);
    const focusPoint = await point('#instrument-list .instrument-play');
    const beforeFocusLoss = await ackCount('OPCODE_STOP_PREVIEW');
    await page.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...focusPoint });
    await page.evaluate("document.querySelector('#instrument-list .instrument-play').focus(); document.getElementById('stop-preview').focus();");
    await page.waitFor('held Play focus-loss cleanup', `window.__failedLoadAcks.filter(message => message.accepted).flatMap(message => message.commands.filter((value, index) => index % 3 === 0 && value === StarPlayerRing.OPCODE_STOP_PREVIEW)).length > ${beforeFocusLoss}`);
    await page.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...focusPoint });
    await page.send('Emulation.setTouchEmulationEnabled', { enabled: false });
    await page.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false });
    await page.evaluate("document.querySelector('.instrument-scroll').scrollTop = 0; document.querySelector('.instrument-scroll').scrollIntoView({ block: 'center' });");
    screenshot = await screenshotPanel();
    await writeFile(`/tmp/w17-instruments-${mode}-desktop.png`, Buffer.from(screenshot.data, 'base64'));
    assert.equal(await value("document.documentElement.scrollWidth <= window.innerWidth"), true);
    // Dispatch the file change in the same page task as two full-module edits;
    // native bridge work and its queued remainder must never retarget the new session.
    assert.equal(await value("document.getElementById('error').hidden"), true, await value("JSON.stringify(window.__failedLoadAcks.filter(message => !message.accepted))"));
    const pendingLoadPage = { evaluate: script => page.evaluate(`${header}.click(); ${header}.click(); ${script}`) };
    await loadFile(pendingLoadPage, compactTableXm(), 'compact-table-fresh.xm');
    await page.waitFor('fresh session during queued bulk', `document.getElementById('module-detail').textContent.includes('compact-table-fresh.xm') && !document.getElementById('pause').disabled && ${checked} === 0 && !${header}.checked && !${header}.indeterminate`);
    await new Promise(resolve => setTimeout(resolve, 200));
    assert.equal(await value(checked), 0, 'late settings cannot change the fresh module');
    assert.equal(await value(selected), 0);
    assert.equal(await value("document.getElementById('error').hidden"), true, await value("JSON.stringify(window.__failedLoadAcks.filter(message => !message.accepted))"));
    // Native selected-key viability shares XM relative-note preflight with rack onset.
    await loadFile(page, compactTableXm(true), 'selected-key.xm');
    await page.waitFor('selected-key module ready', "document.getElementById('module-detail').textContent.includes('selected-key.xm') && !document.getElementById('pause').disabled");
    await page.evaluate("document.getElementById('pause').click();");
    await page.waitFor('selected-key pause', "document.getElementById('transport-chip').textContent === 'paused'");
    assert.equal(await value("document.querySelector('#instrument-list .instrument-play').disabled"), true, 'reference60 native XM tuning is out of range');
    const choose = async note => page.evaluate(`const select = document.getElementById('reference-note'); select.value = '${note}'; select.dispatchEvent(new Event('change'));`);
    await choose(61);
    assert.equal(await value("document.querySelector('#instrument-list .instrument-play').disabled"), false, '61 maps a playable one-shot');
    const beforeSelected = await ackCount('OPCODE_PREVIEW_NOTE');
    await click('#instrument-list .instrument-play');
    await page.waitFor('selected note61 applied', `${countExpression('OPCODE_PREVIEW_NOTE')} === ${beforeSelected + 1}`);
    assert.equal(await value("window.__failedLoadAcks.filter(message => message.accepted).flatMap(message => { const records = []; for (let index = 0; index < message.commands.length; index += 3) if (message.commands[index] === StarPlayerRing.OPCODE_PREVIEW_NOTE) records.push(message.commands[index + 2]); return records; }).at(-1)"), 61);
    await choose(127); assert.equal(await value("document.querySelector('#instrument-list .instrument-play').disabled"), true, 'unmapped native key');
    await choose(95);
    assert.equal(await value("document.querySelector('#instrument-list .instrument-play').disabled"), false, 'native tuned loop is playable at95');
    const loopPoint = await point('#instrument-list .instrument-play');
    const releaseBefore = await ackCount('OPCODE_RELEASE_PREVIEW');
    await page.send('Input.dispatchMouseEvent', { type: 'mousePressed', button: 'left', clickCount: 1, ...loopPoint });
    await page.waitFor('selected looping preview applied', `${countExpression('OPCODE_PREVIEW_NOTE')} === ${beforeSelected + 2}`);
    await choose(61);
    await page.send('Input.dispatchMouseEvent', { type: 'mouseReleased', button: 'left', clickCount: 1, ...loopPoint });
    await page.waitFor('captured native loop release after chooser change', `${countExpression('OPCODE_RELEASE_PREVIEW')} === ${releaseBefore + 1}`);
    const loadBeforeChoice = await value('window.__interactiveLoadsApplied');
    await page.evaluate("document.querySelector('#enhancement-options input').click();");
    await page.waitFor('reference choice across graph rebuild', `window.__interactiveLoadsApplied > ${loadBeforeChoice} && !document.querySelector('#enhancement-options input').disabled`);
    assert.equal(await value("document.getElementById('reference-note').value"), '61');
    await loadFile(page, compactTableXm(), 'reference-retained.xm');
    await page.waitFor('reference choice across fresh module', "document.getElementById('module-detail').textContent.includes('reference-retained.xm') && !document.getElementById('pause').disabled");
    assert.equal(await value("document.getElementById('reference-note').value"), '61');
    await choose(60);
    return { freshLoadDuringBulk: true, instruments: 96, bulkAcrossBatches: true, nativeKeysAndTouch: true, retainedHeldPlay: true, retainedFocusAndScroll: true, bulkRebuild: true };
}
