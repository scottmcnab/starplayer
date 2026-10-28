// Real native input, packaged HTML/CSS and browser persistence complement the Node DOM tests.
import assert from 'node:assert/strict';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';

export async function runPlaylistSelection({ Page, executable, mode, startDevServer }) {
    const server = await startDevServer(mode !== 'plain');
    let page;
    const report = {};
    try {
        page = await Page.open(executable, `http://127.0.0.1:${server.port}/`);
        await page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 960, deviceScaleFactor: 1, mobile: false });
        await page.waitFor('W18 Playlist startup', "document.documentElement.dataset.playerReady === 'true'");
        await page.evaluate(`
            const panel = document.getElementById('playlist-panel'); panel.replaceWith(panel.cloneNode(true));
            const { createPlaylist, wirePlaylistList } = await import('./playlist.js');
            const { createPlaylistControls } = await import('./playlist-controls.js');
            const { openPlaylistStore } = await import('./playlist-store.js');
            const playlist = createPlaylist();
            const store = await openPlaylistStore({ name: 'w18-selection-' + crypto.randomUUID() });
            let writes = 0, notifications = 0;
            const save = store.save; store.save = (...args) => { writes++; return save(...args); };
            await createPlaylistControls({ playlist, state: {}, loader: {}, openStore: async () => store });
            const entries = playlist.add(Array.from({ length: 30 }, (_, index) => ({
                locator: { kind: 'library', sourcePath: 'packs/long-source-name.zip', memberPath: index === 1 ? 'song0.mod' : 'song' + index + '.mod' },
                display: { name: index === 1 ? 'song0.mod' : 'song' + index + '-long-filename-for-ellipsis.mod', title: 'Full title ' + index + ' with detailed module information', format: 'mod', folder: 'packs/long-source-name.zip / modules and nested folders' },
            })));
            playlist.setCurrent(entries[0].id);
            const played = [], failures = new Map([[entries[0].id, 'Complete warning diagnostic with archive source information']]);
            const playback = { failures, playEntry(entry) { played.push(entry.id); playlist.setCurrent(entry.id); } };
            const view = wirePlaylistList(playlist, { playback, availability: entry => entry.id === entries[1].id ? 'library not connected — unavailable original source' : '' });
            playlist.subscribe(() => notifications++);
            await store.settled();
            window.w18 = { playlist, store, view, played, entries, get writes() { return writes; }, get notifications() { return notifications; }, initialWrites: writes };
            window.w18selected = () => [...document.querySelectorAll('#playlist-list tr[aria-selected=true]')].map(row => row.dataset.entryId);
            window.w18row = index => document.querySelectorAll('#playlist-list tr')[index];
        `);
        const click = async (selector, modifiers = 0, double = false) => {
            const point = await page.evaluate(`const target = ${selector}; target.scrollIntoView({ block: 'nearest', inline: 'nearest' }); const bounds = target.getBoundingClientRect(); return { x: bounds.x + Math.min(25, bounds.width / 2), y: bounds.y + bounds.height / 2 };`);
            for (let clickCount = 1; clickCount <= (double ? 2 : 1); clickCount++) for (const type of ['mousePressed', 'mouseReleased']) await page.send('Input.dispatchMouseEvent', { type, button: 'left', clickCount, modifiers, ...point });
        };
        const key = async (key, code, windowsVirtualKeyCode, modifiers = 0) => {
            for (const type of ['keyDown', 'keyUp']) await page.send('Input.dispatchKeyEvent', { type, key, code, windowsVirtualKeyCode, modifiers });
        };
        const selected = () => page.evaluate('return w18selected();');
        const ids = await page.evaluate('return w18.entries.map(entry => entry.id);');
        assert.deepEqual(await selected(), []);
        assert.equal(await page.evaluate("return document.getElementById('playlist-remove-selected').disabled;"), true);
        await click("w18row(1).querySelector('input')");
        assert.deepEqual(await selected(), [ids[1]]);
        assert.equal(await page.evaluate("return document.getElementById('playlist-select-all').indeterminate;"), true);
        await click("w18row(1).querySelector('input')", 0, true);
        assert.deepEqual(await selected(), [ids[1]], 'native double-click toggles twice without selecting another row');
        assert.deepEqual(await page.evaluate('return w18.played;'), [], 'checkbox double-click never starts playback');
        await key(' ', 'Space', 32);
        assert.deepEqual(await selected(), [], 'native checkbox Space toggles once');
        await key(' ', 'Space', 32);
        assert.deepEqual(await selected(), [ids[1]]);
        await click("document.getElementById('playlist-select-all')");
        assert.deepEqual(await selected(), ids, 'mixed header click selects every UUID, including unavailable and offscreen duplicates');
        await key(' ', 'Space', 32);
        assert.deepEqual(await selected(), [], 'native header Space clears all');
        await key(' ', 'Space', 32);
        assert.deepEqual(await selected(), ids);
        await key(' ', 'Space', 32);

        // Measure the actual table, including the native checkbox label/border height.
        const output = process.env.STARPLAYER_SCREENSHOT_DIR ?? '/tmp/starplayer-w18-playlist';
        await mkdir(output, { recursive: true });
        for (const [label, width, height] of [['desktop', 1280, 960], ['phone', 390, 844]]) {
            await page.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
            await page.evaluate("document.getElementById('playlist-panel').scrollIntoView({block:'start'}); document.getElementById('playlist-viewport').scrollLeft = 0;");
            const dimensions = await page.evaluate(`
                const viewport = document.getElementById('playlist-viewport'); viewport.scrollTop = 128;
                const bounds = viewport.getBoundingClientRect(), heading = document.querySelector('#playlist-table th').getBoundingClientRect();
                const name = document.querySelector('#playlist-list .playlist-name'), marker = document.querySelector('#playlist-list .playlist-marker');
                const unavailableFile = w18row(1).querySelector('.playlist-file'), warning = unavailableFile.querySelector('.playlist-availability-warning'), unavailableName = unavailableFile.querySelector('.playlist-name');
                const warningBounds = warning.getBoundingClientRect(), nameBounds = unavailableName.getBoundingClientRect();
                const result = { rowHeights: [...document.querySelectorAll('#playlist-list tr')].map(row => row.getBoundingClientRect().height), pageWidth: document.documentElement.scrollWidth, width: innerWidth, viewportWidth: bounds.width, viewportScrollWidth: viewport.scrollWidth, stickyOffset: heading.top - bounds.top, nameVisible: name.getBoundingClientRect().width > 0, markerVisible: marker.getBoundingClientRect().width > 0, filename: name.textContent, marker: marker.textContent, tooltip: w18row(0).title, warning: warning.textContent, warningTooltip: warning.title, warningAccessibleName: warning.getAttribute('aria-label'), warningColor: getComputedStyle(warning).color, warningWidth: warningBounds.width, warningBeforeFilename: warningBounds.right < nameBounds.left, unavailableText: unavailableFile.textContent, unavailableFilenameTooltip: unavailableName.title, filenameCellWidth: w18row(1).children[1].getBoundingClientRect().width, filenameCellTooltip: w18row(1).children[1].title };
                viewport.scrollTop = 0; return result;
            `);
            assert.ok(dimensions.rowHeights.every(value => value === 32), JSON.stringify(dimensions));
            assert.ok(dimensions.pageWidth <= dimensions.width, 'table does not widen the page');
            assert.ok(Math.abs(dimensions.stickyOffset - 1) < 1, 'header remains at scroll viewport top');
            assert.equal(dimensions.nameVisible, true); assert.equal(dimensions.markerVisible, true);
            assert.equal(dimensions.warning, '!');
            assert.equal(dimensions.warningTooltip, 'library not connected — unavailable original source');
            assert.equal(dimensions.warningAccessibleName, dimensions.warningTooltip);
            assert.equal(dimensions.warningColor, 'rgb(230, 198, 107)');
            assert.ok(dimensions.warningWidth > 0 && dimensions.warningWidth < 16, 'availability warning stays compact');
            assert.equal(dimensions.warningBeforeFilename, true);
            assert.doesNotMatch(dimensions.unavailableText, /library not connected/);
            assert.equal(dimensions.unavailableFilenameTooltip, 'song0.mod');
            assert.match(dimensions.filenameCellTooltip, /library not connected.*long-source-name.zip/);
            assert.match(dimensions.marker, /▶.*⚠/); assert.match(dimensions.tooltip, /Complete warning diagnostic.*long-source-name.zip/);
            if (label === 'phone') assert.ok(dimensions.viewportScrollWidth > dimensions.viewportWidth, 'phone table scrolls horizontally inside panel');
            const screenshot = await page.send('Page.captureScreenshot', { format: 'png' });
            const path = join(output, `playlist-${mode}-${label}.png`); await writeFile(path, Buffer.from(screenshot.data, 'base64'));
            report[label] = { ...dimensions, screenshot: path };
        }
        await page.send('Emulation.setDeviceMetricsOverride', { width: 1280, height: 960, deviceScaleFactor: 1, mobile: false });
        await click('w18row(0).children[1]');
        await click('w18row(2).children[1]', 2);
        assert.deepEqual(await selected(), [ids[0], ids[2]]);
        await click('w18row(4).children[1]', 8);
        assert.deepEqual(await selected(), ids.slice(2, 5));
        assert.equal(await page.evaluate("return getSelection().toString();"), '', 'native modified clicks suppress page text selection');
        await key('ArrowUp', 'ArrowUp', 38, 8);
        assert.deepEqual(await selected(), ids.slice(2, 4));
        await key('End', 'End', 35);
        assert.deepEqual(await selected(), [ids[29]]);
        assert.equal(await page.evaluate('return document.activeElement.dataset.entryId;'), ids[29]);
        await key('Home', 'Home', 36);
        assert.deepEqual(await selected(), [ids[0]]);
        await key('PageDown', 'PageDown', 34);
        const pageSize = await page.evaluate("return Math.max(1, Math.floor((document.getElementById('playlist-viewport').clientHeight - 32) / 32));");
        assert.deepEqual(await selected(), [ids[pageSize]]);
        await key('PageUp', 'PageUp', 33);
        assert.deepEqual(await selected(), [ids[0]]);
        await key('a', 'KeyA', 65, 2);
        assert.deepEqual(await selected(), ids);
        await key('Escape', 'Escape', 27);
        assert.deepEqual(await selected(), []);
        await key('Delete', 'Delete', 46);
        assert.equal(await page.evaluate('return w18.notifications;'), 0, 'empty Delete publishes nothing');
        assert.equal(await page.evaluate('return w18.writes;'), await page.evaluate('return w18.initialWrites;'), 'all native selection interactions leave persistence untouched');

        await click("w18row(1).querySelector('input')");
        await page.evaluate("w18.playlist.updateDisplay([{id:w18.entries[1].id, patch:{title:'New metadata'}}]); w18.playlist.setCurrent(w18.entries[1].id); w18.view.render();");
        assert.deepEqual(await selected(), [ids[1]]);
        assert.equal(await page.evaluate("return document.activeElement === w18row(1).querySelector('input');"), true, 'checkbox focus survives metadata/current/failure rerenders');
        await page.evaluate('w18row(1).focus();');
        await key('ArrowDown', 'ArrowDown', 40, 1);
        assert.equal(await page.evaluate('return w18row(2).dataset.entryId;'), ids[1]);
        assert.deepEqual(await selected(), [ids[1]]);
        await page.evaluate(`
            const transfer = new DataTransfer();
            w18row(2).dispatchEvent(new DragEvent('dragstart', { dataTransfer: transfer, bubbles: true }));
            w18row(5).dispatchEvent(new DragEvent('drop', { dataTransfer: transfer, bubbles: true, cancelable: true }));
        `);
        assert.equal(await page.evaluate('return w18row(5).dataset.entryId;'), ids[1]);
        assert.deepEqual(await selected(), [ids[1]], 'single dragged UUID keeps selection');
        await page.evaluate('w18row(5).focus();');
        await key('Enter', 'Enter', 13);
        await click('w18row(5).children[1]', 0, true);
        assert.deepEqual(await page.evaluate('return w18.played;'), [ids[1], ids[1]], 'Enter and row double-click still play');
        await key('Escape', 'Escape', 27);
        await click('w18row(0).children[1]');
        await click('w18row(29).children[1]', 2);
        const before = await page.evaluate('return {writes:w18.writes, notifications:w18.notifications};');
        await click("document.getElementById('playlist-remove-selected')");
        assert.equal(await page.evaluate('return w18.playlist.entries.length;'), 28);
        assert.equal(await page.evaluate('return w18.notifications;'), before.notifications + 1);
        assert.equal(await page.evaluate('return w18.writes;'), before.writes + 1);
        assert.equal(await page.evaluate('return document.activeElement === w18row(27);'), true, 'bulk removal focuses surviving neighbour');
        await key('a', 'KeyA', 65, 2); await key('Delete', 'Delete', 46);
        assert.equal(await page.evaluate('return w18.playlist.entries.length;'), 0);
        assert.equal(await page.evaluate("return document.activeElement.id;"), 'playlist-add');
        await page.evaluate(`
            w18.playlist.restore({entries:w18.entries.slice(0,2),currentId:null});
            w18row(0).click(); w18.view.dispose();
            const {wirePlaylistList}=await import('./playlist.js'); w18.view=wirePlaylistList(w18.playlist);
        `);
        assert.deepEqual(await selected(), [], 'a new page view begins without selection');
        await page.evaluate('await w18.store.settled(); w18.view.dispose(); w18.store.close();');
        assert.deepEqual(page.consoleErrors, []);
        report.nativeInput = true; report.bulkRemoval = true; report.noSelectionWrites = true;
        return report;
    } finally { if (page) await page.close(); server.stop(); }
}
