import assert from 'node:assert/strict';
import {createServer} from 'node:http';

const pause = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));
async function open({Page, executable, mode, startDevServer}) {
    const server = await startDevServer(mode !== 'plain');
    const page = await Page.open(executable, `http://127.0.0.1:${server.port}/`);
    await page.waitFor('remediation startup', "document.documentElement.dataset.playerReady === 'true'");
    if(mode==='fallback')await page.evaluate("document.getElementById('force-fallback').checked=true;");
    return {page, server};
}

export async function runBackground(options) {
    const {page, server} = await open(options);
    const smoke=process.env.STARPLAYER_BACKGROUND_SMOKE==='1';
    try {
        const environment = {version:page.version, flags:page.browserArguments, concurrentLoad:'coordinator Rust/Node/WASM verification may run concurrently; throughput is configuration-specific'};
        assert.ok(!environment.flags.some(flag => /disable-background|disable-renderer-backgrounding|disable-occlusion/.test(flag)));
        await page.evaluate(`
            const {createDirectorySource}=await import('./library-source.js');
            const {createDiscoveredLibrary}=await import('./library-index.js');
            const {createDiscoveryController}=await import('./library-discovery.js');
            const {cooperativeYieldMethod}=await import('./cooperative-yield.js');
            const method=cooperativeYieldMethod();if(typeof globalThis.scheduler?.postTask==='function'&&typeof globalThis.scheduler?.yield==='function'&&method!=='scheduler.postTask(background) + scheduler.yield')throw new Error('Packaged cooperative helper is stale');
            window.backgroundEvidence={method,transitions:[],runs:[],timerTicks:0,timerTimes:[]};
            document.addEventListener('visibilitychange',()=>backgroundEvidence.transitions.push({time:performance.now(),state:document.visibilityState}));
            let stop=false;
            const timer=()=>{if(stop)return;backgroundEvidence.timerTicks++;if(backgroundEvidence.timerTimes.length<500)backgroundEvidence.timerTimes.push(performance.now());setTimeout(timer,100);};timer();
            window.stopBackgroundTimer=()=>{stop=true;};
            window.beginBackgroundWalk=async (label, gated=false)=>{
                const run={label,start:performance.now(),visible:document.visibilityState,records:0,preparationRecords:0,notifications:0};backgroundEvidence.runs.push(run);
                let preparationComplete=false;
                const barrier=gated?new Promise(resolve=>window.releaseFrozenWalk=resolve):null;
                const root={kind:'directory',name:'bounded-background',async *entries(){for(let index=0;index<65536;index++){if(index===128&&barrier&&preparationComplete)await barrier;if(!preparationComplete)run.preparationRecords++;yield ['track'+index+'.mod',{kind:'file',name:'track'+index+'.mod'}];}}};
                const source=await createDirectorySource(root);preparationComplete=true;
                const context={mode:'discovered',source,library:createDiscoveredLibrary({name:label}),identity:{sha256:'a'.repeat(64),name:label}};
                const controller=createDiscoveryController({context,onUpdate:(context,change)=>{run.notifications++;run.publishedRows=context.library.rows.length;run.published=performance.now();if(window.backgroundViewContext===context)window.backgroundView.updateLibrary(context,change);}});
                run.prepared=performance.now();run.context=context;run.controller=controller;controller.start();
                const done=()=>{run.records=context.library.rows.length;if(context.discoveryStatus.phase==='complete'){run.end=performance.now();return;}setTimeout(done,1000);};done();
            };
            window.backgroundSummary=()=>({...backgroundEvidence,runs:backgroundEvidence.runs.map(({context,controller,...run})=>({...run,records:context?.library.rows.length??0,phase:context?.discoveryStatus.phase??'preparing'}))});
            return {visibility:document.visibilityState,audio:[...document.querySelectorAll('audio')].every(audio=>audio.paused),transport:document.getElementById('transport-chip').textContent};
        `).then(state=>{assert.equal(state.audio,true);assert.notEqual(state.transport,'playing');});
        const other = await page.send('Target.createTarget',{url:'about:blank'});
        await page.send('Target.activateTarget',{targetId:other.targetId});
        await page.waitFor('genuine hidden tab', "document.visibilityState === 'hidden'");
        await page.evaluate("beginBackgroundWalk('early-hidden');return true;");
        const start = Date.now(), samples=[];
        while(Date.now()-start<(smoke?1000:365000)) {
            await pause(smoke?1000:30000);
            const elapsed=Date.now()-start;
            if(elapsed>=(smoke?0:300000)&&!samples.some(sample=>sample.lateStarted)) {
                await page.evaluate("beginBackgroundWalk('after-five-minutes-hidden');return true;");
                samples.push({elapsed,lateStarted:true});
            }
            const sample=await page.evaluate('return backgroundSummary();');
            assert.equal(await page.evaluate('return document.visibilityState;'),'hidden');
            samples.push({elapsed, ...sample});
            console.log('background-sample '+JSON.stringify({elapsed,method:sample.method,timerTicks:sample.timerTicks,runs:sample.runs}));
        }
        const before=await page.evaluate('return backgroundSummary();');
        assert.equal(before.runs.length,2);
        for(const run of before.runs){assert.equal(run.visible,'hidden');assert.ok(run.records>=0&&run.records<=65536);}
        // Create a real pending notification immediately before returning to the tab.
        await page.evaluate(`
            const run=backgroundEvidence.runs.findLast(run=>run.context);if(!run)throw new Error('No prepared context for return probe');window.returnRun=run;
            const panel=document.getElementById('library-panel');panel.replaceWith(panel.cloneNode(true));document.getElementById('library-heading').replaceChildren();document.getElementById('library-match-popover')?.remove();
            const {createLibraryView}=await import('./library-view.js');window.backgroundViewContext=run.context;window.backgroundView=createLibraryView({playlist:{add(){}},play(){}});backgroundView.setLibrary(run.context);
            window.returnPublished=false;
            run.controller.acceptPlaybackMetadata({sourcePath:'track0.mod'},{title:'return delta',format:'mod',channels:4},new Uint8Array([1,2,3]));
            window.returnStarted=performance.now();
        `);
        const hiddenMilliseconds=Date.now()-start;
        const publicationBeforeReturn=await page.evaluate("return returnRun.published >= returnStarted;");
        const returning=Date.now();
        await page.send('Target.activateTarget',{targetId:page.targetId});
        await page.waitFor('visible return', "document.visibilityState === 'visible'");
        await page.waitFor('pending delta publication after return', "returnRun.context.library.rows[0].metadataState === 'ready' && returnRun.published >= returnStarted && document.getElementById('library-rows').textContent.includes('return delta')");
        const visibilityReturnMilliseconds=Date.now()-returning;
        await page.waitFor('hidden iterators safely finish after return', "backgroundEvidence.runs.every(run=>run.context.discoveryStatus.phase === 'complete')",120000);
        await page.evaluate("beginBackgroundWalk('same-iterator-freeze-thaw',true);return true;");
        await page.waitFor('active iterator at freeze barrier',"backgroundEvidence.runs.at(-1).context.library.rows.length === 128");
        await page.send('Page.setWebLifecycleState',{state:'frozen'});
        await pause(2000);
        await page.send('Page.setWebLifecycleState',{state:'active'});
        await page.evaluate("releaseFrozenWalk();return true;");
        await page.waitFor('safe thaw enumeration', "backgroundEvidence.runs.at(-1).context.discoveryStatus.phase === 'complete'",60000);
        const final=await page.evaluate("stopBackgroundTimer();const report=backgroundSummary();for(const run of backgroundEvidence.runs)run.controller.dispose();return report;");
        assert.equal(final.runs.at(-1).records,65536);
        for(const run of final.runs)assert.equal(run.records,65536);
        const returnProbeLabel=await page.evaluate('return returnRun.label;');
        assert.deepEqual(page.consoleErrors,[]);
        return {smoke,environment,hiddenMilliseconds, samples, final,returnProbeLabel,publicationBeforeReturn,notificationAfterReturnMilliseconds:publicationBeforeReturn?0:visibilityReturnMilliseconds,visibilityReturnDOMMilliseconds:visibilityReturnMilliseconds,freezeThawRecovery:true,limitation:'No frozen progress assertion; finite 65,536-row synthetic walks; no real-device throughput guarantee. The last-digest completion flush in this run is measured separately from the dedicated nonterminal debounce visibility case.'};
    } finally {await page.close();server.stop();}
}

