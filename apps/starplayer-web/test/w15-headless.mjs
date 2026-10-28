// W15 integration scenarios share the CDP driver with headless.mjs. Fixtures are
// synthetic, sources are origin-private or in-memory, and every worker is real.
import assert from 'node:assert/strict';

export async function runW15({ Page, executable, mode, startDevServer, syntheticMod, buildZip, verifyPlaylistStore }) {
    const server = await startDevServer(mode !== 'plain');
    let page;
    const report = {};
    try {
        page = await Page.open(executable, `http://127.0.0.1:${server.port}/`);
        await page.waitFor('W15 startup', "document.documentElement.dataset.playerReady === 'true'");
        await page.evaluate(`if (${JSON.stringify(mode)} === 'fallback') document.getElementById('force-fallback').checked = true;`);
        const mod = syntheticMod({ title: 'W15 actual metadata' });
        const nested = buildZip([{ name: 'inner.zip', bytes: buildZip([{ name: 'nested.mod', bytes: mod }]) }]);
        await page.evaluate(`
            const root = await navigator.storage.getDirectory();
            const parentA = await root.getDirectoryHandle('W15-A', { create: true });
            const parentB = await root.getDirectoryHandle('W15-B', { create: true });
            window.w15A = await parentA.getDirectoryHandle('library', { create: true });
            window.w15B = await parentB.getDirectoryHandle('library', { create: true });
            async function write(root, path, bytes) {
                const parts = path.split('/'); let directory = root;
                for (const part of parts.slice(0, -1)) directory = await directory.getDirectoryHandle(part, { create: true });
                const file = await directory.getFileHandle(parts.at(-1), { create: true });
                const writable = await file.createWritable(); await writable.write(bytes); await writable.close();
            }
            window.w15write = write;
            const bytes = Uint8Array.from(${JSON.stringify([...mod])});
            await write(w15A, 'loose/early.mod', bytes); await write(w15A, 'loose/late.mod', bytes);
            await write(w15A, 'packs/nested.zip', Uint8Array.from(${JSON.stringify([...nested])}));
            await write(w15A, 'broken.mod', new Uint8Array([1, 2, 3]));
            await write(w15B, 'loose/early.mod', bytes);
            const sha = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(v => v.toString(16).padStart(2, '0')).join('');
            await write(w15A, 'comments.csv', 'sha256,instrument_names,song_message\\n' + sha + ',W15 sidecar composer,Sidecar-only message');
            window.w15reads = [];
            const originalRead = File.prototype.arrayBuffer;
            File.prototype.arrayBuffer = function() { w15reads.push(this.name); return originalRead.call(this); };
            const originalEntries = FileSystemDirectoryHandle.prototype.entries;
            window.w15gate = new Promise(resolve => window.w15release = resolve);
            FileSystemDirectoryHandle.prototype.entries = async function*() {
                const entries = []; for await (const entry of originalEntries.call(this)) entries.push(entry);
                entries.sort(([a], [b]) => a.localeCompare(b));
                for (const entry of entries) {
                    if (this.name === 'loose' && entry[0] === 'late.mod') {
                        if (window.w15omitLate) continue;
                        if (window.w15gate) await window.w15gate;
                    }
                    yield entry;
                }
            };
            window.showDirectoryPicker = async () => w15A;
            document.getElementById('library-folder').click();
        `);
        await page.waitFor('progressive OPFS rows before late gate', "document.getElementById('library-count').textContent.includes('2 of 2') && document.getElementById('library-summary').textContent.includes('discovery running')");
        assert.deepEqual(await page.evaluate("return w15reads.filter(name => /\\.(mod|s3m)$/i.test(name));"), []);
        await page.waitFor('sidecar awaiting module hashes', "document.getElementById('library-summary').textContent.includes('awaiting metadata/hashes')");
        await page.evaluate(`
            const row = [...document.querySelectorAll('.library-track')].find(row => row.textContent.includes('early.mod'));
            row.click(); document.getElementById('library-add-selected').click();
            const transfer = new DataTransfer(); transfer.items.add(new File(['source_path,title,format_detected\\nloose/late.mod,Provisional,xm\\nloose/late.mod,Duplicate,xm'], 'saved.csv'));
            const input = document.getElementById('playlist-csv'); input.files = transfer.files; input.dispatchEvent(new Event('change'));
        `);
        await page.waitFor('pre-discovery playlist', "document.getElementById('playlist-count').textContent === '3' && document.getElementById('playlist-status').textContent.includes('2 pending discovery')");
        assert.deepEqual(await page.evaluate("return w15reads.filter(name => /\\.(mod|s3m)$/i.test(name));"), []);
        await page.evaluate("document.querySelector('#playlist-list tr').dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true}));");
        await page.waitFor('early playlist playback', "document.getElementById('title').textContent === 'W15 actual metadata' && document.getElementById('transport-chip').textContent === 'playing'");
        await page.waitFor('progress range for automatic advance', "Number(document.getElementById('progress').max) > 1000");
        await page.evaluate("const p=document.getElementById('progress');p.value=String(Number(p.max)-4800);p.dispatchEvent(new Event('change'));");
        await page.waitFor('advance plays not-yet-discovered locator', "document.querySelectorAll('#playlist-list tr')[1].classList.contains('current') && document.querySelectorAll('#playlist-list tr')[1].textContent.includes('W15 actual metadata') && document.getElementById('transport-chip').textContent === 'playing'");
        await page.evaluate("document.querySelectorAll('#playlist-list tr')[2].dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true}));");
        await page.waitFor('explicit not-yet-discovered locator', "document.querySelectorAll('#playlist-list tr')[2].classList.contains('current') && document.querySelectorAll('#playlist-list tr')[2].textContent.includes('W15 actual metadata')");
        report.playBeforeDiscovery = true;
        report.commandTransport = await page.evaluate("return document.getElementById('command-transport').textContent;");
        if (mode === 'sab') assert.match(report.commandTransport, /SharedArrayBuffer/);
        else assert.match(report.commandTransport, /postMessage/);
        await page.evaluate("document.getElementById('library-stop-discovery').click(); window.w15release(); window.w15gate = null;");
        await page.waitFor('stopped walk remains paused', "document.getElementById('library-summary').textContent.includes('discovery paused')");
        assert.equal(await page.evaluate("return document.getElementById('library-count').textContent;"), '2 of 2');
        await page.evaluate("document.getElementById('library-resume-discovery').click();");
        await page.waitFor('real nested catalog complete', "document.getElementById('library-summary').textContent.includes('discovery complete') && document.getElementById('library-count').textContent === '4 of 4'");
        await page.waitFor('sidecar rejoined after playback digest', "document.getElementById('library-summary').textContent.includes('2 tracks matched')");
        await page.evaluate("document.getElementById('library-scan-metadata').click();");
        await page.waitFor('real metadata scan error and nested metadata', "document.getElementById('library-progress').textContent.includes('Metadata scan complete') && document.querySelector('.metadata-error')", 40000);
        assert.equal(await page.evaluate("return document.getElementById('title').textContent;"), 'W15 actual metadata');
        assert.equal(await page.evaluate("return document.getElementById('transport-chip').textContent;"), 'playing');
        await page.evaluate(`
            const broken = [...document.querySelectorAll('.library-track')].find(row => row.textContent.includes('broken.mod'));
            broken.click();
        `);
        assert.equal(await page.evaluate("return !document.getElementById('library-details').hidden && !!document.querySelector('[data-retry-metadata]') && !document.getElementById('library-add-selected').disabled;"), true);
        await page.evaluate("document.querySelector('[data-retry-metadata]').click();");
        await page.waitFor('explicit real retry read', "w15reads.filter(name => name === 'broken.mod').length >= 2");
        const identityA = await page.evaluate(`
            const { storedDirectoryHandle, folderIdentity, createDirectorySource } = await import('./library-source.js');
            const handle = await storedDirectoryHandle();
            if (!await handle.isSameEntry(w15A)) throw new Error('persisted OPFS handle differs');
            return (await folderIdentity(await createDirectorySource(handle))).identity.sha256;
        `);
        await page.evaluate("window.showDirectoryPicker = async () => { throw new DOMException('cancel', 'AbortError'); }; document.getElementById('library-folder').click();");
        assert.equal(await page.evaluate("return document.getElementById('library-count').textContent;"), '4 of 4');
        await page.evaluate("window.showDirectoryPicker = async () => w15B; document.getElementById('library-folder').click();");
        await page.waitFor('different same-named OPFS root', "document.getElementById('library-count').textContent === '1 of 1' && document.getElementById('library-summary').textContent.includes('discovery complete')");
        const identityB = await page.evaluate("const { folderIdentity, createDirectorySource } = await import('./library-source.js'); return (await folderIdentity(await createDirectorySource(w15B))).identity.sha256;");
        assert.notEqual(identityA, identityB);
        await page.evaluate("window.w15omitLate = true; window.showDirectoryPicker = async () => w15A; document.getElementById('library-folder').click();");
        await page.waitFor('A returns with absent saved row', "document.getElementById('library-count').textContent === '3 of 3' && document.getElementById('library-summary').textContent.includes('discovery complete') && document.querySelectorAll('#playlist-list tr')[1].textContent.includes('not found in scan')");
        assert.equal(await page.evaluate("const { folderIdentity, createDirectorySource } = await import('./library-source.js'); return (await folderIdentity(await createDirectorySource(w15A))).identity.sha256;"), identityA);
        for (const index of [1, 0]) {
            await page.evaluate(`document.querySelectorAll('#playlist-list tr')[${index}].dispatchEvent(new KeyboardEvent('keydown', {key:'Enter', bubbles:true}));`);
            await page.waitFor('complete missing-row explicit playback', `document.querySelectorAll('#playlist-list tr')[${index}].classList.contains('current') && document.getElementById('transport-chip').textContent === 'playing'`);
        }
        await page.waitFor('progress range for automatic advance', "Number(document.getElementById('progress').max) > 1000");
        await page.evaluate("const p=document.getElementById('progress');p.value=String(Number(p.max)-4800);p.dispatchEvent(new Event('change'));");
        await page.waitFor('complete missing-row advance playback', "document.querySelectorAll('#playlist-list tr')[1].classList.contains('current') && document.getElementById('transport-chip').textContent === 'playing'");
        report.notFoundStillPlayable = true;
        // Persisted handles and identity cross an actual document reload. OPFS permission
        // is already granted; native picker prompts remain a separate manual gate.
        await page.waitFor('playlist metadata persisted', "await (async () => { const s = await (await import('./playlist-store.js')).openPlaylistStore(); const ok = s.state.entries.length === 3 && s.state.entries.every(e => e.display.metadataState === 'ready'); s.close(); return ok; })()");
        await page.send('Page.addScriptToEvaluateOnNewDocument', { source: `window.showDirectoryPicker = async () => (await (await navigator.storage.getDirectory()).getDirectoryHandle('W15-A')).getDirectoryHandle('library');` });
        await page.send('Page.reload');
        await page.waitFor('real saved OPFS handle restored', "document.documentElement.dataset.playerReady === 'true' && !document.getElementById('library-reconnect').hidden");
        await page.evaluate(`if (${JSON.stringify(mode)} === 'fallback') document.getElementById('force-fallback').checked = true; document.getElementById('library-reconnect').click();`);
        await page.waitFor('reconnected OPFS complete', "document.getElementById('library-summary').textContent.includes('discovery complete') && document.getElementById('library-count').textContent === '4 of 4'");
        assert.equal(await page.evaluate("const { storedDirectoryHandle, folderIdentity, createDirectorySource } = await import('./library-source.js'); const handle = await storedDirectoryHandle(); return (await folderIdentity(await createDirectorySource(handle))).identity.sha256;"), identityA);
        // Gaining a root index chooses CSV; explicit Browse reuses folder identity.
        await page.evaluate(`
            const handle = await (await import('./library-source.js')).storedDirectoryHandle();
            const file = await handle.getFileHandle('index.csv', {create:true}); const out = await file.createWritable(); await out.write('source_path\\nloose/early.mod'); await out.close();
            document.getElementById('library-folder').click();
        `);
        await page.waitFor('CSV gain chooses index', "document.getElementById('library-count').textContent === '1 of 1' && document.getElementById('library-summary').textContent.includes('1 tracks,')");
        await page.evaluate("document.getElementById('library-browse-folder').click();");
        await page.waitFor('CSV bypass discovery', "document.getElementById('library-count').textContent === '4 of 4' && document.getElementById('library-summary').textContent.includes('discovery complete')");
        assert.equal(await page.evaluate("const { storedDirectoryHandle, folderIdentity, createDirectorySource } = await import('./library-source.js'); return (await folderIdentity(await createDirectorySource(await storedDirectoryHandle()))).identity.sha256;"), identityA);
        report.opfs = { identityA, identityB, reload: true, csvGainAndBypass: true, cancelPreserves: true };
        report.store = await page.evaluate(`return await (${verifyPlaylistStore.toString()})((await import('./playlist-store.js')).openPlaylistStore);`);
        report.workers = await workerScenarios(page, mod, buildZip([{ name: 'flat.mod', bytes: mod }]));
        assert.deepEqual(page.consoleErrors, []);
    } finally {
        if (page) await page.close();
        server.stop();
    }
    return report;
}

