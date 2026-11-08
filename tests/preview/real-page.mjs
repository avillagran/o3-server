import fs from 'node:fs';import path from 'node:path';import assert from 'node:assert/strict';import {spawn} from 'node:child_process';import {once} from 'node:events';import {createRequire} from 'node:module';
import {RetainingArtifactFs} from '../../runtime/preview/retaining-artifact-fs.mjs';import {TabCdpRelay} from '../../runtime/preview/tab-cdp-relay.mjs';import {attachPage} from '../../runtime/preview/page-worker.mjs';import {FixtureCdp} from './fixture-cdp.mjs';
assert(fs.existsSync('/run/.containerenv'));const out='/out';
for(const name of ['tmp','quarantine','profile','guard'])fs.mkdirSync(path.join(out,name),{mode:0o700});
process.env.TMPDIR=out+'/tmp';process.env.O3_RECURSIVE_FS_GUARD_DIR=out+'/guard';await import('/guard.mjs');
const artifacts=new RetainingArtifactFs({tempRoot:out+'/tmp',quarantineRoot:out+'/quarantine'});artifacts.install();
const require=createRequire('/playwright/package.json');const {chromium}=require('/playwright');const {ws:WebSocket,wsServer:WebSocketServer}=require('/playwright/lib/utilsBundle.js');
const nativePage=await import('/native/apps/server/src/preview/ServerBrowserPage.ts');
const nativeContracts=await import('/native/packages/contracts/src/previewAutomation.ts');const Schema=await import('/deps/node_modules/effect/dist/Schema.js');
const child=spawn('/usr/bin/chromium',['--headless','--no-sandbox','--disable-gpu','--no-first-run','--disable-background-networking','--disable-dev-shm-usage','--remote-debugging-port=0','--user-data-dir='+out+'/profile','about:blank'],{stdio:['ignore','ignore','pipe']});
let stderr='';let browser;let relay;let otherBrowser;let otherRelay;let upstream;let endpoint;let failure;
try{
 endpoint=await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('Chromium startup deadline')),20000);child.stderr.on('data',data=>{stderr=(stderr+data).slice(-4096);const found=/DevTools listening on (ws:\/\/127\.0\.0\.1:[^\s]+)/.exec(stderr);if(found){clearTimeout(timer);resolve(found[1]);}});child.once('exit',()=>{clearTimeout(timer);reject(Error('Owned Chromium stopped'));});});
 const socket=new WebSocket(endpoint);await once(socket,'open');upstream=new FixtureCdp(socket);
 const root=(await upstream.send('Target.createTarget',{url:'about:blank'})).targetId;const foreign=(await upstream.send('Target.createTarget',{url:'about:blank'})).targetId;
 const foreignSession=(await upstream.send('Target.attachToTarget',{targetId:foreign,flatten:true})).sessionId;
 let admitted=true;relay=new TabCdpRelay({upstream,targetId:root,admitted:()=>admitted,WebSocketServer});const scoped=await relay.listen();
 browser=await attachPage({chromium,endpoint:scoped,targetId:root,nativePage,nativeContracts,Schema,admitted:()=>admitted,artifactFs:artifacts});
 await browser.page.bringToFront();
 await browser.page.setContent(`<button id="go" onclick="this.textContent='Done'">Start</button><input id="field"><input type="password" value="private-do-not-leak"><select id="select"><option value="a">A</option><option value="b">B</option></select><button id="hover" onmouseover="this.dataset.over='yes'">Hover</button><div id="scroll" style="height:50px;overflow:auto"><div style="height:1000px">scroll</div></div><div id="drag" draggable="true" ondragstart="event.dataTransfer.setData('text/plain','owned')">Drag</div><div id="drop" style="height:60px" ondragover="event.preventDefault()" ondrop="event.preventDefault();this.dataset.dropped=event.dataTransfer.getData('text/plain')">Drop</div><iframe srcdoc="<input id='inside'>"></iframe>`);
 const invalidBefore=upstream.calls.length;await assert.rejects(browser.invoke('click',{x:1}));assert.equal(upstream.calls.length,invalidBefore);
 await browser.invoke('click',{locator:'#go',timeoutMs:2000});assert.equal(await browser.page.locator('#go').textContent(),'Done');
 await browser.invoke('type',{locator:'#field',text:'synthetic',clear:true});assert.equal(await browser.page.locator('#field').inputValue(),'synthetic');
 await browser.invoke('press',{key:'End'});await browser.invoke('press',{key:'x'});assert.equal(await browser.page.locator('#field').inputValue(),'syntheticx');
 await browser.invoke('hover',{locator:'#hover',timeoutMs:2000});assert.equal(await browser.page.locator('#hover').getAttribute('data-over'),'yes');
 await browser.invoke('scroll',{locator:'#scroll',deltaY:100});assert((await browser.page.locator('#scroll').evaluate(e=>e.scrollTop))>0);
 await browser.invoke('drag',{source:'#drag',target:'#drop',timeoutMs:2000});assert.equal(await browser.page.locator('#drop').getAttribute('data-dropped'),'owned');
 await browser.invoke('type',{locator:'iframe >> internal:control=enter-frame >> #inside',text:'inside',clear:true,timeoutMs:2000});assert.equal(await browser.page.frameLocator('iframe').locator('#inside').inputValue(),'inside');
 await browser.invoke('select',{locator:'#select',values:['b']});assert.equal(await browser.page.locator('#select').inputValue(),'b');
 await browser.invoke('waitFor',{text:'Done',locator:'#field',timeoutMs:1000});
 const snapshot=await browser.invoke('snapshot',{});assert(snapshot.accessibilityTree.includes('[ref=t3-'));assert(!JSON.stringify(snapshot).includes('private-do-not-leak'));assert(snapshot.screenshot.data.length>100);
 const ref=/\[ref=([^\]]+)\]/.exec(snapshot.accessibilityTree)[1];
 otherRelay=new TabCdpRelay({upstream,targetId:foreign,admitted:()=>admitted,WebSocketServer});otherBrowser=await attachPage({chromium,endpoint:await otherRelay.listen(),targetId:foreign,nativePage,nativeContracts,Schema,admitted:()=>admitted,artifactFs:artifacts});
 await assert.rejects(otherBrowser.invoke('click',{locator:'aria-ref='+ref,timeoutMs:100}),/stale|another tab/);await otherBrowser.close();otherBrowser=null;await otherRelay.close();otherRelay=null;
 browser.invalidateRefs();await assert.rejects(browser.invoke('click',{locator:'aria-ref='+ref,timeoutMs:100}),/stale|another tab/);
 const cdp=await browser.page.context().newCDPSession(browser.page);const before=upstream.calls.length;
 await assert.rejects(cdp.send('Target.attachToTarget',{targetId:foreign,flatten:true}));assert.equal(upstream.calls.length,before);
 await assert.rejects(cdp.send('Target.detachFromTarget',{sessionId:foreignSession}));assert.equal(upstream.calls.length,before);
 await assert.rejects(cdp.send('Browser.close'));await assert.rejects(cdp.send('Target.createBrowserContext'));await assert.rejects(cdp.send('Browser.setDownloadBehavior',{behavior:'allow',downloadPath:'/tmp/foreign'}));
 const result=await browser.invoke('evaluate',{expression:'({ok:true})',timeoutMs:1000});assert.deepEqual(result,{ok:true});
 await cdp.detach();const beforeRevoke=upstream.calls.length;admitted=false;
 await assert.rejects(browser.invoke('evaluate',{expression:'1'}),/revoked/);await assert.rejects(browser.page.evaluate('1'));assert.equal(upstream.calls.length,beforeRevoke);
 await browser.close();browser=null;await relay.close();relay=null;
 assert.equal((await upstream.send('Target.getTargetInfo',{targetId:foreign})).targetInfo.targetId,foreign);
 assert.equal((await upstream.send('Runtime.evaluate',{expression:'1',returnByValue:true},foreignSession)).result.value,1);
 assert(artifacts.healthy);assert(artifacts.receipts.some(r=>r.kind==='quarantined'));assert(artifacts.receipts.every(r=>r.executedRecursiveRemoval===false));
 fs.writeFileSync(out+'/proof.json',JSON.stringify({complete:true,privateRealChromium:true,wholeNativePage:true,scope:'Foundation only; no MCP catalog activation',foreignTargetRefused:true,foreignSessionRefused:true,foreignTargetSurvives:true,wrongTabRefRefused:true,revocationBeforeDispatch:true,nativeRecursiveCleanupRequests:artifacts.receipts.length,executedRecursiveRemovals:0,artifactReceipts:artifacts.receipts},null,2)+'\n');
}catch(error){failure=error;fs.writeFileSync(out+'/failure.txt',String(error.stack).replace(/wss?:\/\/[^\s)]+/g,'[owned-CDP]')+'\n');}
finally{
 if(otherBrowser)await otherBrowser.close().catch(()=>{});if(otherRelay)await otherRelay.close().catch(()=>{});
 if(browser)await browser.close().catch(()=>{});if(relay)await relay.close().catch(()=>{});upstream?.close();
 if(child.exitCode===null&&child.signalCode===null){child.kill('SIGTERM');await Promise.race([once(child,'close'),new Promise(resolve=>setTimeout(resolve,5000))]);if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await once(child,'close');}}
 fs.writeFileSync(out+'/artifact-receipts.json',JSON.stringify({healthy:artifacts.healthy,receipts:artifacts.receipts})+'\n');
}
if(failure)throw failure;console.log('REAL_NATIVE_PAGE_PASS');