export async function runRemediation(options){
    const {page,server}=await open(options);
    const fixtureServer=createServer((request,response)=>{
        const query=new URL(request.url,'http://localhost').searchParams,total=Number(query.get('total')),nested=query.get('nested')==='1',large=query.get('large')==='1';
        if(![3,64,130].includes(total)){response.writeHead(400);response.end();return;}
        const mod=options.syntheticMod({title:query.get('version')==='1'?'next snapshot':'matrix snapshot'}),members=Array.from({length:total},(_,index)=>({name:String(index).padStart(3,'0')+'.mod',bytes:mod}));
        const entries=nested?[{name:'inner.zip',bytes:options.buildZip(members)}]:members;
        if(large)entries.push({name:'padding.bin',bytes:Buffer.alloc(33*1024*1024),stored:true});
        const bytes=options.buildZip(entries);response.writeHead(200,{'Access-Control-Allow-Origin':'*','Cross-Origin-Resource-Policy':'cross-origin','Content-Length':bytes.length});response.end(bytes);
    });
    await new Promise(resolve=>fixtureServer.listen(0,'127.0.0.1',resolve));
    try{
        const mod=options.syntheticMod({title:'remediation group'}),zip=options.buildZip(['A','B','C'].map(name=>({name:name+'.mod',bytes:mod})));
        await page.evaluate(`const transfer=new DataTransfer();transfer.items.add(new File([Uint8Array.from(${JSON.stringify([...mod])})],'active-regression.mod'));const picker=document.getElementById('file-picker');picker.files=transfer.files;picker.dispatchEvent(new Event('change'));`);
        await page.waitFor('actual audio before gated regression work',"document.getElementById('title').textContent==='remediation group'&&document.getElementById('transport-chip').textContent==='playing'");
        const mixed=options.buildZip([{name:'good.mod',bytes:mod},{name:'broken.mod',bytes:Buffer.from([1,2,3])},{name:'encrypted.mod',bytes:mod,encrypted:true}]);
        const result=await page.evaluate(`return await (${regressionScenario.toString()})(${JSON.stringify([...mod])},${JSON.stringify([...zip])},${JSON.stringify([...mixed])});`);
        const matrix=await page.evaluate(`return await (${archiveMatrixScenario.toString()})(${JSON.stringify('http://127.0.0.1:'+fixtureServer.address().port+'/matrix')});`);
        const selection=await page.evaluate(`return await (${selectionScenario.toString()})(${JSON.stringify([...mod])});`);
        assert.equal(await page.evaluate("return document.getElementById('title').textContent==='remediation group'&&document.getElementById('transport-chip').textContent==='playing';"),true,'real active playback survives regression and pending source work');
        const commandTransport=await page.evaluate("return document.getElementById('command-transport').textContent;");assert.match(commandTransport,options.mode==='sab'?/SharedArrayBuffer/:/postMessage/);
        assert.deepEqual(page.consoleErrors,[]);return {version:page.version,commandTransport,...result,matrix,selection,activePlaybackPreserved:true};
    }finally{await page.close();server.stop();await new Promise(resolve=>{fixtureServer.close(resolve);fixtureServer.closeAllConnections();});}
}

export async function runPerformance(options){const {page,server}=await open(options);try{const result=await page.evaluate(`return await (${performanceScenario.toString()})();`);assert.deepEqual(page.consoleErrors,[]);return {version:page.version,flags:page.browserArguments,...result};}finally{await page.close();server.stop();}}
export async function runDigestMeasurements(options){
    const {page,server}=await open(options);const mod=options.syntheticMod({title:'native measurement'});
    const archiveServer=createServer((request,response)=>{const padded=Buffer.alloc(64*1024*1024);mod.copy(padded);const bytes=options.buildZip([{name:'large.mod',bytes:padded}]);response.writeHead(200,{'Access-Control-Allow-Origin':'*','Cross-Origin-Resource-Policy':'cross-origin','Content-Length':bytes.length});response.end(bytes);});
    await new Promise(resolve=>archiveServer.listen(0,'127.0.0.1',resolve));
    try{await page.evaluate(`if(${JSON.stringify(options.mode)}==='fallback')document.getElementById('force-fallback').checked=true;`);const report=await page.evaluate(`return await (${digestScenario.toString()})(${JSON.stringify([...mod])},${JSON.stringify('http://127.0.0.1:'+archiveServer.address().port+'/large.zip')});`);assert.deepEqual(page.consoleErrors,[]);return {version:page.version,flags:page.browserArguments,...report};}finally{await page.close();server.stop();await new Promise(resolve=>{archiveServer.close(resolve);archiveServer.closeAllConnections();});}
}