async function workerScenarios(page, mod, flatZip) {
    return page.evaluate(`
        const { createLibraryWorkerRuntime } = await import('./library-worker-runtime.js');
        const { createDirectorySource, storedDirectoryHandle } = await import('./library-source.js');
        const runtime = createLibraryWorkerRuntime({source: await createDirectorySource(await storedDirectoryHandle())});
        try {
            const listed = await runtime.enqueue({generation:991, kind:'catalog', sourcePath:'packs/nested.zip'}).result;
            if (listed.payload.entries.length !== 1 || listed.payload.entries[0].containerMember !== 'inner.zip') throw new Error('real nested catalog');
            let inspected;
            const group = (generation, locator) => ({generation,kind:'inspect-group',sourcePath:locator.sourcePath,members:[{token:'one',locator,version:0}],purpose:'scan'});
            await runtime.enqueue(group(991,{sourcePath:'packs/nested.zip',containerMember:'inner.zip',memberPath:'nested.mod',rawHex:''}),{onMember:frame=>{inspected=frame.payload;}}).result;
            if (inspected.metadata.title !== 'W15 actual metadata' || inspected.metadata.format !== 'mod' || inspected.sha256.length !== 64) throw new Error('real nested metadata');
            const bytes = Uint8Array.from(${JSON.stringify([...mod])});
            const sha256 = [...new Uint8Array(await crypto.subtle.digest('SHA-256',bytes))].map(value=>value.toString(16).padStart(2,'0')).join('');
            if (!bytes.byteLength || sha256 !== inspected.sha256) throw new Error('native borrowed hash bytes');
            const cancelled = runtime.enqueue(group(991,{sourcePath:'loose/early.mod'})); cancelled.cancel();
            const error = await cancelled.result.catch(error => error);
            if (error.code !== 'aborted') throw new Error('real worker cancellation');
            const flatBytes = Uint8Array.from(${JSON.stringify([...flatZip])});
            const flatRuntime = createLibraryWorkerRuntime({source:{size:()=>flatBytes.length,readFile:async()=>flatBytes.slice()}});
            try {
                const flatCatalog = await flatRuntime.enqueue({generation:992,kind:'catalog',sourcePath:'flat.zip'}).result;
                let flatMetadata; await flatRuntime.enqueue(group(992,{sourcePath:'flat.zip',containerMember:'',memberPath:'flat.mod',rawHex:''}),{onMember:frame=>{flatMetadata=frame.payload;}}).result;
                if (flatCatalog.payload.entries.length !== 1 || flatCatalog.payload.entries[0].containerMember !== '' || flatMetadata.metadata.title !== 'W15 actual metadata') throw new Error('real ordinary ZIP catalog/metadata');
            } finally {flatRuntime.dispose();}
            return {ordinaryCatalogAndMetadata:true,nestedCatalog:true, nestedMetadata:true, sha256, cancellation:true};
        } finally { runtime.dispose(); }
    `);
}

