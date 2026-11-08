// Private real Chromium/Playwright defaults, not a mock Page or settings readback.
import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createRequire} from 'node:module';
import {RetainingArtifactFs} from '../../runtime/preview/retaining-artifact-fs.mjs';
import {TabCdpRelay} from '../../runtime/preview/tab-cdp-relay.mjs';
import {attachPage} from '../../runtime/preview/page-worker.mjs';
import {FixtureCdp} from './fixture-cdp.mjs';

assert(fs.existsSync('/run/.containerenv'));
const out='/out';
for(const name of ['tmp','quarantine','profile','guard'])fs.mkdirSync(path.join(out,name),{mode:0o700});
process.env.TMPDIR=out+'/tmp';
process.env.O3_RECURSIVE_FS_GUARD_DIR=out+'/guard';
await import('/guard.mjs');
const artifacts=new RetainingArtifactFs({tempRoot:out+'/tmp',quarantineRoot:out+'/quarantine'});
artifacts.install();
const require=createRequire('/playwright/package.json');
const {chromium}=require('/playwright');
const {ws:WebSocket,wsServer:WebSocketServer}=require('/playwright/lib/utilsBundle.js');
const nativePage=await import('/native/apps/server/src/preview/ServerBrowserPage.ts');
const nativeContracts=await import('/native/packages/contracts/src/previewAutomation.ts');
const Schema=await import('/deps/node_modules/effect/dist/Schema.js');
const child=spawn('/usr/bin/chromium',['--headless','--no-sandbox','--disable-gpu','--no-first-run','--disable-background-networking','--disable-dev-shm-usage','--remote-debugging-port=0','--user-data-dir='+out+'/profile','about:blank'],{stdio:['ignore','ignore','pipe']});
let browser,relay,upstream,server,comparisonCdp,failure,stderr='';
const sockets=new Set();
const observations={evaluatePairs:[],timeoutCases:[]};
const retainObservations=()=>fs.writeFileSync(out+'/observations.json',JSON.stringify(observations,null,2)+'\n');
try{
 const endpoint=await new Promise((resolve,reject)=>{
  const timer=setTimeout(()=>reject(Error('Chromium startup deadline')),20000);
  child.stderr.on('data',data=>{
   stderr=(stderr+data).slice(-4096);
   const found=/DevTools listening on (ws:\/\/127\.0\.0\.1:[^\s]+)/.exec(stderr);
   if(found){clearTimeout(timer);resolve(found[1]);}
  });
  child.once('exit',()=>{clearTimeout(timer);reject(Error('Owned Chromium stopped'));});
 });
 const socket=new WebSocket(endpoint);await once(socket,'open');upstream=new FixtureCdp(socket);
 const targetId=(await upstream.send('Target.createTarget',{url:'about:blank'})).targetId;
 relay=new TabCdpRelay({upstream,targetId,admitted:()=>true,WebSocketServer});
 browser=await attachPage({chromium,endpoint:await relay.listen(),targetId,nativePage,nativeContracts,Schema,admitted:()=>true,artifactFs:artifacts});
 // Compare the complete pinned Page implementation with the actual adapter.
 // Record the observed type/value, including the entry's nullish projection;
 // do not manufacture an undefined expectation before CDP has answered.
 const evaluatePairs=observations.evaluatePairs;
 const describe=value=>({type:typeof value,isNull:value===null,isUndefined:value===undefined,...(value===undefined?{}:{value})});
 comparisonCdp=await browser.page.context().newCDPSession(browser.page);
 for(const expression of ['undefined','void 0','null']){
   const input=Schema.decodeUnknownSync(nativeContracts.PreviewAutomationEvaluateInput)({expression});
   const original=await nativePage.evaluate(comparisonCdp,input,15000);
   const wrapper=await browser.invoke('evaluate',{expression},15000);
   evaluatePairs.push({expression,original:describe(original),wrapper:describe(wrapper),workerEntryNullishProjection:describe(wrapper??null),equal:Object.is(wrapper,original)});
   retainObservations();
 }
 // The private server intentionally sends no headers. It cannot acquire an
 // external destination, profile or credential in this network-none container.
 server=http.createServer(()=>{});
 server.on('connection',socket=>{sockets.add(socket);socket.once('close',()=>sockets.delete(socket));});
 server.listen(0,'127.0.0.1');await once(server,'listening');
 const started=performance.now();
 // Run the cases sequentially so navigation cannot invalidate the first
 // locator's execution context. Retain raw failures before asserting either.
 for(const [name,call] of [
  ['locator',()=>browser.page.locator('#never-present-private-fixture').waitFor()],
  ['navigation',()=>browser.page.goto(`http://127.0.0.1:${server.address().port}/owned-stalled-navigation`)],
 ]){
  const caseStarted=performance.now();
  const [result]=await Promise.allSettled([call()]);
  observations.timeoutCases.push({name,elapsedMs:performance.now()-caseStarted,status:result.status,...(result.status==='rejected'?{error:{name:result.reason.name,message:result.reason.message}}:{fulfilledValueType:typeof result.value})});
  retainObservations();
 }
 const elapsedMs=performance.now()-started;
 const results=observations.timeoutCases;
 for(const pair of evaluatePairs)assert.equal(pair.equal,true);
 for(const result of results){
  assert.equal(result.status,'rejected');
  assert.equal(result.error.name,'TimeoutError');
  assert.match(result.error.message,/Timeout 15000ms exceeded/);
  assert(result.elapsedMs>=14000&&result.elapsedMs<60000);
 }
 assert(artifacts.healthy);
 fs.writeFileSync(out+'/proof.json',JSON.stringify({complete:true,privateRealChromium:true,pinnedPlaywright:'1.60.0',defaultActionTimeoutMs:15000,defaultNavigationTimeoutMs:15000,explicitPerCallTimeouts:false,sequentialTimeoutCases:true,elapsedMs,timeoutCases:results,evaluatePairs,qualification:'Actual Page defaults and three whole-original evaluate/adapter pairs; nullish entry projection is recorded separately, not a full worker invocation. No Rust factory, source Control, caller, snapshot metadata or full twelve-tool acceptance'},null,2)+'\n');
}catch(error){
 failure=error;
 fs.writeFileSync(out+'/failure.txt',String(error.stack).replace(/wss?:\/\/[^\s)]+/g,'[owned-CDP]')+'\n');
}finally{
 for(const socket of sockets)socket.destroy();
 if(server?.listening)await new Promise(resolve=>server.close(resolve));
 if(comparisonCdp)await comparisonCdp.detach().catch(()=>{});
 if(browser)await browser.close().catch(()=>{});
 if(relay)await relay.close().catch(()=>{});
 upstream?.close();
 if(child.exitCode===null&&child.signalCode===null){
  child.kill('SIGTERM');
  await Promise.race([once(child,'close'),new Promise(resolve=>setTimeout(resolve,5000))]);
  if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await once(child,'close');}
 }
 fs.writeFileSync(out+'/artifact-receipts.json',JSON.stringify({healthy:artifacts.healthy,receipts:artifacts.receipts,executedRecursiveRemoval:artifacts.receipts.some(receipt=>receipt.executedRecursiveRemoval)})+'\n');
}
if(failure)throw failure;
console.log('NATIVE_PAGE_DEFAULTS_PASS');