async function performanceScenario() {
    const model=await import('./library-index.js');
    const {createLibraryView}=await import('./library-view.js');
    const panel=document.getElementById('library-panel');panel.replaceWith(panel.cloneNode(true));
    document.getElementById('library-heading').replaceChildren();document.getElementById('library-match-popover')?.remove();
    const el=name=>document.getElementById('library-'+name);
    const check=(ok,why)=>{if(!ok)throw new Error(why);};
    const settle=()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
    const counts={},snapshot=()=>({...counts}),delta=previous=>Object.fromEntries(Object.keys(counts).map(key=>[key,counts[key]-(previous[key]||0)]));
    const longTasks=[];const observer=new PerformanceObserver(list=>longTasks.push(...list.getEntries().map(entry=>({start:entry.startTime,duration:entry.duration}))));observer.observe({type:'longtask',buffered:true});
    const record=(index,prefix='group')=>({sourcePath:prefix+String(index%100).padStart(3,'0')+'/sub/track'+String(index).padStart(6,'0')+'.mod',format:'mod',size:null,timestamp:'',playable:true});
    const library=model.createDiscoveredLibrary({name:'100k synthetic'});
    const start=performance.now();model.appendDiscoveredRows(library,Array.from({length:100000},(_,index)=>record(index)));
    let issueReads=0;const issues=Array.from({length:10000},(_,id)=>({id,path:'synthetic/'+id,reason:'bounded issue '+id}));
    const status={phase:'running',filesExamined:100000,tracksFound:100000,archivesPending:0,metadataPhase:'idle',runtimeError:'',metadataCounts:{ready:0,unknown:100000,error:0,excluded:0,hashesPending:0,refreshPending:0,scanActionable:100000},metadataIncomplete:true,issuesRevision:1,issueCount:10000,get issues(){issueReads++;return issues;}};
    const context={mode:'discovered',library,identity:{sha256:'b'.repeat(64),name:'perf'},discoveryStatus:status,discovery:{}};
    let added=0;const view=createLibraryView({playlist:{add:items=>added+=items.length},play:()=>{},onOperation:(name,amount)=>counts[name]=(counts[name]||0)+amount});view.setLibrary(context);await settle();
    const setupMilliseconds=performance.now()-start;
    el('browse').click();await settle();
    check(!el('folders').hidden,'tree open');
    const treeBefore=el('tree').innerHTML;el('browse').click();await settle();check(el('folders').hidden,'tree hide');el('browse').click();await settle();check(el('tree').innerHTML===treeBefore,'tree reopen keeps expansion');
    el('issues').open=true;el('issues').dispatchEvent(new Event('toggle'));await settle();
    check(el('issues-list').children.length===100,'10k issues only 100 DOM entries');el('issues-next').click();check(el('issues-list').firstChild.textContent.startsWith('synthetic/100:'),'issue next page');el('issues-previous').click();
    const issueReadsBefore=issueReads, beforeStatus=snapshot();
    for(let index=0;index<50;index++)view.updateLibrary(context,{statusChanged:true,rowChanges:[],issuesRevision:1});await settle();
    const statusOnly=delta(beforeStatus);check(issueReads===issueReadsBefore,'status ticks do not snapshot issue getter');check(!statusOnly.rowEvaluations&&!statusOnly.fullSorts&&!statusOnly.treeBuilds&&!statusOnly.treeStatusUpdates&&!statusOnly.issueMaterializations,'status work is scalar');
    el('search').value='track';el('search').dispatchEvent(new Event('input'));await new Promise(resolve=>setTimeout(resolve,150));await settle();
    el('viewport').scrollTop=32009;el('viewport').dispatchEvent(new Event('scroll'));await settle();
    const anchor=()=>{const index=Math.floor(el('viewport').scrollTop/32);const row=[...el('rows').children].find(row=>Number(row.getAttribute('aria-rowindex'))===index+2);return {id:Number(row.dataset.rowId),offset:el('viewport').scrollTop-index*32};};
    const secondVisibleIndex=Math.floor(el('viewport').scrollTop/32)+3;[...el('rows').children].find(row=>Number(row.getAttribute('aria-rowindex'))===secondVisibleIndex).click();el('grid').focus();el('grid').dispatchEvent(new KeyboardEvent('keydown',{key:'ArrowDown',bubbles:true}));
    const focused=document.activeElement,initialAnchor=anchor();check(initialAnchor.offset===9,'nonzero pixel anchor fixture');
    const updates=[],inputs=[],batches=[];
    for(let batch=0;batch<20;batch++) {
        const before=snapshot(), beforeAnchor=anchor();
        const patches=Array.from({length:100},(_,index)=>({locator:library.rows[(batch*100+index)%100000],patch:{title:'metadata '+batch+' '+index,channels:4,metadataState:'ready'}}));
        const time=performance.now();model.updateDiscoveredRows(library,patches);view.updateLibrary(context,{changedRowIds:library.lastRowChanges.map(change=>change.id),rowChanges:library.lastRowChanges,metadataUpdates:patches});await settle();updates.push(performance.now()-time);
        const operations=delta(before);check(!operations.fullSorts&&!operations.treeBuilds&&!operations.treeContributions,'metadata batch never rebuilds tree/full sort');check((operations.rowEvaluations||0)<=100,'metadata reevaluates changed rows only');check(!operations.anchorRestores,'display metadata does not restore anchor');check(JSON.stringify(anchor())===JSON.stringify(beforeAnchor),'metadata preserves anchor offset');check(document.activeElement===focused,'keyboard focus retained');
        batches.push({kind:'metadata',size:100,operations});
        const inputStart=performance.now();const activeBefore=el('grid').getAttribute('aria-activedescendant');el('grid').dispatchEvent(new KeyboardEvent('keydown',{key:batch%2?'ArrowUp':'ArrowDown',bubbles:true}));await settle();check(el('grid').getAttribute('aria-activedescendant')!==activeBefore,'handled keyboard input acknowledged');inputs.push(performance.now()-inputStart);
    }
    const beforeAdd=snapshot(),addAnchor=anchor();
    model.appendDiscoveredRows(library,Array.from({length:100},(_,index)=>record(100000+index,'append')));const additions=library.lastRowChanges;
    const addStart=performance.now();view.updateLibrary(context,{changedRowIds:additions.map(change=>change.id),rowChanges:additions});await settle();const additionMilliseconds=performance.now()-addStart;
    const additionOperations=delta(beforeAdd);check(!additionOperations.fullSorts&&!additionOperations.treeBuilds,'additions incremental');check(additionOperations.rowEvaluations===100&&additionOperations.treeContributions===100&&additionOperations.batchSorts===1,'additions process one changed batch');check(additionOperations.mergeRows===100000,'one merge across existing order');check(JSON.stringify(anchor())===JSON.stringify(addAnchor),'append keeps anchor');
    // A structural move prunes the focused branch and repairs focus to a surviving ancestor.
    model.appendDiscoveredRows(library,[{sourcePath:'prune/leaf/only.mod',format:'mod',playable:true}]);view.updateLibrary(context,{rowChanges:library.lastRowChanges});await settle();
    const branch=document.querySelector('[data-scope="prune"]');branch.querySelector('[data-tree-toggle]').click();await settle();document.querySelector('[data-scope="prune/leaf"]').focus();
    const moved=library.rows.at(-1);moved.folder='moved/leaf';moved.sourcePath='moved/leaf/only.mod';
    view.updateLibrary(context,{rowChanges:[{id:moved.id,added:false,fields:['folder','sourcePath']}]});await settle();check(!document.querySelector('[data-scope="prune"]'),'empty path pruned');check(document.activeElement?.getAttribute('role')==='treeitem','tree focus repaired');
    const beforeLabels=snapshot();status.phase='complete';view.updateLibrary(context,{statusChanged:true,rowChanges:[]});await settle();const phaseLabels=delta(beforeLabels);check(phaseLabels.treeStatusUpdates>0,'phase transition updates tree labels');check(!document.querySelector('[data-scope=""] [data-tree-add]').title.includes('discovered'),'complete label');
    const sortStart=performance.now();document.querySelector('#library-heading [data-column="title"] button').click();const sortEnd=performance.now(),synchronousSortMilliseconds=sortEnd-sortStart;await settle();await new Promise(resolve=>setTimeout(resolve,100));
    const sortLongestTask=Math.max(0,...longTasks.filter(task=>task.start<sortEnd&&task.start+task.duration>sortStart).map(task=>task.duration));
    const sortedMetadataBatches=[];
    for(let batch=0;batch<5;batch++) {
        const before=snapshot();const patches=Array.from({length:100},(_,index)=>({locator:library.rows[batch*100+index],patch:{title:'sorted metadata '+batch+' '+index}}));model.updateDiscoveredRows(library,patches);view.updateLibrary(context,{rowChanges:library.lastRowChanges,changedRowIds:library.lastRowChanges.map(change=>change.id)});await settle();
        const operations=delta(before);check(!operations.fullSorts&&!operations.treeBuilds&&!operations.treeContributions&&operations.rowEvaluations===100&&operations.batchSorts===1&&operations.mergeRows===100100,'sort-key updates reevaluate 100 changed rows and merge the existing order once');check(operations.comparisons<library.rows.length+2000,'changed-key batch avoids sorting unchanged rows');sortedMetadataBatches.push({size:100,operations});
    }
    check(el('rows').children.length<100&&el('issues-list').children.length===100,'visible DOM bounded');
    observer.disconnect();
    const distribution=values=>{const sorted=values.slice().sort((a,b)=>a-b);return {count:values.length,p50:sorted[Math.floor(sorted.length*.5)],p95:sorted[Math.min(sorted.length-1,Math.floor(sorted.length*.95))],max:Math.max(...values)};};
    return {rows:library.rows.length,issues:issues.length,visibleRowDOM:el('rows').children.length,issueDOMCount:el('issues-list').children.length,treeNodesDOM:el('tree').querySelectorAll('[role=treeitem]').length,setupMilliseconds,statusOnly,issueReadsBefore,issueReadsAfter:issueReads,metadataBatches:batches,sortedMetadataBatches,additionOperations,additionMilliseconds,updateLatency:distribution(updates),inputLatency:distribution(inputs),synchronousSortMilliseconds,longestTask:Math.max(0,...longTasks.map(task=>task.duration)),sortLongestTask,longTasks,phaseLabels,initialAnchor,syntheticBaseline:{metadataBatches:20,oldFullEvaluations:20*100000,oldFullSorts:20,oldTreeBuilds:20,oldIssueDOM:10000},limitation:'Update/input latency includes dispatch through two animation frames; synchronous sort time is measured separately. Timing describes this headless machine only. Baseline operation counts are derived from the superseded full rebuild strategy, not timed baseline execution.'};
}