export async function runW15View({ Page, executable, mode, startDevServer }) {
    const server = await startDevServer(mode !== 'plain');
    let page;
    try {
        page = await Page.open(executable, `http://127.0.0.1:${server.port}/`);
        await page.waitFor('W15 DOM startup', "document.documentElement.dataset.playerReady === 'true'");
        const result = await page.evaluate(`
            const model = await import('./library-index.js');
            const { createLibraryView } = await import('./library-view.js');
            const originalPanel = document.getElementById('library-panel');
            originalPanel.replaceWith(originalPanel.cloneNode(true));
            document.getElementById('library-match-popover')?.remove();
            document.getElementById('library-heading').replaceChildren();
            const added = [], retries = [], played = [];
            const view = createLibraryView({playlist:{add:items => added.push(items)},play:row => played.push(row.id)});
            const el = name => document.getElementById('library-' + name);
            const check = (ok, why) => { if (!ok) throw new Error(why); };
            const equal = (a,b,why) => check(JSON.stringify(a) === JSON.stringify(b), why + ': ' + JSON.stringify({actual:a,expected:b}));
            const tick = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
            const status = phase => ({phase,filesExamined:0,archivesPending:1,tracksFound:0,issues:[],issueCount:0,metadataPhase:'idle',runtimeError:'',metadataCounts:{ready:0,unknown:0,error:0,hashesPending:0},metadataIncomplete:true,sidecarCounts:{joined:0,unmatched:0,awaitingHashes:0}});
            function context(records, phase='running') {
                const library = model.createDiscoveredLibrary({name:'DOM fixture'});
                model.appendDiscoveredRows(library,records);
                return {mode:'discovered',library,identity:{sha256:'a'.repeat(64),name:'DOM fixture'},discoveryStatus:status(phase),discovery:{retryMetadata:locator=>retries.push(locator)}};
            }
            const record = (path, extra={}) => ({sourcePath:path,containerMember:'',memberPath:'',rawHex:'',format:'mod',size:null,timestamp:'',playable:true,error:'',...extra});
            let ctx;
            const set = async records => { ctx=context(records); view.setLibrary(ctx); await tick(); };
            const update = async records => { const ids=model.appendDiscoveredRows(ctx.library,records); view.updateLibrary(ctx,{changedRowIds:ids,rowChanges:ctx.library.lastRowChanges}); await tick(); };
            const patch = async updates => { const ids=model.updateDiscoveredRows(ctx.library,updates); view.updateLibrary(ctx,{changedRowIds:ids,rowChanges:ctx.library.lastRowChanges,metadataUpdates:updates}); await tick(); };
            const filter = async value => { el('search').value=value; el('search').dispatchEvent(new Event('input')); await new Promise(resolve=>setTimeout(resolve,100)); await tick(); };
            const format = async value => {el('format').value=value;el('format').dispatchEvent(new Event('change'));await tick();};
            const row = id => document.querySelector('[data-row-id="'+id+'"]');
            const scroll = async value => {el('viewport').scrollTop=value;el('viewport').dispatchEvent(new Event('scroll'));await tick();};
            const anchor = () => { const index=Math.floor(el('viewport').scrollTop/32); const r=[...el('rows').children].find(row=>Number(row.getAttribute('aria-rowindex'))===index+2); return {id:Number(r.dataset.rowId),offset:el('viewport').scrollTop-index*32,y:r.getBoundingClientRect().top-el('viewport').getBoundingClientRect().top}; };
            const reports=[];
            check(el('select-all').disabled&&el('add-selected').disabled,'W16 initial no-library disabled');
            await set([]);check(el('select-all').disabled&&el('add-selected').disabled,'W16 empty result disabled');
            await set(Array.from({length:240},(_,i)=>record('group/track'+String(i).padStart(4,'0')+'.mod')));
            await scroll(80*32+9); const before=anchor();
            await update([record('group/000-first.mod')]);
            equal(anchor(),before,'E-append-scrolled stable ID/pixel offset'); reports.push('E-append-scrolled');
            // Metadata removes the anchor from an existing format filter; nearest old
            // order survivor wins, with an end-of-list clamp when the list shrinks.
            await format('mod'); await scroll(80*32+11); const removed=anchor();
            await patch([{locator:ctx.library.rows[removed.id],patch:{format:'xm',metadataState:'ready'}}]);
            equal(anchor().id,removed.id+1,'E-anchor-removed nearest old-order survivor');
            equal(anchor().offset,11,'E-anchor-removed offset');
            await scroll(999999);
            await patch(ctx.library.rows.filter(r=>r.id>10&&r.format==='mod').map(r=>({locator:r,patch:{format:'xm'}})));
            equal(el('viewport').scrollTop,Math.max(0,ctx.library.rows.filter(r=>r.format==='mod').length*32-el('viewport').clientHeight),'E-anchor-removed clamps small result'); reports.push('E-anchor-removed');
            await set(Array.from({length:160},(_,i)=>record('folder'+String(i%40).padStart(2,'0')+'/sub/track'+String(i).padStart(4,'0')+'.mod')));
            row(0).click(); el('grid').focus(); view.setPlaying(0);
            await update([record('folder00/sub/new.mod')]);
            check(document.activeElement===el('grid'),'E-selection grid focus');
            check(row(0).getAttribute('aria-selected')==='true'&&row(0).classList.contains('playing'),'E-selection selected/playing');
            row(0).querySelector('[data-select]').focus();
            await patch([{locator:ctx.library.rows[0],patch:{title:'Enriched'}}]);
            check(document.activeElement.closest('[data-row-id]')?.dataset.rowId==='0','E-selection row-checkbox focus');
            await filter('no-match'); await filter('');
            el('add-selected').click(); equal(added.at(-1).map(e=>e.locator.sourcePath),[ctx.library.rows[0].sourcePath],'E-selection hidden selection preserved');
            el('browse').click();
            document.querySelector('[data-scope="folder00"]').click();
            document.querySelector('[data-scope="folder00"] [data-tree-toggle]').click();
            await filter('track'); await format('mod');
            const tree=el('tree'); tree.scrollTop=100;
            document.querySelector('[data-scope="folder00"]').dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true}));
            tree.scrollTop=100;
            const treeScroll=tree.scrollTop;
            await update([record('folder00/sub/track-new.mod')]);
            equal(document.activeElement.dataset.scope,'folder00/sub','E-tree focus survives');
            equal(tree.scrollTop,treeScroll,'E-tree scroll survives');
            equal(el('scope').textContent,'folder00','E-tree scope survives');
            check(el('search').value==='track'&&el('format').value==='mod','E-tree search/format survive');
            equal(document.querySelector('[data-scope="folder00"]').getAttribute('aria-expanded'),'true','E-tree expansion survives');
            reports.push('E-selection-focus-tree-survive');
            await set([record('first.mod')]);
            check(el('comments-toggle').hidden,'comments initially hidden');
            await patch([{locator:ctx.library.rows[0],patch:{instrumentNames:'First composer',format:'xm',metadataState:'ready'}}]);
            check(!el('comments-toggle').hidden,'first metadata enables comments toggle');
            await filter('composer'); check(row(0)?.querySelector('mark')?.textContent==='composer','first comment highlighted');
            el('search-comments').click(); await tick(); equal(el('count').textContent,'0 of 1','comment toggle excludes');
            await filter('text:composer'); check(row(0)?.querySelector('mark'),'explicit text query highlights');
            el('search-comments').click(); await filter(''); await format('xm');
            await patch([{locator:ctx.library.rows[0],patch:{format:'it'}}]);
            equal(el('format').value,'xm','selected format preserved after actual format changes');
            check([...el('format').options].some(o=>o.value==='xm'),'selected absent format retained');
            equal(el('count').textContent,'0 of 1','actual format removes filtered row');
            reports.push('E-first-comments-and-format-filter');
            await set([record('bad.mod',{metadataState:'error',error:'Inspection failed'}),record('unplayable.zip',{memberPath:'large.mod',playable:false,error:'64 MiB limit'})]);
            row(0).click();
            check(!el('details').hidden&&el('details').textContent.includes('Inspection failed'),'error details without comments');
            equal(row(0).title,'Inspection failed','row error tooltip');
            equal(row(0).querySelector('.library-error-marker').title,'Inspection failed','error marker tooltip');
            check(el('details').open,'details expanded by default');
            el('details-title').click();await tick();check(!el('details').open,'native summary collapses details');
            row(0).click();check(el('details').hidden,'repeat ordinary click clears sole selection and details');
            row(0).click();check(!el('details').hidden&&!el('details').open,'selection restores collapsed disclosure');
            row(0).querySelector('[data-select]').click();check(el('details').hidden,'checkbox deselection hides details');
            row(0).querySelector('[data-select]').click();
            el('grid').dispatchEvent(new KeyboardEvent('keydown',{key:'Escape',bubbles:true}));
            check(el('details').hidden,'Escape hides details');
            row(0).click();
            check(!el('add-selected').disabled,'metadata error remains playable');
            equal(el('unplayable-count').textContent,'1','live unplayable count');
            el('unplayable').click();await tick();check(row(1)?.title.includes('64 MiB'),'structural error shown');
            equal(row(1).querySelector('.library-error-marker').title,'64 MiB limit','unplayable marker tooltip');
            row(1).click();check(!el('details').open&&el('details').textContent.includes('64 MiB limit'),'collapsed details follow new selection');
            const disclosureSummary=el('details-title');disclosureSummary.click();await tick();
            check(el('details').open,'native summary expands details');
            row(0).click();check(el('details').open,'expanded disclosure survives selection change');
            el('select-all').click();el('select-all').click();check(el('details').hidden,'heading deselection hides details');
            row(0).click();
            await update([record('more.zip',{memberPath:'encrypted.mod',playable:false,error:'encrypted'})]);
            equal(el('unplayable-count').textContent,'2','unplayable count updates');
            if(row(0).getAttribute('aria-selected')!=='true')row(0).click(); const oldRetry=document.querySelector('[data-retry-metadata]');
            const readsBefore=retries.length;
            row(0).querySelector('[data-select]').click();row(0).querySelector('[data-select]').click();el('add-selected').click();
            equal(retries.length,readsBefore,'selection/add delegates no retry or I/O');
            oldRetry.click();equal(retries.at(-1),{sourcePath:'bad.mod',containerMember:'',memberPath:''},'exact row retry locator');
            reports.push('E-error-details','E-row-retry-no-I/O','LF-selection-disclosure-marker-tooltips');
            await set(Array.from({length:240},(_,index)=>record((index<120?'folder/':'other/')+'track'+String(index).padStart(4,'0')+'.mod')));
            const header=el('select-all');
            const headerState=()=>[header.checked,header.indeterminate,header.disabled];
            equal(headerState(),[false,false,false],'W16 initial heading');
            row(0).querySelector('[data-select]').click();row(1).querySelector('[data-select]').click();
            equal(headerState(),[false,true,false],'W16 independent toggles mixed');
            header.focus();header.click();
            equal(headerState(),[true,false,false],'W16 all includes offscreen');
            await update([record('folder/new.mod')]);
            equal(headerState(),[false,true,false],'W16 future discovery unselected');
            check(document.activeElement===header,'W16 heading focus survives update');
            check(!row(240).querySelector('[data-select]').checked,'W16 new row unchecked');
            await filter('other/');header.click();
            equal(headerState(),[false,false,false],'W16 clear filtered results');
            await filter('folder/');equal(headerState(),[false,true,false],'W16 hidden selections retained');
            header.click();equal(headerState(),[true,false,false],'W16 filtered all');
            await scroll(100*32);check([...el('rows').querySelectorAll('[data-select]')].every(input=>input.checked),'W16 scroll checked state');
            await scroll(0);row(240).querySelector('[data-select]').focus();
            await patch([{locator:ctx.library.rows[240],patch:{title:'Updated'}}]);
            check(document.activeElement.matches('[data-select]')&&document.activeElement.checked,'W16 metadata preserves checkbox focus/state');
            await scroll(100*32);check(document.activeElement===el('grid'),'W16 virtualized checkbox focus falls back');
            await filter('');el('browse').click();document.querySelector('[data-scope="other"]').click();
            equal(headerState(),[false,false,false],'W16 folder scoped none');
            header.click();equal(headerState(),[true,false,false],'W16 folder scoped all');
            header.click();await filter('nothing-matches');equal(headerState(),[false,false,true],'W16 empty disabled');
            reports.push('W16-checkbox-result-scope-focus');
            const obsolete=ctx; await set([record('replacement.mod')]);
            const html=el('rows').innerHTML;oldRetry.click();
            model.appendDiscoveredRows(obsolete.library,[record('stale.mod')]);view.updateLibrary(obsolete,{changedRowIds:[2]});await tick();
            equal(el('rows').innerHTML,html,'stale context ignored');equal(retries.length,readsBefore+1,'detached old retry ignored');reports.push('E-stale-context-ignored');
            await set([record('folder/a.mod'),record('other/b.mod')]);el('browse').click();
            const rootAdd=document.querySelector('[data-scope=""] > .library-tree-row [data-tree-add]');
            check(rootAdd.title==='Add discovered tracks in All folders','partial root label');
            check(document.querySelector('[data-scope="folder"] [data-tree-add]').title==='Add discovered tracks in folder','partial folder label');
            rootAdd.click();const snapshot=added.at(-1);equal(snapshot.length,2,'root snapshot size');
            document.querySelector('[data-scope="folder"] [data-tree-add]').click();const folderSnapshot=added.at(-1);equal(folderSnapshot.length,1,'folder snapshot size');
            await update([record('folder/later.mod')]);equal(snapshot.length,2,'root excludes later rows');equal(folderSnapshot.length,1,'folder excludes later rows');
            ctx.discoveryStatus=status('complete');view.updateLibrary(ctx,{statusChanged:true});
            check(!document.querySelector('[data-scope="folder"] [data-tree-add]').title.includes('discovered'),'complete folder label');
            reports.push('E-folder-snapshot-add');
            return reports;
        `);
        assert.deepEqual(page.consoleErrors, []);
        return result;
    } finally { if(page) await page.close();server.stop(); }
}

