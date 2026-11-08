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
let stderr='';let browser;let relay;let upstream;let endpoint;let failure;let outstanding=0,maximumOutstanding=0;const records=[];
try{
 endpoint=await new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('Chromium startup deadline')),20000);child.stderr.on('data',data=>{stderr=(stderr+data).slice(-4096);const found=/DevTools listening on (ws:\/\/127\.0\.0\.1:[^\s]+)/.exec(stderr);if(found){clearTimeout(timer);resolve(found[1]);}});child.once('exit',()=>{clearTimeout(timer);reject(Error('Owned Chromium stopped'));});});
 const socket=new WebSocket(endpoint);await once(socket,'open');upstream=new FixtureCdp(socket);
 const targetId=(await upstream.send('Target.createTarget',{url:'about:blank'})).targetId;
 const send=upstream.send.bind(upstream);
 upstream.send=(method,params={},session)=>{
  const tracked=method==='Runtime.evaluate'||method==='Runtime.terminateExecution';
  if(tracked){outstanding++;maximumOutstanding=Math.max(maximumOutstanding,outstanding);records.push({method,event:'wire-send',outstanding});}
  return send(method,params,session).finally(()=>{if(tracked){outstanding--;records.push({method,event:'wire-settled',outstanding});}});
 };
 relay=new TabCdpRelay({upstream,targetId,admitted:()=>true,WebSocketServer});
 browser=await attachPage({chromium,endpoint:await relay.listen(),targetId,nativePage,nativeContracts,Schema,admitted:()=>true,artifactFs:artifacts});
 const cdp=await browser.page.context().newCDPSession(browser.page);
 const background=[];
 for(let index=0;index<7;index++)background.push(cdp.send('Runtime.evaluate',{expression:'new Promise(() => {})',awaitPromise:true,returnByValue:true}).catch(()=>{}));
 const waitUntil=async predicate=>{const deadline=performance.now()+2000;while(!predicate()){assert(performance.now()<deadline,'Expected wire admission');await new Promise(resolve=>setTimeout(resolve,5));}};
 await waitUntil(()=>outstanding===7);
 const {SessionControl}=await import('/native/apps/server/src/preview/SessionControl.ts');
 const control=new SessionControl('private-agent');
 await assert.rejects(control.agent('private-agent',()=>nativePage.evaluate(cdp,{expression:'new Promise(() => {})',awaitPromise:true,returnByValue:true},100)),error=>error.tag==='PreviewAutomationTimeoutError');
 await waitUntil(()=>records.some(record=>record.method==='Runtime.terminateExecution'&&record.event==='wire-send'));
 const terminated=records.find(record=>record.method==='Runtime.terminateExecution'&&record.event==='wire-send');
 assert.equal(terminated.outstanding,9);assert.equal(maximumOutstanding,9);
 await control.agent('private-agent',async()=>assert.equal(await browser.page.evaluate('1'),1));
 await control.close();await cdp.detach();
 await browser.close();browser=null;await relay.close();relay=null;
 assert(artifacts.healthy);assert(artifacts.receipts.every(receipt=>receipt.executedRecursiveRemoval===false));
 assert.equal(fs.readdirSync(out+'/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);
 fs.writeFileSync(out+'/proof.json',JSON.stringify({complete:true,privateRealChromium:true,wholeNativeEvaluate:true,wholeNativeSessionControl:true,pinnedPlaywright:true,backgroundDeferredCdpCalls:7,maximumRealWireOutstanding:maximumOutstanding,records,nativeRecursiveCleanupRequests:artifacts.receipts.length,executedRecursiveRemovals:0,qualification:'Seven explicit deferred CDP background producers plus one original native evaluation. Proves actual Playwright/CDP transport admits its timeout termination as a ninth real wire call; does not assert nine concurrent MCP tool invocations or activate the O3 gateway.'},null,2)+'\n');
}catch(error){failure=error;fs.writeFileSync(out+'/failure.txt',String(error.stack).replace(/wss?:\/\/[^\s)]+/g,'[owned-CDP]')+'\n');}
finally{
 if(browser)await browser.close().catch(()=>{});if(relay)await relay.close().catch(()=>{});upstream?.close();
 if(child.exitCode===null&&child.signalCode===null){child.kill('SIGTERM');await Promise.race([once(child,'close'),new Promise(resolve=>setTimeout(resolve,5000))]);if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await once(child,'close');}}
 fs.writeFileSync(out+'/artifact-receipts.json',JSON.stringify({healthy:artifacts.healthy,receipts:artifacts.receipts})+'\n');
}
if(failure)throw failure;console.log('REAL_NATIVE_EVALUATION_CAPACITY_PASS');