async function regressionScenario(modArray, zipArray, mixedArray) {
    const {createDiscoveryController}=await import('./library-discovery.js');
    const {createLibraryWorkerRuntime}=await import('./library-worker-runtime.js');
    const model=await import('./library-index.js');
    const bytes=Uint8Array.from(modArray),zip=Uint8Array.from(zipArray);
    const seedTitle=new TextDecoder().decode(bytes.subarray(0,20)).replace(/\0.*$/,'');
    const replayBytes=title=>{const output=bytes.slice();output.fill(0,0,20);output.set(new TextEncoder().encode(title));return output;};
    const check=(ok,why)=>{if(!ok)throw new Error(why);};
    const wait=async predicate=>{const deadline=performance.now()+30000;while(!predicate()){if(performance.now()>deadline)throw new Error('regression timeout');await new Promise(resolve=>setTimeout(resolve,5));}};
    const hex=buffer=>[...new Uint8Array(buffer)].map(value=>value.toString(16).padStart(2,'0')).join('');
    const metadata=title=>({format:'mod',title,channels:4,instrumentNames:'',sampleNames:'',songMessage:''});
    const locate=path=>({sourcePath:path,containerMember:'',memberPath:'',rawHex:''});
    const context=(source,name='regression')=>({mode:'discovered',source,library:model.createDiscoveredLibrary({name}),identity:{sha256:'a'.repeat(64),name}});
    const source={kind:'filelist',label:'regression',size:()=>bytes.length,readFile:async()=>bytes.slice(),async *enumerate(){for(const path of ['A.mod','B.mod','C.mod'])yield {kind:'file',path,size:bytes.length,timestamp:''};}};
    const gates=[],metrics=[],snapshots=[];
    let mode='delay';
    const digest=input=>{
        if(mode==='sync')throw new Error('synchronous digest failure');
        if(mode==='reject')return Promise.reject(new Error('asynchronous digest failure'));
        if(mode==='invalid')return Promise.resolve(new Uint8Array(32));
        const native=crypto.subtle.digest('SHA-256',input);
        if(mode==='native')return native;
        let release,reject;const gate=new Promise((yes,no)=>{release=yes;reject=no;});gates.push({release,reject});return native.then(result=>gate.then(()=>result));
    };
    const ctx=context(source);
    const controller=createDiscoveryController({context:ctx,digest,onDigestMetric:metric=>metrics.push(metric),onUpdate:()=>snapshots.push(ctx.library.rows.map(row=>({title:row.title,sha:row.sha256,text:ctx.library.commentsFor(row.id).instrumentNames}))) });controller.start();await wait(()=>ctx.discoveryStatus.phase==='complete');
    const seedSha=hex(await crypto.subtle.digest('SHA-256',bytes));
    controller.attachComments(new Map([[seedSha,{instrumentNames:'seed sidecar',sampleNames:'',songMessage:''}]]));
    for(const path of ['A.mod','B.mod','C.mod'])controller.acceptPlaybackMetadata(locate(path),metadata(seedTitle),bytes);
    check(ctx.library.rows.every(row=>row.metadataState==='unknown'&&!row.sha256),'first play remains unknown until success');
    gates.splice(0).forEach(gate=>gate.release());await wait(()=>ctx.discoveryStatus.metadataCounts.hashesPending===0);
    check(ctx.library.rows.every(row=>row.sha256===seedSha&&ctx.library.commentsFor(row.id).instrumentNames==='seed sidecar'),'seed coherent metadata SHA sidecar');
    const changed=replayBytes('A new'),bytesB=replayBytes('B new'),bytesC=replayBytes('C new');const changedSha=hex(await crypto.subtle.digest('SHA-256',changed)),shaB=hex(await crypto.subtle.digest('SHA-256',bytesB)),shaC=hex(await crypto.subtle.digest('SHA-256',bytesC));
    controller.attachComments(new Map([[seedSha,{instrumentNames:'seed sidecar',sampleNames:'',songMessage:''}],[changedSha,{instrumentNames:'new sidecar',sampleNames:'',songMessage:''}],[shaB,{instrumentNames:'B sidecar',sampleNames:'',songMessage:''}],[shaC,{instrumentNames:'C sidecar',sampleNames:'',songMessage:''}]]));
    const replayStart=snapshots.length;
    const mutable={...metadata('A new')};controller.acceptPlaybackMetadata(locate('A.mod'),mutable,changed);mutable.title='mutated caller';changed[0]=90;
    controller.acceptPlaybackMetadata(locate('B.mod'),metadata('B new'),bytesB);
    controller.acceptPlaybackMetadata(locate('C.mod'),metadata('C new'),bytesC);
    check(ctx.library.rows.every(row=>row.title===seedTitle&&row.sha256===seedSha),'successful replay retains old coherent tuples');
    check(ctx.discoveryStatus.metadataCounts.refreshPending===3&&ctx.discoveryStatus.metadataCounts.hashesPending===3,'current pending refresh counts');
    controller.cancelMetadata();controller.retry();
    check(ctx.discoveryStatus.metadataCounts.hashesPending===3,'cancel/retry preserve page digests');
    gates[1].release();gates[2].release();await wait(()=>ctx.library.rows[1].title==='B new'&&ctx.library.rows[2].title==='C new');
    check(ctx.library.rows[1].sha256===shaB&&ctx.library.rows[2].sha256===shaC&&ctx.library.commentsFor(1).instrumentNames==='B sidecar'&&ctx.library.commentsFor(2).instrumentNames==='C sidecar','B/C latest bytes and sidecars agree');
    check(ctx.library.rows[0].title===seedTitle&&ctx.library.rows[0].sha256===seedSha,'unrelated digests complete independently');gates[0].release();gates.length=0;await wait(()=>ctx.discoveryStatus.metadataCounts.hashesPending===0);
    check(ctx.library.rows[0].title==='A new'&&ctx.library.rows[0].sha256===changedSha&&ctx.library.commentsFor(0).instrumentNames==='new sidecar','native snapshot and metadata scalar snapshot publish atomic');
    for(const snapshot of snapshots.slice(replayStart))for(const row of snapshot)check(row.sha.length===64,'no empty SHA interval for seeded successful A/B/C replays');
    for(const snapshot of snapshots)for(const row of snapshot){if(row.title==='A new')check(row.sha===changedSha&&row.text==='new sidecar','no incoherent atomic A update');if(row.title==='B new')check(row.sha===shaB&&row.text==='B sidecar','no incoherent atomic B update');if(row.title==='C new')check(row.sha===shaC&&row.text==='C sidecar','no incoherent atomic C update');}
    controller.acceptPlaybackMetadata(locate('A.mod'),metadata('obsolete'),bytes);controller.acceptPlaybackMetadata(locate('A.mod'),metadata('latest'),bytes);
    check(ctx.discoveryStatus.metadataCounts.hashesPending===2&&ctx.discoveryStatus.metadataCounts.refreshPending===1,'superseded promises physically counted');gates[1].release();await wait(()=>ctx.library.rows[0].title==='latest');check(ctx.discoveryStatus.metadataCounts.hashesPending===1&&!ctx.discoveryStatus.metadataIncomplete,'stale-only promise does not make verified latest incomplete');gates[0].reject(new Error('stale failure'));gates.length=0;await wait(()=>!ctx.discoveryStatus.metadataCounts.hashesPending);check(ctx.discoveryStatus.issueCount===0,'stale failure ignored');
    for(const failureMode of ['sync','reject','invalid']) {mode=failureMode;controller.acceptPlaybackMetadata(locate('A.mod'),metadata('failed '+mode),bytes);await wait(()=>ctx.library.rows[0].title==='failed '+mode&&!ctx.discoveryStatus.metadataCounts.hashesPending);check(ctx.library.rows[0].sha256===''&&ctx.discoveryStatus.issueCount>0,'digest failure clears SHA and adds joined issue '+mode);}
    mode='native';controller.acceptPlaybackMetadata(locate('A.mod'),metadata('recovered'),bytes);await wait(()=>ctx.library.rows[0].sha256===seedSha);
    const beforeDisposeIssues=ctx.discoveryStatus.issueCount;mode='delay';controller.acceptPlaybackMetadata(locate('A.mod'),metadata('disposed'),bytes);controller.dispose();gates.splice(0).forEach(gate=>gate.reject(new Error('disposed failure')));await wait(()=>metrics.at(-1).event==='settle'&&metrics.at(-1).pending===0);check(ctx.library.rows[0].title==='recovered'&&ctx.discoveryStatus.issueCount===beforeDisposeIssues,'disposed result only clears scalar accounting');
    // Native subview digest snapshots exactly its byte range at invocation.
    const subviewContext=context({...source,async *enumerate(){yield {kind:'file',path:'view.mod',size:3,timestamp:''};}},'native subview');const subviewController=createDiscoveryController({context:subviewContext});subviewController.start();await wait(()=>subviewContext.library.rows.length===1);
    const backing=new Uint8Array([9,1,2,3,8]),view=backing.subarray(1,4);subviewController.acceptPlaybackMetadata(locate('view.mod'),metadata('native subview'),view);backing.fill(0);await wait(()=>subviewContext.library.rows[0].metadataState==='ready');check(subviewContext.library.rows[0].sha256===hex(await crypto.subtle.digest('SHA-256',new Uint8Array([1,2,3]))),'controller native subview snapshot correctness');check(backing.byteLength===5&&view.byteLength===3,'borrowed subview is not detached');subviewController.dispose();
    let releaseArrival;const arrival=new Promise(resolve=>releaseArrival=resolve);const before=context({...source,async *enumerate(){await arrival;yield {kind:'file',path:'before.mod',size:1,timestamp:''};}},'before-row');mode='delay';const beforeController=createDiscoveryController({context:before,digest});beforeController.start();beforeController.acceptPlaybackMetadata(locate('before.mod'),metadata('before old'),bytes);beforeController.acceptPlaybackMetadata(locate('before.mod'),metadata('before latest'),bytes);gates[1].release();await wait(()=>before.discoveryStatus.metadataCounts.hashesPending===1);releaseArrival();await wait(()=>before.library.rows.length===1);check(before.library.rows[0].title==='before latest'&&before.library.rows[0].sha256===seedSha,'before-row latest coherent cache');gates[0].release();gates.length=0;await wait(()=>!before.discoveryStatus.metadataCounts.hashesPending);beforeController.dispose();
    // Real endpoint: actual WASM unreachable is fatal; explicit retry keeps page digest alive.
    const endpoint=new URL('./library-worker-jobs.js',location.href).href,bindings=new URL('./starplayer_web.js',location.href).href,wasm=new URL('./starplayer_web_bg.wasm',location.href).href;
    const trapModule=[0,97,115,109,1,0,0,0,1,4,1,96,0,0,3,2,1,0,7,8,1,4,116,114,97,112,0,0,10,5,1,3,0,0,11];
    const workerCode=`import {startLibraryWorker} from ${JSON.stringify(endpoint)};import * as Loader from ${JSON.stringify(bindings)};await Loader.default({module_or_path:${JSON.stringify(wasm)}});const trap=new WebAssembly.Instance(new WebAssembly.Module(Uint8Array.from(${JSON.stringify(trapModule)}))).exports.trap;startLibraryWorker({scope:self,loadLoader:async()=>({...Loader,inspect_s3m:()=>trap()})});`;
    const trapURL=URL.createObjectURL(new Blob([workerCode],{type:'text/javascript'}));let first=true,trapMetrics=[];
    const trapped=context({...source,async *enumerate(){for(const path of ['A.mod','B.mod','C.mod'])yield {kind:'file',path,size:bytes.length,timestamp:''};}},'trap');mode='delay';
    const trappedController=createDiscoveryController({context:trapped,digest,onDigestMetric:metric=>trapMetrics.push(metric),runtimeFactory:options=>createLibraryWorkerRuntime({...options,workerFactory:()=>{const url=first?trapURL:new URL('./library-worker.js',location.href);first=false;return new Worker(url,{type:'module'});}})});
    trappedController.start();await wait(()=>trapped.library.rows.length===3);trappedController.acceptPlaybackMetadata(locate('B.mod'),metadata('survives trap retry cancel'),bytes);trappedController.scanMetadata();await wait(()=>!!trapped.discoveryStatus.runtimeError);check(trapped.library.rows[0].metadataState==='unknown'&&trapped.discoveryStatus.metadataPhase==='suspended','actual trap suspends without corrupt row');
    check(trapped.discoveryStatus.metadataCounts.hashesPending===1,'digest physically spans actual trap');await new Promise(resolve=>setTimeout(resolve,60));check(trapped.library.rows[2].metadataState==='unknown','queued C remains suspended after actual trap');trappedController.retry();await wait(()=>trapped.library.rows[0].metadataState==='ready'&&trapped.library.rows[2].metadataState==='ready');trappedController.cancelMetadata();check(trapped.discoveryStatus.metadataCounts.hashesPending===1,'digest survives actual fatal endpoint and explicit retry');gates.splice(0).forEach(gate=>gate.release());await wait(()=>trapped.library.rows[1].title==='survives trap retry cancel'&&!trapped.discoveryStatus.metadataCounts.hashesPending);check(!trapped.discoveryStatus.runtimeError,'explicit retry clears fatal state');trappedController.dispose();URL.revokeObjectURL(trapURL);
    // Frozen immutable real group: playback invalidates only A; B/C continue exactly once.
    let gated=false,readStarted=false,releaseRead;const readGate=new Promise(resolve=>releaseRead=resolve);let cancels=0,deliveries=[];
    const grouped=context({kind:'filelist',label:'group',size:()=>zip.length,readFile:async()=>{if(gated){readStarted=true;await readGate;}return zip.slice();},async *enumerate(){yield {kind:'file',path:'group.zip',size:zip.length,timestamp:''};}},'group');
    const groupController=createDiscoveryController({context:grouped,runtimeFactory:options=>{const runtime=createLibraryWorkerRuntime(options);return {...runtime,enqueue:(job,hooks)=>{const handle=runtime.enqueue(job,job.kind==='inspect-group'?{...hooks,onMember:frame=>{deliveries.push(frame.locator.memberPath);hooks.onMember(frame);}}:hooks);return {...handle,cancel:()=>{cancels++;handle.cancel();}};}};}});
    groupController.start();await wait(()=>grouped.library.rows.length===3);gated=true;groupController.scanMetadata();await wait(()=>readStarted);groupController.acceptPlaybackMetadata({...locate('group.zip'),memberPath:'A.mod'},metadata('played A'),bytes);await wait(()=>grouped.library.rows.find(row=>row.memberPath==='A.mod').title==='played A');check(cancels===0,'playback does not cancel shared group');releaseRead();await wait(()=>grouped.discoveryStatus.metadataPhase==='complete');check(grouped.library.rows.filter(row=>row.memberPath!=='A.mod').every(row=>row.metadataState==='ready'),'B/C shared group continue');check(deliveries.length===3&&new Set(deliveries).size===3,'group settles all members once');check(grouped.library.rows.find(row=>row.memberPath==='A.mod').title==='played A','stale A inspection ignored');groupController.dispose();
    // Callback failures before and after application insert trusted path issues and clean descriptors.
    const callbackResults=[];
    for(const after of [false,true]) {
        let once=true;const target=context({...source,async *enumerate(){yield {kind:'file',path:'A.mod',size:bytes.length,timestamp:''};}},'callback');
        const originalError=console.error;console.error=(...args)=>{if(args[0]!=='Library metadata callback failed')originalError(...args);};
        const controller=createDiscoveryController({context:target,runtimeFactory:options=>{const runtime=createLibraryWorkerRuntime(options);return {...runtime,enqueue:(job,hooks)=>runtime.enqueue(job,{...hooks,onMember:frame=>{if(after||!once)hooks.onMember(frame);if(once){once=false;throw new Error('deliberate callback test');}}})};}});
        try {controller.start();await wait(()=>target.library.rows.length===1);controller.scanMetadata();await wait(()=>target.discoveryStatus.issueCount===1&&target.discoveryStatus.metadataPhase==='complete');check(target.discoveryStatus.issues[0].path==='A.mod'&&target.library.rows[0].metadataState!=='error','callback issue belongs to controller');check(after?target.library.rows[0].metadataState==='ready':target.discoveryStatus.metadataCounts.scanActionable===1,'callback terminal cleanup');if(!after){controller.scanMetadata();await wait(()=>target.library.rows[0].metadataState==='ready');}callbackResults.push({after,clean:true});}finally{controller.dispose();console.error=originalError;}
    }
    const excluded=context({kind:'filelist',label:'excluded',size:()=>0,readFile:async()=>new Uint8Array(),async *enumerate(){yield {kind:'file',path:'excluded.zip',size:0,timestamp:''};}},'excluded');
    const excludedRuntime={enqueue:job=>({id:1,result:Promise.resolve({generation:job.generation,kind:'catalog',payload:{entries:[{containerMember:'',memberPath:'excluded.mod',rawHex:'',size:1,format:'mod',playable:false,error:'encrypted'}],issues:[]}}),cancel(){}}),pauseCatalog(){},resumeCatalog(){},cancelMetadata(){},dispose(){}};
    const excludedController=createDiscoveryController({context:excluded,runtimeFactory:()=>excludedRuntime});excludedController.start();await wait(()=>excluded.discoveryStatus.phase==='complete');check(excluded.discoveryStatus.metadataCounts.excluded===1&&!excluded.discoveryStatus.metadataIncomplete&&!excluded.discoveryStatus.metadataCounts.scanActionable,'excluded-only metadata complete');excludedController.attachComments(new Map());check(excluded.discoveryStatus.sidecarCounts.excludedHashes===1&&!excluded.discoveryStatus.sidecarCounts.awaitingHashes,'excluded missing SHA separated');excludedController.dispose();
    const mixedBytes=Uint8Array.from(mixedArray);
    const mixed=context({kind:'filelist',label:'mixed',size:()=>mixedBytes.length,readFile:async()=>mixedBytes.slice(),async *enumerate(){yield {kind:'file',path:'mixed.zip',size:mixedBytes.length,timestamp:''};}},'mixed');
    const mixedController=createDiscoveryController({context:mixed});mixedController.start();await wait(()=>mixed.discoveryStatus.phase==='complete');mixedController.attachComments(new Map([[seedSha,{instrumentNames:'joined valid',sampleNames:'',songMessage:''}]]));
    check(mixed.discoveryStatus.metadataCounts.unknown===2&&mixed.discoveryStatus.metadataCounts.excluded===1&&mixed.discoveryStatus.sidecarCounts.awaitingHashes===2&&mixed.discoveryStatus.sidecarCounts.excludedHashes===1,'mixed eligibility before scan');
    mixedController.scanMetadata();await wait(()=>mixed.discoveryStatus.metadataPhase==='complete');check(mixed.discoveryStatus.metadataCounts.ready===1&&mixed.discoveryStatus.metadataCounts.error===1&&mixed.discoveryStatus.metadataCounts.excluded===1&&!mixed.discoveryStatus.metadataCounts.scanActionable,'mixed eligible ready/error/excluded totals');check(mixed.discoveryStatus.sidecarCounts.joined===1&&mixed.discoveryStatus.sidecarCounts.failedHashes===1&&mixed.discoveryStatus.sidecarCounts.excludedHashes===1&&!mixed.discoveryStatus.sidecarCounts.awaitingHashes,'mixed sidecar error/exclusion separation');mixedController.dispose();
    const empty=context({kind:'filelist',label:'empty',async *enumerate(){}},'empty');const emptyController=createDiscoveryController({context:empty});emptyController.start();await wait(()=>empty.discoveryStatus.phase==='complete');check(!empty.discoveryStatus.metadataIncomplete&&!empty.discoveryStatus.metadataCounts.scanActionable&&empty.library.rows.length===0,'empty library has no missing metadata work');emptyController.dispose();
    let errorReads=0;const errorOnly=context({kind:'filelist',label:'error only',size:()=>3,readFile:async()=>{errorReads++;return new Uint8Array([1,2,3]);},async *enumerate(){yield {kind:'file',path:'bad.mod',size:3,timestamp:''};}},'error only');const errorController=createDiscoveryController({context:errorOnly});errorController.start();await wait(()=>errorOnly.discoveryStatus.phase==='complete');errorController.attachComments(new Map([[seedSha,{instrumentNames:'unmatched',sampleNames:'',songMessage:''}]]));errorController.scanMetadata();await wait(()=>errorOnly.discoveryStatus.metadataPhase==='complete');check(errorOnly.discoveryStatus.metadataCounts.error===1&&!errorOnly.discoveryStatus.metadataCounts.unknown&&!errorOnly.discoveryStatus.metadataCounts.scanActionable&&errorOnly.discoveryStatus.metadataIncomplete,'error-only library requires explicit retry');check(errorOnly.discoveryStatus.sidecarCounts.failedHashes===1&&!errorOnly.discoveryStatus.sidecarCounts.awaitingHashes,'error-only sidecar failure count');errorController.scanMetadata();await new Promise(resolve=>setTimeout(resolve,60));check(errorReads===1,'Scan does not retry error-only library');errorController.retryMetadata({sourcePath:'bad.mod'});await wait(()=>errorReads===2&&errorOnly.discoveryStatus.metadataPhase==='complete');errorController.dispose();
    // A page digest is independent of a real group-boundary worker recycle.
    const recycling=context({...source,size:()=>32*1024*1024,readFile:async()=>{const padded=new Uint8Array(32*1024*1024);padded.set(bytes);return padded;},async *enumerate(){for(const path of ['A.mod','B.mod'])yield {kind:'file',path,size:32*1024*1024,timestamp:''};}},'recycle');
    mode='delay';const recycleEvents=[];const recycleController=createDiscoveryController({context:recycling,digest,runtimeFactory:options=>createLibraryWorkerRuntime({...options,onMetric:event=>recycleEvents.push(event)})});recycleController.start();await wait(()=>recycling.discoveryStatus.phase==='complete');recycleController.acceptPlaybackMetadata(locate('A.mod'),metadata('survives recycle'),bytes);recycleController.scanMetadata();await wait(()=>recycleEvents.some(event=>event.event==='recycle'));check(recycling.discoveryStatus.metadataCounts.hashesPending===1,'recycling preserves page digest');gates.splice(0).forEach(gate=>gate.release());await wait(()=>recycling.library.rows[0].title==='survives recycle'&&!recycling.discoveryStatus.metadataCounts.hashesPending);recycleController.dispose();
    return {atomicReplay:true,firstPlayUnknown:true,snapshotMutation:true,independentABC:true,supersededPhysicalAccounting:true,staleAndDisposedCleanup:true,syncAsyncInvalidFailures:true,beforeRowVersions:true,nativeSubview:true,actualWasmEndpointTrap:true,actualTrapSuspendsQueuedC:true,explicitRetryHealthyWorker:true,explicitRetryAndCancelDigest:true,sharedGroupABC:true,callbackResults,excludedOnly:true,mixedErrorSidecars:true,emptyLibrary:true,errorOnlyExplicitRetry:true,pageDigestAcrossRecycle:true,metrics};
}