export async function runW15Stress({Page, executable, mode, startDevServer}) {
    const server=await startDevServer(mode!=='plain');let page;
    try {
        page=await Page.open(executable,`http://127.0.0.1:${server.port}/`);
        await page.waitFor('W15 stress startup',"document.documentElement.dataset.playerReady === 'true'");
        const result = await page.evaluate(`
            const {createDirectorySource,createFileListSource}=await import('./library-source.js');
            const {createDiscoveredLibrary}=await import('./library-index.js');
            const {createDiscoveryController}=await import('./library-discovery.js');
            const {createLibraryWorkerRuntime}=await import('./library-worker-runtime.js');
            const {createLibraryView}=await import('./library-view.js');
            const originalPanel = document.getElementById('library-panel');
            originalPanel.replaceWith(originalPanel.cloneNode(true));
            document.getElementById('library-match-popover')?.remove();
            document.getElementById('library-heading').replaceChildren();
            const el=name=>document.getElementById('library-'+name);
            const check=(ok,why)=>{if(!ok)throw new Error(why);};
            let reads=0,beats=0;const heartbeat=setInterval(()=>beats++,0);
            const wait=async predicate=>{const until=performance.now()+90000;while(!await predicate()){if(performance.now()>until)throw new Error('stress timed out '+el('count').textContent);await new Promise(r=>setTimeout(r,10));}};
            const view=createLibraryView({playlist:{add:items=>{window.w15stressAdded=items.length;}},play:()=>{}});
            const summary=[];
            try {
                for(const kind of ['directory','filelist']) {
                    const startBeats=beats;
                    const fakeFiles=Array.from({length:40000},(_,i)=>({name:'track'+String(i).padStart(5,'0')+'.mod',webkitRelativePath:'stress/group/track'+String(i).padStart(5,'0')+'.mod',size:1,lastModified:0,arrayBuffer(){reads++;throw new Error('unexpected loose read');},slice(){reads++;throw new Error('unexpected loose header read');}}));
                    const nested={kind:'directory',name:'group',async *entries(){for(const file of fakeFiles)yield [file.name,{kind:'file',name:file.name,getFile(){reads++;throw new Error('unexpected getFile');}}];}};
                    const root={kind:'directory',name:'stress',async *entries(){yield ['group',nested];}};
                    const source=kind==='directory'?await createDirectorySource(root):await createFileListSource(fakeFiles);
                    const mappedBeats=beats-startBeats;
                    if(kind==='filelist')check(mappedBeats>0,'FileList map construction yields');
                    const ctx={mode:'discovered',source,library:createDiscoveredLibrary({name:kind}),identity:{sha256:'d'.repeat(64),name:kind}};
                    const enumerationStarted=performance.now(),publications=[];
                    createDiscoveryController({context:ctx,onUpdate:(context,change)=>{publications.push({milliseconds:performance.now()-enumerationStarted,rows:context.library.rows.length,phase:context.discoveryStatus.phase});view.updateLibrary(context,change);}});
                    view.setLibrary(ctx);ctx.discovery.start();
                    await wait(()=>ctx.library.rows.length>0&&ctx.library.rows.length<40000&&el('rows').children.length>0);
                    const first=ctx.library.rows.length;
                    const beatsAtFirst=beats;
                    el('search').value='track';el('search').dispatchEvent(new Event('input'));
                    ctx.discovery.pause();
                    await wait(()=>el('progress').textContent.includes('paused'));
                    const paused=ctx.library.rows.length;
                    await new Promise(r=>setTimeout(r,30));check(ctx.library.rows.length===paused,'paused walk does not append');
                    el('search').value='';el('search').dispatchEvent(new Event('input'));
                    await new Promise(r=>setTimeout(r,100));
                    document.querySelector('.library-track').click();el('grid').focus();
                    el('viewport').scrollTop=100;el('viewport').dispatchEvent(new Event('scroll'));
                    el('add-selected').click();check(window.w15stressAdded===1,'selection/add usable before complete');
                    ctx.discovery.resume();await wait(()=>ctx.discoveryStatus.phase==='complete');
                    await wait(()=>el('count').textContent==='40,000 of 40,000');
                    check(beats>beatsAtFirst,'heartbeat interleaves discovery');
                    check(el('rows').children.length<100,'visible DOM bounded');
                    check(document.activeElement===el('grid'),'stress grid focus retained');
                    check(el('viewport').scrollTop===100,'stress scroll retained');
                    check(reads===0,'40k discovery/select/add has zero loose contents reads');
                    summary.push({kind,rows:ctx.library.rows.length,firstUsable:first,heartbeatTicks:beats-startBeats,mapHeartbeatTicks:mappedBeats,visibleRows:el('rows').children.length,looseReads:reads,enumerationMilliseconds:performance.now()-enumerationStarted,publications});
                    ctx.discovery.dispose();
                }
                // A deterministic source-read barrier keeps a real archive job pending.
                // Input, walk pause/resume, selection and heartbeat must work before it opens.
                let release;const gate=new Promise(resolve=>release=resolve);
                const source={kind:'filelist',label:'slow',size:()=>4,readFile:async()=>{await gate;return new Uint8Array([1,2,3,4]);},async *enumerate(){yield {kind:'file',path:'slow.zip',size:4,timestamp:''};yield {kind:'file',path:'usable.mod',size:1,timestamp:''};}};
                const ctx={mode:'discovered',source,library:createDiscoveredLibrary({name:'slow'}),identity:{sha256:'e'.repeat(64),name:'slow'}};
                createDiscoveryController({context:ctx,onUpdate:(context,change)=>view.updateLibrary(context,change)});view.setLibrary(ctx);ctx.discovery.start();
                await wait(()=>el('rows').children.length===1&&ctx.discoveryStatus.archivesPending===1);
                const b=beats;el('search').value='usable';el('search').dispatchEvent(new Event('input'));await new Promise(r=>setTimeout(r,120));
                document.querySelector('.library-track').click();el('add-selected').click();check(window.w15stressAdded===1&&beats>b,'slow archive leaves input/add/heartbeat usable');
                ctx.discovery.pause();await wait(()=>el('progress').textContent.includes('paused'));ctx.discovery.resume();release();await wait(()=>ctx.discoveryStatus.phase==='complete');
                check(ctx.discoveryStatus.issueCount===1,'bad slow archive surfaces issue');ctx.discovery.dispose();
                summary.push({slowArchiveResponsiveBeforeRelease:true});
                return summary;
            } finally {clearInterval(heartbeat);}
        `);
        assert.deepEqual(page.consoleErrors, []);
        return result;
    } finally {if(page)await page.close();server.stop();}
}

