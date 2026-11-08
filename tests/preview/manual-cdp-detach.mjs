// Private physical regression for browser-parent CDP event ownership.
import fs from 'node:fs';
import path from 'node:path';
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
let worker,legacy,workerRelay,legacyRelay,upstream,borrowedSession,failure,stderr='',delayNextAttach=false,heldSession,releaseHeldAttach;
const observations={checks:[],detachEvents:{a:[],b:[],retiring:[]}};
const retain=()=>fs.writeFileSync(out+'/observations.json',JSON.stringify(observations,null,2)+'\n');
const extraRelays=[],releaseBarriers=new Set();
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
 borrowedSession=(await upstream.send('Target.attachToTarget',{targetId,flatten:true})).sessionId;
 const attach=async relay=>attachPage({chromium,endpoint:await relay.listen(),targetId,nativePage,nativeContracts,Schema,admitted:()=>true,artifactFs:artifacts});
 legacyRelay=new TabCdpRelay({upstream,targetId,admitted:()=>true,WebSocketServer});
 legacy=await attach(legacyRelay);
 await legacy.page.setContent('<input id="owned-input"><button id="owned-button" onclick="window.__ownedClicks=(window.__ownedClicks??0)+1">Owned UI</button>');
 await legacy.page.evaluate('window.__ownedMarker="retained-ui"');
 // Delay only one selected physical reply to exercise parent retirement. The
 // target/session and eventual detach still come from actual Chromium.
 const workerUpstream={
  on:(...args)=>upstream.on(...args),off:(...args)=>upstream.off(...args),
  async send(method,...args){
   const held=method==='Target.attachToTarget'&&delayNextAttach;
   if(held)delayNextAttach=false;
   const result=await upstream.send(method,...args);
   if(held){heldSession=result.sessionId;await new Promise(resolve=>{releaseHeldAttach=resolve;});}
   return result;
  },
 };
 workerRelay=new TabCdpRelay({upstream:workerUpstream,targetId,admitted:()=>true,WebSocketServer});
 worker=await attach(workerRelay);
 const usable=async label=>{
  assert.equal(worker.page.isClosed(),false);
  assert.equal(legacy.page.isClosed(),false);
  await worker.page.locator('#owned-input').fill(label);
  assert.equal(await legacy.page.locator('#owned-input').inputValue(),label);
  assert.equal(await worker.invoke('evaluate',{expression:'window.__ownedMarker'}),'retained-ui');
  assert.equal((await upstream.send('Runtime.evaluate',{expression:'document.querySelector("#owned-input").value',returnByValue:true},borrowedSession)).result.value,label);
  observations.checks.push({label,bothPlaywrightPagesUsable:true,borrowedSessionUsable:true});retain();
 };
 await usable('before-detach');
 const manual=await worker.page.context().newCDPSession(worker.page);
 assert.equal((await manual.send('Runtime.evaluate',{expression:'window.__ownedMarker',returnByValue:true})).result.value,'retained-ui');
 await manual.detach();
 await assert.rejects(manual.send('Runtime.evaluate',{expression:'1'}));
 await usable('after-page-session-detach');

 const browser=worker.page.context().browser();
 const a=await browser.newBrowserCDPSession();
 const b=await browser.newBrowserCDPSession();
 a.on('Target.detachedFromTarget',event=>{observations.detachEvents.a.push(event);retain();});
 b.on('Target.detachedFromTarget',event=>{observations.detachEvents.b.push(event);retain();});
 const sessionA=(await a.send('Target.attachToTarget',{targetId,flatten:true})).sessionId;
 const sessionB=(await b.send('Target.attachToTarget',{targetId,flatten:true})).sessionId;
 assert.notEqual(sessionA,sessionB);
 const detachCount=()=>upstream.calls.filter(call=>call.method==='Target.detachFromTarget').length;
 const beforeRefusals=detachCount();
 await assert.rejects(a.send('Target.detachFromTarget',{sessionId:sessionB}),/unavailable/);
 await assert.rejects(a.send('Target.detachFromTarget',{sessionId:borrowedSession}),/unavailable/);
 await assert.rejects(a.send('Target.detachFromTarget',{sessionId:'unowned-private-fixture'}),/unavailable/);
 assert.equal(detachCount(),beforeRefusals);
 assert.equal((await upstream.send('Runtime.evaluate',{expression:'window.__ownedMarker',returnByValue:true},sessionB)).result.value,'retained-ui');
 observations.foreignAndOtherParentDetachesRefusedBeforeUpstream=3;retain();
 const waitFor=async check=>{
  const deadline=performance.now()+2000;
  while(!check()&&performance.now()<deadline)await new Promise(resolve=>setTimeout(resolve,1));
  assert(check(),'Expected owned detach event was not delivered');
 };
 await a.send('Target.detachFromTarget',{sessionId:sessionA});
 await waitFor(()=>observations.detachEvents.a.length===1);
 assert.equal(observations.detachEvents.a[0].sessionId,sessionA);
 assert.equal(observations.detachEvents.b.length,0);
 assert.equal((await b.send('Target.getTargetInfo',{targetId})).targetInfo.targetId,targetId);
 await usable('after-parent-a-detach');
 await b.send('Target.detachFromTarget',{sessionId:sessionB});
 await waitFor(()=>observations.detachEvents.b.length===1);
 assert.equal(observations.detachEvents.b[0].sessionId,sessionB);
 assert.equal(observations.detachEvents.a.length,1);
 await usable('after-parent-b-detach');
 await a.detach();await b.detach();
 const retiring=await browser.newBrowserCDPSession();
 retiring.on('Target.detachedFromTarget',event=>{observations.detachEvents.retiring.push(event);retain();});
 const retiringChild=(await retiring.send('Target.attachToTarget',{targetId,flatten:true})).sessionId;
 await retiring.detach();
 await waitFor(()=>observations.detachEvents.retiring.length===1);
 assert.equal(observations.detachEvents.retiring.length,1);
 assert.equal(observations.detachEvents.retiring[0].sessionId,retiringChild);
 await assert.rejects(upstream.send('Runtime.evaluate',{expression:'1'},retiringChild));
 observations.retiringParentDetachesItsActivePhysicalChild=true;retain();
 await usable('after-active-parent-retirement');

 const pendingParent=await browser.newBrowserCDPSession();
 delayNextAttach=true;
 const pendingAttach=pendingParent.send('Target.attachToTarget',{targetId,flatten:true}).then(value=>({status:'fulfilled',value}),error=>({status:'rejected',message:error.message}));
 await waitFor(()=>heldSession!==undefined);
 let parentReturned=false;
 const pendingRetirement=pendingParent.detach().then(()=>{parentReturned=true;return {ok:true};},error=>({ok:false,message:error.message}));
 // Observe the actual closing gate through the protocol before releasing the
 // physical reply; an arbitrary sleep would not prove retirement had entered.
 let closingObserved=false;
 const closingDeadline=performance.now()+2000;
 while(!closingObserved&&performance.now()<closingDeadline){
  try{await pendingParent.send('Target.getTargetInfo',{targetId});}
  catch(error){assert.match(error.message,/unavailable|closed/);closingObserved=true;}
  if(!closingObserved)await new Promise(resolve=>setTimeout(resolve,1));
 }
 assert(closingObserved,'Parent closing gate was not observed');
 assert.equal(parentReturned,false);
 releaseHeldAttach();releaseHeldAttach=undefined;
 const attachResult=await pendingAttach;
 assert.equal(attachResult.status,'rejected');
 assert.match(attachResult.message,/unavailable|closed/);
 assert.deepEqual(await pendingRetirement,{ok:true});
 await assert.rejects(upstream.send('Runtime.evaluate',{expression:'1'},heldSession));
 observations.retiringParentWaitsForActualLateAttachmentReap=true;
 observations.parentClosingGateObservedBeforeActualReplyReleased=true;retain();
 await usable('after-pending-parent-retirement');

 // A raw private protocol peer observes exact CDP session ownership while the
 // two whole Playwright Pages continue using the same actual target.
 const rawPeer=async relay=>{
  const socket=new WebSocket(await relay.listen());await once(socket,'open');
  const pending=new Map(),events=[];let next=0;
  socket.on('message',data=>{const packet=JSON.parse(data.toString());if(packet.id&&pending.has(packet.id)){const resolve=pending.get(packet.id);pending.delete(packet.id);resolve(packet);}else if(!packet.id)events.push(packet);});
  socket.on('close',()=>{for(const resolve of pending.values())resolve({transportClosed:true});pending.clear();});
  return {events,call(method,params={},sessionId){return new Promise(resolve=>{const id=++next;pending.set(id,resolve);socket.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}));});}};
 };
 const barrier=()=>{
  let release;const waiting=new Promise(resolve=>{release=()=>{releaseBarriers.delete(release);resolve();};});
  releaseBarriers.add(release);return {waiting,release};
 };
 let familySession,lateChildSession,parentDetachHeld=false,childDetachHeld=false,childDetachAcknowledged=false,enteredDetachSession,enteredDetachReplyHeld=false;
 const parentBarrier=barrier(),childBarrier=barrier(),enteredDetachBarrier=barrier();
 const lateUpstream={
  on:(...args)=>upstream.on(...args),off:(...args)=>upstream.off(...args),
  async send(method,params,sessionId,...args){
   if(method==='Target.detachFromTarget'&&params.sessionId===familySession){parentDetachHeld=true;await parentBarrier.waiting;}
   if(method==='Target.detachFromTarget'&&params.sessionId===lateChildSession){childDetachHeld=true;await childBarrier.waiting;}
   const result=await upstream.send(method,params,sessionId,...args);
   if(method==='Target.detachFromTarget'&&params.sessionId===lateChildSession)childDetachAcknowledged=true;
   if(method==='Target.detachFromTarget'&&params.sessionId===enteredDetachSession){enteredDetachReplyHeld=true;await enteredDetachBarrier.waiting;}
   return result;
  },
 };
 const lateRelay=new TabCdpRelay({upstream:lateUpstream,targetId,admitted:()=>true,WebSocketServer});extraRelays.push(lateRelay);
 const latePeer=await rawPeer(lateRelay);
 const familyParent=(await latePeer.call('Target.attachToBrowserTarget')).result.sessionId;
 familySession=(await latePeer.call('Target.attachToTarget',{targetId,flatten:true},familyParent)).result.sessionId;
 const autoAttach=await latePeer.call('Target.setAutoAttach',{autoAttach:true,waitForDebuggerOnStart:false,flatten:true},familySession);assert(autoAttach.result);
 const observeLateChild=packet=>{if(packet.sessionId===familySession&&packet.method==='Target.attachedToTarget'&&packet.params.targetInfo.type==='worker')lateChildSession=packet.params.sessionId;};
 upstream.on('event',observeLateChild);
 let lateParentReturned=false;
 const lateRetirement=latePeer.call('Target.detachFromTarget',{sessionId:familyParent}).then(value=>{lateParentReturned=true;return value;});
 await waitFor(()=>parentDetachHeld);
 await upstream.send('Runtime.evaluate',{expression:'window.__lateOwnedWorker=new Worker(URL.createObjectURL(new Blob(["setInterval(()=>{},1000)"],{type:"text/javascript"}))); true',returnByValue:true},borrowedSession);
 await waitFor(()=>lateChildSession!==undefined&&childDetachHeld);
 assert.equal((await upstream.send('Runtime.evaluate',{expression:'1',returnByValue:true},lateChildSession)).result.value,1);
 const beforeAncestorRefusals=upstream.calls.filter(call=>call.method==='Runtime.evaluate'&&[familySession,lateChildSession].includes(call.sessionId)).length;
 assert((await latePeer.call('Runtime.evaluate',{expression:'1'},familySession)).error);
 assert((await latePeer.call('Runtime.evaluate',{expression:'1'},lateChildSession)).error);
 assert.equal(upstream.calls.filter(call=>call.method==='Runtime.evaluate'&&[familySession,lateChildSession].includes(call.sessionId)).length,beforeAncestorRefusals);
 assert.equal(lateParentReturned,false);
 await usable('while-late-child-retirement-held');
 childBarrier.release();await waitFor(()=>childDetachAcknowledged);
 await assert.rejects(upstream.send('Runtime.evaluate',{expression:'1'},lateChildSession));
 assert.equal(lateParentReturned,false);
 parentBarrier.release();assert((await lateRetirement).result);
 await assert.rejects(upstream.send('Runtime.evaluate',{expression:'1'},familySession));
 upstream.off('event',observeLateChild);
 await upstream.send('Runtime.evaluate',{expression:'window.__lateOwnedWorker.terminate(); delete window.__lateOwnedWorker; true',returnByValue:true},borrowedSession);

 const enteredParent=(await latePeer.call('Target.attachToBrowserTarget')).result.sessionId;
 enteredDetachSession=(await latePeer.call('Target.attachToTarget',{targetId,flatten:true},enteredParent)).result.sessionId;
 const enteredDetach=latePeer.call('Target.detachFromTarget',{sessionId:enteredDetachSession},enteredParent);
 await waitFor(()=>enteredDetachReplyHeld&&latePeer.events.some(packet=>packet.method==='Target.detachedFromTarget'&&packet.params.sessionId===enteredDetachSession));
 let enteredParentReturned=false;
 const enteredParentRetirement=latePeer.call('Target.detachFromTarget',{sessionId:enteredParent}).then(value=>{enteredParentReturned=true;return value;});
 let enteredClosingObserved=false;const enteredDeadline=performance.now()+2000;
 while(!enteredClosingObserved&&performance.now()<enteredDeadline){
  const result=await latePeer.call('Target.getTargetInfo',{targetId},enteredParent);
  if(result.error)enteredClosingObserved=true;else await new Promise(resolve=>setTimeout(resolve,1));
 }
 assert(enteredClosingObserved);assert.equal(enteredParentReturned,false);
 enteredDetachBarrier.release();assert((await enteredDetach).result);assert((await enteredParentRetirement).result);
 await assert.rejects(upstream.send('Runtime.evaluate',{expression:'1'},enteredDetachSession));
 observations.parentRetirementWaitsForEnteredDetachAfterItsNotification=true;retain();
 await lateRelay.close();assert.equal(lateRelay.cleanup.state,'complete');assert.equal(lateRelay.cleanup.pendingAttachments,0);
 observations.closingAncestorRejectsParentAndNewChildBeforeUpstream=true;
 observations.actualWorkerChildArrivingDuringDetachGetsIndependentCleanup=true;
 observations.lateChildCleanup=lateRelay.cleanup;retain();
 await usable('after-late-child-retirement');

 const globalAttachBarrier=barrier();let globalHeldSession;
 const globalUpstream={
  on:(...args)=>upstream.on(...args),off:(...args)=>upstream.off(...args),
  async send(method,...args){
   const result=await upstream.send(method,...args);
   if(method==='Target.attachToTarget'){globalHeldSession=result.sessionId;await globalAttachBarrier.waiting;}
   return result;
  },
 };
 const globalRelay=new TabCdpRelay({upstream:globalUpstream,targetId,admitted:()=>true,WebSocketServer});extraRelays.push(globalRelay);
 const globalPeer=await rawPeer(globalRelay);
 const globalParent=(await globalPeer.call('Target.attachToBrowserTarget')).result.sessionId;
 const globalAttach=globalPeer.call('Target.attachToTarget',{targetId,flatten:true},globalParent);
 await waitFor(()=>globalHeldSession!==undefined);
 let globalReturned=false;
 const globalClosing=globalRelay.close().then(()=>{globalReturned=true;return {ok:true};},error=>({ok:false,message:error.message}));
 await waitFor(()=>globalRelay.cleanup.state==='draining');
 assert.equal(globalRelay.cleanup.pendingAttachments,1);assert.equal(globalReturned,false);
 globalAttachBarrier.release();
 assert.deepEqual(await globalClosing,{ok:true});await globalAttach;
 await assert.rejects(upstream.send('Runtime.evaluate',{expression:'1'},globalHeldSession));
 assert.equal(globalRelay.cleanup.state,'complete');assert.equal(globalRelay.cleanup.pendingAttachments,0);
 observations.globalCloseWaitsForEnteredPhysicalAttachAndReap=true;retain();

 // Schedule close after #info validates its real reply, before the awaiting
 // auto-attach continuation. The accessor is an explicit scheduling fixture;
 // all identity data still comes from Chromium and no attach may reach it.
 let gapRelay,gapClose,gapQueued=false,gapAttachWrites=0;
 const gapUpstream={
  on:(...args)=>upstream.on(...args),off:(...args)=>upstream.off(...args),
  async send(method,...args){
   if(method==='Target.attachToTarget')gapAttachWrites++;
   const result=await upstream.send(method,...args);
   if(method!=='Target.getTargetInfo')return result;
   const info=result.targetInfo;
   return Object.defineProperty({...result},'targetInfo',{enumerable:true,get(){if(!gapQueued){gapQueued=true;queueMicrotask(()=>{gapClose=gapRelay.close().then(()=>({ok:true}),error=>({ok:false,message:error.message}));});}return info;}});
  },
 };
 gapRelay=new TabCdpRelay({upstream:gapUpstream,targetId,admitted:()=>true,WebSocketServer});extraRelays.push(gapRelay);
 const gapPeer=await rawPeer(gapRelay);
 await gapPeer.call('Target.setAutoAttach',{autoAttach:true,flatten:true});
 await waitFor(()=>gapClose!==undefined);assert.deepEqual(await gapClose,{ok:true});
 assert.equal(gapAttachWrites,0);assert.equal(gapRelay.cleanup.state,'complete');
 observations.closeBetweenInfoAndAttachStartsNoUpstreamAttachment=true;retain();
 await legacy.page.locator('#owned-button').click();
 assert.equal(await worker.invoke('evaluate',{expression:'window.__ownedClicks'}),1);
 await worker.close();worker=undefined;
 await workerRelay.close();
 const workerCleanup=workerRelay.cleanup;
 assert.equal(workerCleanup.state,'complete');assert.equal(workerCleanup.pendingAttachments,0);
 assert(workerCleanup.receipts.some(receipt=>receipt.kind==='detached-owned-session'));
 assert.equal(legacy.page.isClosed(),false);
 assert.equal(await legacy.invoke('evaluate',{expression:'window.__ownedMarker'}),'retained-ui');
 await legacy.page.locator('#owned-input').fill('after-worker-retirement');
 assert.equal((await upstream.send('Runtime.evaluate',{expression:'document.querySelector("#owned-input").value',returnByValue:true},borrowedSession)).result.value,'after-worker-retirement');
 await legacy.close();legacy=undefined;await legacyRelay.close();
 const legacyCleanup=legacyRelay.cleanup;
 assert.equal(legacyCleanup.state,'complete');assert.equal(legacyCleanup.pendingAttachments,0);
 assert.equal((await upstream.send('Target.getTargetInfo',{targetId})).targetInfo.targetId,targetId);
 assert.equal((await upstream.send('Runtime.evaluate',{expression:'window.__ownedMarker',returnByValue:true},borrowedSession)).result.value,'retained-ui');
 assert(artifacts.healthy);
 assert(artifacts.receipts.every(receipt=>receipt.executedRecursiveRemoval===false));
 fs.writeFileSync(out+'/proof.json',JSON.stringify({complete:true,privateRealChromium:true,pinnedPlaywright:'1.60.0',wholeNativePage:true,manualPageCdpDetachPreservesPage:true,twoVirtualParentsRouteOnlyTheirOwnDetach:true,foreignAndOtherParentDetachesRefusedBeforeUpstream:3,retiringParentDetachesItsActivePhysicalChild:true,retiringParentWaitsForActualLateAttachmentReap:true,closingAncestorRejectsParentAndNewChildBeforeUpstream:true,actualWorkerChildArrivingDuringDetachGetsIndependentCleanup:true,parentRetirementWaitsForEnteredDetachAfterItsNotification:true,globalCloseWaitsForEnteredPhysicalAttachAndReap:true,closeBetweenInfoAndAttachStartsNoUpstreamAttachment:true,twoPlaywrightPagesRemainUsable:true,borrowedLegacySessionAndPhysicalTargetSurviveBothRelays:true,workerCleanup,legacyCleanup,executedRecursiveRemovals:0,qualification:'Private physical relay/manual CDP lifecycle regression with controlled actual attachment/detach barriers, an actual blob Worker child and an explicit accessor scheduling fixture around a real target-info reply. No Rust factory, source caller, public MCP, real GUI, default-timeout rerun or production installation acceptance'},null,2)+'\n');
}catch(error){
 failure=error;retain();
 fs.writeFileSync(out+'/failure.txt',String(error.stack).replace(/wss?:\/\/[^\s)]+/g,'[owned-CDP]')+'\n');
}finally{
 releaseHeldAttach?.();
 for(const release of releaseBarriers)release();
 for(const relay of extraRelays)await relay.close().catch(()=>{});
 if(worker)await worker.close().catch(()=>{});
 if(workerRelay)await workerRelay.close().catch(()=>{});
 if(legacy)await legacy.close().catch(()=>{});
 if(legacyRelay)await legacyRelay.close().catch(()=>{});
 if(borrowedSession&&upstream)await upstream.send('Target.detachFromTarget',{sessionId:borrowedSession}).catch(()=>{});
 upstream?.close();
 if(child.exitCode===null&&child.signalCode===null){
  child.kill('SIGTERM');
  await Promise.race([once(child,'close'),new Promise(resolve=>setTimeout(resolve,5000))]);
  if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await once(child,'close');}
 }
 fs.writeFileSync(out+'/artifact-receipts.json',JSON.stringify({healthy:artifacts.healthy,receipts:artifacts.receipts,executedRecursiveRemoval:artifacts.receipts.some(receipt=>receipt.executedRecursiveRemoval)})+'\n');
}
if(failure)throw failure;
console.log('MANUAL_CDP_DETACH_PASS');