async function archiveMatrixScenario(base) {
    const {createLibraryWorkerRuntime}=await import('./library-worker-runtime.js');
    const check=(ok,why)=>{if(!ok)throw new Error(why);};
    const results=[];
    for(const nested of [false,true])for(const large of [false,true])for(const total of [3,64,130]) {
        let reads=0,delivered=0,sourceBytes=0;const events=[];
        const url=base+`?nested=${Number(nested)}&large=${Number(large)}&total=${total}`;
        const source={size:()=>null,readFile:async()=>{reads++;const bytes=new Uint8Array(await (await fetch(url)).arrayBuffer());sourceBytes=bytes.byteLength;return bytes;}};
        const runtime=createLibraryWorkerRuntime({source,onMetric:event=>events.push(event)});
        try {
            for(let offset=0;offset<total;offset+=64) {
                const members=Array.from({length:Math.min(64,total-offset)},(_,index)=>({token:String(index+offset),version:0,locator:{sourcePath:'matrix.zip',containerMember:nested?'inner.zip':'',memberPath:String(index+offset).padStart(3,'0')+'.mod',rawHex:''}}));
                const result=await runtime.enqueue({generation:1,kind:'inspect-group',sourcePath:'matrix.zip',members,purpose:'scan'},{onMember:frame=>{check(!frame.error&&frame.payload.metadata.title==='matrix snapshot'&&frame.payload.sha256.length===64,'actual worker matrix result');delivered++;}}).result;
                check(result.payload.completedTokens.length===members.length,'matrix complete token accounting');
            }
            const groups=Math.ceil(total/64),recycles=events.filter(event=>event.event==='recycle').length,instances=events.filter(event=>event.event==='instance').length;
            check(reads===groups&&delivered===total,'one source read per group of at most 64 members');check(large?sourceBytes>=32*1024*1024:sourceBytes<32*1024*1024,'matrix source size boundary');check(recycles===(large?groups:total>=100?1:0),'group-boundary recycle threshold');
            results.push({nested,large,total,sourceBytes,reads,groups,recycles,instances,delivered,innerDecodeCount:'native counter is cfg(test) only; see Rust evidence, not inferred from adapter calls'});
        } finally {runtime.dispose();}
    }
    // Catalog, scan groups and playback each have explicitly attributed source reads.
    let version=0,totalReads=0;const source={size:()=>null,readFile:async()=>{totalReads++;return new Uint8Array(await(await fetch(base+'?nested=1&large=0&total=3&version='+version)).arrayBuffer());}};
    const runtime=createLibraryWorkerRuntime({source});
    const members=Array.from({length:3},(_,index)=>({token:String(index),version:0,locator:{sourcePath:'mutable.zip',containerMember:'inner.zip',memberPath:String(index).padStart(3,'0')+'.mod',rawHex:''}}));
    try{
        await runtime.enqueue({generation:2,kind:'catalog',sourcePath:'mutable.zip'}).result;const catalogReads=totalReads;
        const old=[];await runtime.enqueue({generation:2,kind:'inspect-group',sourcePath:'mutable.zip',members,purpose:'scan'},{onMember:frame=>{old.push(frame.payload.metadata.title);version=1;}}).result;check(old.every(title=>title==='matrix snapshot'),'source mutation cannot alter open group snapshot');
        const next=[];await runtime.enqueue({generation:2,kind:'inspect-group',sourcePath:'mutable.zip',members,purpose:'scan'},{onMember:frame=>next.push(frame.payload.metadata.title)}).result;check(next.every(title=>title==='next snapshot'),'next group sees changed source');const scanReads=totalReads-catalogReads;
        const Loader=await import('./starplayer_web.js'),packed=await source.readFile('mutable.zip');const session=new Loader.ArchiveInspectionSession(packed);try{const extracted=session.extract_member('inner.zip','000.mod','');Loader.inspect_s3m(extracted);check(JSON.parse(Loader.module_metadata()).title==='next snapshot','playback extraction sees latest source');}finally{session.free();}
        results.push({mutationBetweenGroups:true,immutableWithinGroup:true,playbackSeesLatest:true,catalogReads,scanReads,playbackReads:totalReads-catalogReads-scanReads});
    }finally{runtime.dispose();}
    let release,readStarted=false;const barrier=new Promise(resolve=>release=resolve);
    const cancelRuntime=createLibraryWorkerRuntime({source:{size:()=>1,readFile:async()=>{readStarted=true;await barrier;return new Uint8Array([1]);}}});
    const cancelled=cancelRuntime.enqueue({generation:3,kind:'inspect-group',sourcePath:'cancel.mod',members:[{token:'cancel',version:0,locator:{sourcePath:'cancel.mod'}}],purpose:'scan'});cancelled.result.catch(()=>{});
    while(!readStarted)await new Promise(resolve=>setTimeout(resolve,1));
    let acknowledged=false;const input=document.getElementById('library-search');input.addEventListener('input',()=>{acknowledged=true;},{once:true});input.value='while group gated';input.dispatchEvent(new Event('input'));check(acknowledged,'input acknowledged before source gate releases');
    cancelled.cancel();const error=await cancelled.result.catch(error=>error);check(error.code==='aborted'&&error.progress.completedTokens.length===0,'explicit whole-group cancellation');release();cancelRuntime.dispose();results.push({explicitGroupCancellation:true,inputAcknowledgedBeforeRelease:true});
    return results;
}