export async function runW15Playlist({Page,executable,mode,startDevServer,syntheticMod}) {
    const server=await startDevServer(mode!=='plain');let page;
    try {
        page=await Page.open(executable,`http://127.0.0.1:${server.port}/`);
        await page.waitFor('W15 playlist startup',"document.documentElement.dataset.playerReady === 'true'");
        const result=await page.evaluate(`
            const {createPlaylist}=await import('./playlist.js');
            const {createPlaylistControls}=await import('./playlist-controls.js');
            const {createDiscoveryController}=await import('./library-discovery.js');
            const {createDiscoveredLibrary}=await import('./library-index.js');
            const {exportPlaylist}=await import('./playlist-transfer.js');
            const {openPlaylistStore}=await import('./playlist-store.js');
            const original=document.getElementById('playlist-panel');original.replaceWith(original.cloneNode(true));
            const check=(ok,why)=>{if(!ok)throw new Error(why);};
            const wait=async predicate=>{const deadline=performance.now()+20000;while(!await predicate()){if(performance.now()>deadline)throw new Error('playlist boundary timed out');await new Promise(r=>setTimeout(r,20));}};
            let release;const gate=new Promise(resolve=>release=resolve);
            const identity={sha256:'a'.repeat(64),name:'this folder'};
            const other={sha256:'b'.repeat(64),name:'other folder'};
            const locator={kind:'library',libraryIdentity:identity,sourcePath:'before.mod',containerMember:'',memberPath:'',rawHex:''};
            const bytes=Uint8Array.from(${JSON.stringify([...syntheticMod({title:'W15 actual metadata'})])});
            const source={kind:'filelist',label:'boundary',size:()=>bytes.length,readFile:async()=>bytes.slice(),async *enumerate(){await gate;yield {kind:'file',path:'before.mod',size:bytes.length,timestamp:''};}};
            const ctx={mode:'discovered',source,library:createDiscoveredLibrary({name:'boundary'}),identity};
            const state={library:ctx};const playlist=createPlaylist();
            const name='w15-playlist-'+crypto.randomUUID();
            const controls=await createPlaylistControls({playlist,state,loader:{},openStore:options=>openPlaylistStore({...options,name})});
            createDiscoveryController({context:ctx,onUpdate:(context,change)=>controls.libraryUpdated(context,change)});controls.libraryChanged(ctx);ctx.discovery.start();
            const entries=playlist.add([
                {locator,display:{name:'legacy',title:'old',format:'xm'}},
                {locator,display:{name:'unknown',title:'provisional',format:'it',metadataState:'unknown'}},
                {locator,display:{name:'error',title:'failed',format:'s3m',metadataState:'error'}},
                {locator:{...locator,libraryIdentity:other},display:{name:'other',title:'unchanged',format:'xm',metadataState:'unknown'}},
                {locator:{kind:'upload',blobId:'missing'},display:{name:'upload',title:'unchanged',format:'it',metadataState:'unknown'}},
            ]);
            playlist.setCurrent(entries[1].id);const ids=playlist.entries.map(e=>e.id);
            const provisional=exportPlaylist(playlist.entries).csv;
            const {parseCsv}=await import('./library-index.js');
            const cells=parseCsv(provisional);check(cells[1][5]==='xm'&&cells[2][5]===''&&cells[3][5]==='','legacy format retained; unknown/error export blank');
            for(const phase of ['running','paused','failed']) {
                ctx.discoveryStatus={...ctx.discoveryStatus,phase};controls.libraryUpdated(ctx,{statusChanged:true});
                check(document.getElementById('playlist-rebind').disabled&&!document.getElementById('playlist-rebind').hidden,'rebind disabled '+phase);
                check(controls.canPlay(entries[0])&&controls.canPlay(entries[1]),'informational status does not gate '+phase);
            }
            // Playback publication precedes enumeration. Every matching entry, including
            // legacy and error provenance, is enriched; unrelated identities/uploads stay.
            ctx.discovery.acceptPlaybackMetadata(locator,{format:'mod',title:'W15 actual metadata',channels:4,instrumentNames:'composer',sampleNames:'',songMessage:''},bytes);
            await wait(()=>playlist.entries.slice(0,3).every(e=>e.display.metadataState==='ready'));
            check(ctx.library.rows.length===0,'metadata arrived before row');
            check(playlist.entries.slice(0,3).every(e=>e.display.title==='W15 actual metadata'&&e.display.format==='mod'),'all matching display metadata enriched');
            check(playlist.entries.slice(3).every(e=>e.display.title==='unchanged'&&e.display.metadataState==='unknown'),'other identity/uploads isolated');
            check(JSON.stringify(playlist.entries.map(e=>e.id))===JSON.stringify(ids)&&playlist.current.id===entries[1].id,'IDs/order/current retained');
            release();await wait(()=>ctx.discoveryStatus.phase==='complete');
            check(ctx.library.rows[0].metadataState==='ready'&&ctx.library.rows[0].title==='W15 actual metadata','eventual row enriched');
            controls.libraryUpdated(ctx,{statusChanged:true});check(!document.getElementById('playlist-rebind').disabled,'rebind enabled complete');
            document.getElementById('playlist-rebind').click();check(playlist.entries[3].locator.libraryIdentity.sha256===identity.sha256,'explicit rebind applies matching set');
            const actual=parseCsv(exportPlaylist(playlist.entries).csv);check(actual.slice(1).every(row=>row[5]==='mod'),'ready export uses actual detected format');
            await wait(async()=>{const saved=await openPlaylistStore({name});const ok=saved.state.entries.length===5&&saved.state.entries.slice(0,4).every(e=>e.display.metadataState==='ready'&&e.display.format==='mod')&&saved.state.currentId===entries[1].id;saved.close();return ok;});
            ctx.discovery.dispose();
            return {metadataBeforeRow:true,legacyAndAllMatching:true,otherIdentityAndUploadsIsolated:true,unknownErrorExportBlank:true,readyExportActual:true,rebindAllPhases:true,idbMetadata:true};
        `);
        assert.deepEqual(page.consoleErrors,[]);return result;
    } finally {if(page)await page.close();server.stop();}
}