async function selectionScenario(modArray) {
    const {wireLibraryControls}=await import('./library-controls.js');
    const {createLibraryView}=await import('./library-view.js');
    const {folderIdentity}=await import('./library-source.js');
    const panel=document.getElementById('library-panel');panel.replaceWith(panel.cloneNode(true));document.getElementById('library-heading').replaceChildren();document.getElementById('library-match-popover')?.remove();
    const el=name=>document.getElementById('library-'+name),check=(ok,why)=>{if(!ok)throw new Error(why);};
    const wait=async predicate=>{const deadline=performance.now()+20000;while(!predicate()){if(performance.now()>deadline)throw new Error('selection timeout '+el('selection-status').textContent);await new Promise(resolve=>setTimeout(resolve,10));}};
    const sourceControls=document.querySelector('[aria-label="Library sources"]');sourceControls.replaceWith(sourceControls.cloneNode(true));
    const state={},updates=[];let rejectIdentity=false;
    const view=createLibraryView({playlist:{add(){}},play(){}});
    await wireLibraryControls({state,loader:await import('./starplayer_web.js'),identityForFolder:source=>{if(rejectIdentity)throw new Error('deliberate identity failure');return folderIdentity(source);},onLibrary:context=>view.setLibrary(context),onLibraryUpdate:(context,change)=>{updates.push(context);view.updateLibrary(context,change);}});
    const root=await navigator.storage.getDirectory();const accepted=await root.getDirectoryHandle('accepted-remediation',{create:true}),candidate=await root.getDirectoryHandle('candidate-remediation',{create:true});
    const write=async(root,name,data)=>{const handle=await root.getFileHandle(name,{create:true}),out=await handle.createWritable();await out.write(data);await out.close();};
    await write(accepted,'live.mod',Uint8Array.from(modArray));await write(candidate,'a.csv','source_path\nlive.mod');await write(candidate,'b.csv','source_path\nlive.mod');
    window.showDirectoryPicker=async()=>accepted;el('folder').click();await wait(()=>state.library?.library.rows.length===1&&state.library.discoveryStatus.phase==='complete');const current=state.library;
    window.showDirectoryPicker=async()=>candidate;el('folder').click();await wait(()=>!el('choices').hidden);check(state.library===current,'pending chooser preserves context');
    current.discovery.pause();await wait(()=>el('summary').textContent.includes('paused'));current.discovery.resume();await wait(()=>el('summary').textContent.includes('complete'));
    current.discovery.acceptPlaybackMetadata({sourcePath:'live.mod'},{title:'accepted while chooser pending',format:'mod',channels:4},Uint8Array.from(modArray));await wait(()=>current.library.rows[0].title==='accepted while chooser pending');
    const updateCount=updates.length;await wait(()=>updates.length>updateCount||el('rows').textContent.includes('accepted while chooser pending'));check(state.library===current&&el('selection-status').textContent.includes('Choose'),'accepted callbacks and chooser status coexist');
    const transfer=new DataTransfer();transfer.items.add(new File(['source_path\n"unterminated'],'invalid.csv'));el('index').files=transfer.files;el('index').dispatchEvent(new Event('change'));await wait(()=>el('selection-status').textContent.includes('invalid.csv selected'));window.showDirectoryPicker=async()=>candidate;el('folder').click();await wait(()=>/quot|unterminated/i.test(el('selection-status').textContent));check(state.library===current,'invalid index preserves context');
    rejectIdentity=true;window.showDirectoryPicker=async()=>accepted;el('browse-folder').click();await wait(()=>el('selection-status').textContent.includes('identity failure'));check(state.library===current,'identity failure preserves context');
    current.discovery.acceptPlaybackMetadata({sourcePath:'live.mod'},{title:'accepted after identity failure',format:'mod',channels:4},Uint8Array.from(modArray));await wait(()=>current.library.rows[0].title==='accepted after identity failure');current.discovery.dispose();
    check(document.getElementById('transport-chip').textContent==='playing'&&document.getElementById('title').textContent==='remediation group','pending selection leaves actual audio unchanged');
    return {pendingChooser:true,invalidIndex:true,identityFailure:true,acceptedStopResume:true,acceptedPlaybackEnrichment:true};
}

async function digestScenario(modArray, archiveBase) {
    const {createDiscoveryController}=await import('./library-discovery.js');
    const {createDiscoveredLibrary}=await import('./library-index.js');
    const Loader=await import('./starplayer_web.js');
    const check=(ok,why)=>{if(!ok)throw new Error(why);};
    const wait=async predicate=>{const deadline=performance.now()+60000;while(!predicate()){if(performance.now()>deadline)throw new Error('native digest/activation timeout');await new Promise(resolve=>setTimeout(resolve,5));}};
    const distribution=values=>{const sorted=values.slice().sort((a,b)=>a-b);return {count:values.length,p50:sorted[Math.floor(sorted.length*.5)],p95:sorted[Math.min(sorted.length-1,Math.floor(sorted.length*.95))],max:Math.max(...values)};};
    const summary=[];
    for(const fixture of [{size:1},{size:32},{size:64},{size:128},{size:64,archive:true}]) {
        let input,archiveSourceBytes=null;
        if(fixture.archive){const packed=new Uint8Array(await (await fetch(archiveBase)).arrayBuffer());archiveSourceBytes=packed.byteLength;const session=new Loader.ArchiveInspectionSession(packed);try{input=session.extract_member('','large.mod','');}finally{session.free();}}
        else {input=new Uint8Array(fixture.size*1024*1024);input.set(modArray);}
        check(input.byteLength===fixture.size*1024*1024,'actual accepted input size');
        Loader.inspect_s3m(input);const acceptedMetadata=JSON.parse(Loader.module_metadata());check(acceptedMetadata.format==='mod'&&acceptedMetadata.channels===4,'existing loader accepts padding without altered validity');
        const runs=[];
        for(let repeat=0;repeat<3;repeat++) {
            const metrics=[];const source={kind:'filelist',label:'native digest',size:()=>input.byteLength,readFile:async()=>{throw new Error('digest measurement must not read source');},async *enumerate(){yield {kind:'file',path:'measure.mod',size:input.byteLength,timestamp:''};}};
            const context={mode:'discovered',source,library:createDiscoveredLibrary({name:'digest'}),identity:{sha256:'c'.repeat(64),name:'digest'}};
            const controller=createDiscoveryController({context,onDigestMetric:metric=>metrics.push(metric)});controller.start();await wait(()=>context.discoveryStatus.phase==='complete');
            Loader.inspect_s3m(input);const currentMetadata=JSON.parse(Loader.module_metadata());
            const callCosts=[];for(let play=0;play<4;play++){const start=performance.now();controller.acceptPlaybackMetadata({sourcePath:'measure.mod'},currentMetadata,input);callCosts.push(performance.now()-start);}
            await wait(()=>context.discoveryStatus.metadataCounts.hashesPending===0);check(metrics.filter(metric=>metric.event==='start').length===4&&metrics.filter(metric=>metric.event==='settle').length===4&&metrics.at(-1).pendingInputBytes===0,'native physical promise settlement and cleanup');check(context.library.rows[0].sha256.length===64,'native digest publication');controller.dispose();
            // Measure actual application activation separately, without assuming its ordering relative to hashes.
            const title='native '+fixture.size+' '+Number(!!fixture.archive)+' '+repeat;input.fill(0,0,20);input.set(new TextEncoder().encode(title),0);
            const transfer=new DataTransfer();transfer.items.add(new File([input],'native-measure.mod'));const picker=document.getElementById('file-picker');picker.files=transfer.files;const activationStart=performance.now();picker.dispatchEvent(new Event('change'));
            await wait(()=>document.getElementById('title').textContent===title&&document.getElementById('transport-chip').textContent==='playing');const activationMilliseconds=performance.now()-activationStart;
            document.getElementById('stop').click();await wait(()=>document.getElementById('transport-chip').textContent==='stopped');
            runs.push({repeat,rapidCalls:4,actualInputBytes:input.byteLength,peakOutstanding:Math.max(...metrics.map(metric=>metric.pending)),peakPendingInputBytes:Math.max(...metrics.map(metric=>metric.pendingInputBytes)),nativeStartCost:distribution(metrics.filter(metric=>metric.event==='start').map(metric=>metric.startCost)),controllerInvocation:distribution(callCosts),settlementLatency:distribution(metrics.filter(metric=>metric.event==='settle').map(metric=>metric.latency)),activationMilliseconds,metrics});
        }
        summary.push({source:fixture.archive?'archive-extracted':'loose',sourceMiB:fixture.size,archiveSourceBytes,acceptedInputBytes:input.byteLength,repeats:3,runs,activationLatency:distribution(runs.map(run=>run.activationMilliseconds)),allStartCosts:distribution(runs.flatMap(run=>run.metrics.filter(metric=>metric.event==='start').map(metric=>metric.startCost))),allSettlementLatency:distribution(runs.flatMap(run=>run.metrics.filter(metric=>metric.event==='settle').map(metric=>metric.latency)))});
        input=null;
    }
    return {summary,limitation:'SUM(byteLength) for physically outstanding native calls is a conservative input-snapshot budgeting estimate, not measured total browser memory or a proven upper bound on WebCrypto overhead/copies. Controller instrumentation retains scalar events only, no module buffers. Native invocation cost includes browser snapshot copying plus controller work. Settlement latency is observed at the JS completion callback and includes delivery delay; three repeats provide 12 native samples and 3 activation samples per size, not a statistical confidence bound. Each repeat issues four synchronous controller digest requests and measures one real application activation separately after they settle; no concurrency cap or activation-vs-digest speed assertion. Actual activation is measured separately after native digests settle; only synthetic valid MODs with loader-accepted trailing padding are used.'};
}