export async function runW15Runtime({Page,executable,mode,startDevServer,syntheticMod}) {
    const server=await startDevServer(mode!=='plain');let page;
    try {
        page=await Page.open(executable,`http://127.0.0.1:${server.port}/`);
        await page.waitFor('W15 runtime startup',"document.documentElement.dataset.playerReady === 'true'");
        const result=await page.evaluate(`
            const {createLibraryWorkerRuntime}=await import('./library-worker-runtime.js');
            const {createDiscoveryController}=await import('./library-discovery.js');
            const {createDiscoveredLibrary}=await import('./library-index.js');
            const check=(ok,why)=>{if(!ok)throw new Error(why);};
            const wait=async predicate=>{const deadline=performance.now()+20000;while(!await predicate()){if(performance.now()>deadline)throw new Error('runtime browser timeout');await new Promise(r=>setTimeout(r,20));}};
            const bytes=Uint8Array.from(${JSON.stringify([...syntheticMod({title:'Retry real worker'})])});
            const workerURL=new URL('./library-worker.js',location.href).href;
            let worker;
            const blobURL=URL.createObjectURL(new Blob([
                'self.addEventListener("message", event => { if (event.data.testCrash) { event.stopImmediatePropagation(); throw new Error("W15 deliberate idle crash"); } }); import('+JSON.stringify(workerURL)+');'
            ],{type:'text/javascript'}));
            const source={kind:'filelist',label:'runtime',size:()=>bytes.length,readFile:async()=>bytes.slice(),async *enumerate(){yield {kind:'file',path:'retry.mod',size:bytes.length,timestamp:''};}};
            const runtime=createLibraryWorkerRuntime({source,workerFactory:()=>worker=new Worker(blobURL,{type:'module'})});
            try {
                await runtime.enqueue({generation:1,kind:'inspect-group',sourcePath:'retry.mod',members:[{token:'one',locator:{sourcePath:'retry.mod'},version:0}],purpose:'scan'}).result;
                runtime.pauseCatalog();
                const crash=new Promise(resolve=>worker.addEventListener('error',resolve,{once:true}));worker.postMessage({testCrash:true});await crash;
                const first=runtime.enqueue({generation:1,kind:'catalog',sourcePath:'paused.zip'});
                const error=await first.result.catch(error=>error);check(error.code==='worker-failed','idle crash reported to paused next promise');
                let settled=false;const next=runtime.enqueue({generation:1,kind:'inspect-group',sourcePath:'retry.mod',members:[{token:'two',locator:{sourcePath:'retry.mod'},version:0}],purpose:'scan'});next.result.then(()=>settled=true,()=>settled=true);
                await new Promise(r=>setTimeout(r,60));check(!settled,'remaining jobs suspended after idle crash');runtime.dispose();await next.result.catch(()=>{});
            } finally {runtime.dispose();URL.revokeObjectURL(blobURL);}
            // Genuine startup failure in a worker, followed by the controller's Retry
            // action recreating a healthy real WASM worker and finishing inspection.
            const startupURL=URL.createObjectURL(new Blob(['throw new Error("W15 deliberate worker startup failure")'],{type:'text/javascript'}));
            let first=true;
            const ctx={mode:'discovered',source,library:createDiscoveredLibrary({name:'retry'}),identity:{sha256:'f'.repeat(64),name:'retry'}};
            const controller=createDiscoveryController({context:ctx,runtimeFactory:options=>createLibraryWorkerRuntime({...options,workerFactory:()=>{if(first){first=false;return new Worker(startupURL,{type:'module'});}return new Worker(workerURL,{type:'module'});}})});
            controller.start();await wait(()=>ctx.library.rows.length===1);controller.scanMetadata();
            await wait(()=>!!ctx.discoveryStatus.runtimeError);
            check(ctx.discoveryStatus.phase==='complete'&&ctx.discoveryStatus.metadataPhase==='suspended','complete discovery survives worker failure; scan suspended');
            controller.retry();await wait(()=>ctx.library.rows[0].metadataState==='ready'&&ctx.discoveryStatus.metadataPhase==='complete');
            check(ctx.library.rows[0].title==='Retry real worker'&&!ctx.discoveryStatus.runtimeError,'Retry real metadata succeeds');
            controller.dispose();URL.revokeObjectURL(startupURL);
            let readStarted=false,releaseRead;
            const readGate=new Promise(resolve=>releaseRead=resolve);
            const gatedSource={...source,readFile:async()=>{readStarted=true;await readGate;return bytes.slice();}};
            const cancelledContext={mode:'discovered',source:gatedSource,library:createDiscoveredLibrary({name:'cancel'}),identity:ctx.identity};
            const cancelController=createDiscoveryController({context:cancelledContext});cancelController.start();
            await wait(()=>cancelledContext.library.rows.length===1);cancelController.scanMetadata();await wait(()=>readStarted);
            cancelController.cancelMetadata();releaseRead();await new Promise(r=>setTimeout(r,80));
            check(cancelledContext.discoveryStatus.metadataPhase==='cancelled'&&cancelledContext.library.rows[0].metadataState==='unknown','cancel metadata discards late read/result');
            cancelController.scanMetadata();await wait(()=>cancelledContext.library.rows[0].metadataState==='ready');cancelController.dispose();
            let oldRead=false,releaseOld,oldUpdates=0;
            const oldGate=new Promise(resolve=>releaseOld=resolve);
            const oldContext={mode:'discovered',source:{...source,readFile:async()=>{oldRead=true;await oldGate;return bytes.slice();}},library:createDiscoveredLibrary({name:'old'}),identity:ctx.identity};
            const oldController=createDiscoveryController({context:oldContext,onUpdate:()=>oldUpdates++});oldController.start();
            await wait(()=>oldContext.library.rows.length===1);oldController.scanMetadata();await wait(()=>oldRead);oldController.dispose();const updatesAtDispose=oldUpdates;
            releaseOld();await new Promise(r=>setTimeout(r,80));check(oldUpdates===updatesAtDispose&&oldContext.library.rows[0].metadataState==='unknown','source replacement discards old read/results');
            let releaseArrival;const arrivalGate=new Promise(resolve=>releaseArrival=resolve);
            const arrivalContext={mode:'discovered',source:{...source,async *enumerate(){yield {kind:'file',path:'known.mod',size:bytes.length,timestamp:''};await arrivalGate;yield {kind:'file',path:'later.mod',size:bytes.length,timestamp:''};}},library:createDiscoveredLibrary({name:'arrivals'}),identity:ctx.identity};
            const arrivalController=createDiscoveryController({context:arrivalContext});arrivalController.start();await wait(()=>arrivalContext.library.rows.length===1);arrivalController.pause();arrivalController.scanMetadata();
            await wait(()=>arrivalContext.library.rows[0].metadataState==='ready'&&arrivalContext.discoveryStatus.metadataPhase==='waiting');
            check(arrivalContext.discoveryStatus.phase==='paused','known-row metadata continues during paused discovery');
            releaseArrival();arrivalController.resume();await wait(()=>arrivalContext.library.rows.length===2&&arrivalContext.library.rows.every(row=>row.metadataState==='ready')&&arrivalContext.discoveryStatus.metadataPhase==='complete');arrivalController.dispose();
            return {idleCrashReportedOnce:true,pausedNextPromise:true,remainingSuspended:true,startupFailureSuspends:true,retryRealWorker:true,cancelMetadataLateRead:true,replacementIgnoresLateRead:true,scanIncludesArrivals:true,pausedDiscoveryMetadataContinues:true};
        `);
        assert.deepEqual(page.consoleErrors,[]);return result;
    }finally{if(page)await page.close();server.stop();}
}