export async function runVisibilityReturn(options) {
    const {page,server}=await open(options);
    try {
        await page.evaluate(`
            const {createDiscoveryController}=await import('./library-discovery.js');const {createDiscoveredLibrary}=await import('./library-index.js');const {createLibraryView}=await import('./library-view.js');
            const panel=document.getElementById('library-panel');panel.replaceWith(panel.cloneNode(true));document.getElementById('library-heading').replaceChildren();document.getElementById('library-match-popover')?.remove();
            const source={kind:'filelist',label:'debounce return',size:()=>1,async *enumerate(){for(const path of ['A.mod','B.mod'])yield {kind:'file',path,size:1,timestamp:''};}};
            window.returnContext={mode:'discovered',source,library:createDiscoveredLibrary({name:'debounce return'}),identity:{sha256:'a'.repeat(64),name:'debounce return'}};
            window.returnView=createLibraryView({playlist:{add(){}},play(){}});window.returnGates=[];window.returnDigests=[];window.returnNotifications=[];
            const digest=bytes=>{const native=crypto.subtle.digest('SHA-256',bytes);let release;const gate=new Promise(resolve=>release=resolve);returnGates.push(release);const result=native.then(hash=>gate.then(()=>hash));returnDigests.push(result);return result;};
            window.returnController=createDiscoveryController({context:returnContext,digest,onUpdate:(context,change)=>{returnNotifications.push({time:performance.now(),rows:change.rowChanges.length,refresh:context.discoveryStatus.metadataCounts.refreshPending});returnView.updateLibrary(context,change);}});returnView.setLibrary(returnContext);returnController.start();
        `);
        await page.waitFor('initial visible rows',"returnContext.discoveryStatus.phase==='complete'&&document.querySelectorAll('.library-track').length===2");
        await page.evaluate('await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));');
        const other=await page.send('Target.createTarget',{url:'about:blank'});await page.send('Target.activateTarget',{targetId:other.targetId});await page.waitFor('normal hidden debounce tab',"document.visibilityState==='hidden'");
        const hiddenStart=Date.now();
        const before=await page.evaluate(`
            const metadata=title=>({title,format:'mod',channels:4});const bytes=new Uint8Array([1,2,3]);
            returnController.acceptPlaybackMetadata({sourcePath:'A.mod'},metadata('nonterminal visible delta'),bytes);
            returnController.acceptPlaybackMetadata({sourcePath:'B.mod'},metadata('held refresh'),bytes);
            returnGates[0]();await returnDigests[0];await Promise.resolve();await Promise.resolve();
            return {time:performance.now(),model:returnContext.library.rows[0].title,dom:document.getElementById('library-rows').textContent,pending:returnContext.discoveryStatus.metadataCounts.refreshPending,hashes:returnContext.discoveryStatus.metadataCounts.hashesPending,notifications:returnNotifications.length,visibility:document.visibilityState};
        `);
        assert.equal(before.visibility,'hidden');assert.equal(before.model,'nonterminal visible delta');assert.equal(before.pending,1);assert.ok(!before.dom.includes('nonterminal visible delta'),'model changed while DOM awaits real debounce');
        const returning=Date.now();await page.send('Target.activateTarget',{targetId:page.targetId});await page.waitFor('visible nonterminal debounce delta',"document.visibilityState==='visible'&&document.getElementById('library-rows').textContent.includes('nonterminal visible delta')");
        const domAfterReturnMilliseconds=Date.now()-returning,hiddenMilliseconds=returning-hiddenStart;
        await page.waitFor('actual nonterminal debounced row-change publication',"returnNotifications.some(event=>event.rows>0&&event.refresh===1)");
        const debouncedNotificationAfterReturnMilliseconds=Date.now()-returning;
        const after=await page.evaluate('return {pending:returnContext.discoveryStatus.metadataCounts.refreshPending,notifications:returnNotifications};');assert.equal(after.pending,1,'second refresh held through actual DOM publication');
        await page.evaluate('returnGates[1]();return true;');await page.waitFor('held native-compatible digest cleanup',"returnContext.discoveryStatus.metadataCounts.hashesPending===0");await page.evaluate('returnController.dispose();');assert.deepEqual(page.consoleErrors,[]);
        return {version:page.version,flags:page.browserArguments,hiddenMilliseconds,before,after,domAfterReturnMilliseconds,debouncedNotificationAfterReturnMilliseconds,normalThrottling:true,nonterminalDebounce:true,limitation:'Quick genuine hidden-tab debounce-return test; six-minute intensive scheduling evidence is recorded separately. No forced flush or visibility simulation.'};
    }finally{await page.close();server.stop();}
}

export async function runEnumerationDiagnostic(options) {
    const {page,server}=await open(options);
    try{return await page.evaluate(`
        const {createDirectorySource,createFileListSource}=await import('./library-source.js');const {createDiscoveryController}=await import('./library-discovery.js');const {createDiscoveredLibrary}=await import('./library-index.js');
        const method=(await import('./cooperative-yield.js')).cooperativeYieldMethod();
        const reports=[];
        for(const kind of ['directory','filelist']) {
            const files=Array.from({length:40000},(_,index)=>({name:'track'+index+'.mod',webkitRelativePath:'diagnostic/group/track'+index+'.mod',size:1,lastModified:0}));
            const group={kind:'directory',name:'group',async *entries(){for(const file of files)yield [file.name,{kind:'file',name:file.name}];}};const root={kind:'directory',name:'diagnostic',async *entries(){yield ['group',group];}};
            const source=kind==='directory'?await createDirectorySource(root):await createFileListSource(files);const context={mode:'discovered',source,library:createDiscoveredLibrary({name:kind}),identity:{sha256:'a'.repeat(64),name:kind}};
            const publications=[],start=performance.now();const controller=createDiscoveryController({context,onUpdate:(context,change)=>publications.push({milliseconds:performance.now()-start,rows:context.library.rows.length,phase:context.discoveryStatus.phase,delta:change.rowChanges.length})});controller.start();
            while(context.discoveryStatus.phase!=='complete')await new Promise(resolve=>setTimeout(resolve,10));
            reports.push({kind,enumerationMilliseconds:performance.now()-start,publications,partialPublication:publications.some(event=>event.rows>0&&event.rows<40000)});controller.dispose();
        }
        return {method,reports};
    `);}finally{await page.close();server.stop();}
}
