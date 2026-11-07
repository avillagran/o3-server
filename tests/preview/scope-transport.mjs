// Transparent observation of public ws.send on the exact private Page endpoint.
// This proves correlation only; it does not mint an authorization capability.
import fs from 'node:fs';import path from 'node:path';import assert from 'node:assert/strict';import {spawn} from 'node:child_process';import {once} from 'node:events';import {createRequire} from 'node:module';import {AsyncLocalStorage} from 'node:async_hooks';
import {RetainingArtifactFs} from '../../runtime/preview/retaining-artifact-fs.mjs';import {TabCdpRelay} from '../../runtime/preview/tab-cdp-relay.mjs';import {attachPage} from '../../runtime/preview/page-worker.mjs';import {FixtureCdp} from './fixture-cdp.mjs';import {OwnerScopeTransport} from '../../runtime/preview/owner-scope-transport.mjs';
assert(fs.existsSync('/run/.containerenv'));for(const name of ['tmp','quarantine','profile','guard'])fs.mkdirSync('/out/'+name,{mode:0o700});process.env.TMPDIR='/out/tmp';process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const artifacts=new RetainingArtifactFs({tempRoot:'/out/tmp',quarantineRoot:'/out/quarantine'});artifacts.install();
const require=createRequire('/playwright/package.json'),{chromium}=require('/playwright'),{ws:WebSocket,wsServer:WebSocketServer}=require('/playwright/lib/utilsBundle.js');
const nativePage=await import('/native/apps/server/src/preview/ServerBrowserPage.ts'),nativeContracts=await import('/native/packages/contracts/src/previewAutomation.ts'),Schema=await import('/deps/node_modules/effect/dist/Schema.js');
const descriptor=Object.getOwnPropertyDescriptor(WebSocket.prototype,'send');
const issued=new Map(),trace=[];let adapter,faults=0,deniedLate=0,sequence=0;
function issue(kind){const ticket=(++sequence).toString(16).padStart(48,'0');const scope=adapter.admit(ticket);issued.set(ticket,{kind,active:true});return {scope,ticket};}
async function scoped(kind,work){const admitted=issue(kind);try{return await adapter.run(admitted.scope,work);}finally{adapter.finish(admitted.scope);issued.get(admitted.ticket).active=false;}}
const child=spawn('/usr/bin/chromium',['--headless','--no-sandbox','--disable-gpu','--no-first-run','--disable-background-networking','--disable-dev-shm-usage','--remote-debugging-port=0','--user-data-dir=/out/profile','about:blank'],{stdio:['ignore','ignore','pipe']});let browser,relay,upstream,failure;
let bootstrap,cleanup;
try{
 const endpoint=await new Promise((resolve,reject)=>{let text='';const timer=setTimeout(()=>reject(Error('Private Chromium startup deadline')),20000);child.stderr.on('data',chunk=>{text=(text+chunk).slice(-4096);const match=/DevTools listening on (ws:\/\/127\.0\.0\.1:[^\s]+)/.exec(text);if(match){clearTimeout(timer);resolve(match[1]);}});child.once('exit',()=>{clearTimeout(timer);reject(Error('Private Chromium stopped'));});});
 const socket=new WebSocket(endpoint);await once(socket,'open');upstream=new FixtureCdp(socket);const targetId=(await upstream.send('Target.createTarget',{url:'about:blank'})).targetId;
 const scopedUpstream={on:(...args)=>upstream.on(...args),off:(...args)=>upstream.off(...args),send(method,params,sessionId,ticket){
  const receipt=issued.get(ticket);assert(receipt,'Unknown owner ticket');
  if(!receipt.active&&!['Target.detachFromTarget','Runtime.releaseObject','Runtime.releaseObjectGroup','Runtime.terminateExecution'].includes(method)){deniedLate++;return Promise.reject(Error('Inactive owner receipt'));}
  trace.push({kind:receipt.kind,method});return upstream.send(method,params,sessionId);
 }};
 const cleanupTicket=(++sequence).toString(16).padStart(48,'0');issued.set(cleanupTicket,{kind:'cleanup',active:true});
 relay=new TabCdpRelay({upstream:scopedUpstream,targetId,admitted:()=>true,WebSocketServer,requireReceipt:true,cleanupReceipt:cleanupTicket});const endpointScoped=await relay.listen();
 adapter=new OwnerScopeTransport({WebSocket,endpoint:endpointScoped,onFault(){faults++;throw Error('Injected diagnostic callback failure');}});
 cleanup={scope:adapter.admit(cleanupTicket),ticket:cleanupTicket};bootstrap=issue('bootstrap');
 browser=await adapter.run(bootstrap.scope,()=>attachPage({chromium,endpoint:endpointScoped,targetId,nativePage,nativeContracts,Schema,admitted:()=>true,artifactFs:artifacts}));
 await scoped('setup',()=>browser.invoke('evaluate',{expression:"document.body.innerHTML='<input id=field><button id=button>Click</button>';true"}));
 await scoped('evaluate',()=>browser.invoke('evaluate',{expression:'({actual:true})'}));
 await scoped('press',()=>browser.invoke('press',{key:'End'}));
 await scoped('type',()=>browser.invoke('type',{locator:'#field',text:'private',clear:true}));
 await scoped('click',()=>browser.invoke('click',{locator:'#button'}));
 await scoped('snapshot',()=>browser.invoke('snapshot',{}));
 const waiting=scoped('waitFor',()=>browser.invoke('waitFor',{text:'private-marker',timeoutMs:1500}));
 await Promise.all([scoped('concurrent-snapshot',()=>browser.invoke('snapshot',{})),scoped('release-marker',()=>browser.invoke('evaluate',{expression:"document.body.append(document.createTextNode('private-marker'));true"})),waiting]);
 const old=issue('retired-callback'),late=Promise.withResolvers();
 adapter.run(old.scope,()=>{setTimeout(()=>{void browser.page.evaluate('globalThis.__lateScopeMutation=true').then(()=>late.resolve('applied'),()=>late.resolve('refused'));},40);});
 adapter.finish(old.scope);issued.get(old.ticket).active=false;
 assert.equal(await scoped('successor',()=>browser.invoke('evaluate',{expression:"new Promise(resolve=>setTimeout(()=>resolve('successor'),100))"})),'successor');
 assert.equal(await late.promise,'refused');assert.equal(deniedLate,1);
 assert.equal(await scoped('late-check',()=>browser.invoke('evaluate',{expression:'typeof globalThis.__lateScopeMutation'})),'undefined');
 let timeout;try{await scoped('native-timeout',()=>browser.invoke('evaluate',{expression:'new Promise(resolve=>setTimeout(()=>resolve(true),150))'},20));}catch(error){timeout=error.tag;}
 assert.equal(timeout,'PreviewAutomationTimeoutError');
 assert.equal(await scoped('after-timeout',()=>browser.invoke('evaluate',{expression:'true'})),true);
 const unicode=await scoped('unicode',()=>browser.invoke('evaluate',{expression:"({[String.fromCharCode(0xd800)]:String.fromCharCode(0xdfff)})"}));assert.equal(unicode['\ud800'],'\udfff');
 const before=upstream.calls.length;await assert.rejects(browser.invoke('evaluate',{expression:'globalThis.__unscopedMutation=true'}));assert.equal(faults,1);assert.equal(upstream.calls.length,before);
 await adapter.run(cleanup.scope,()=>browser.close());browser=null;await relay.close();relay=null;
 const summary={};for(const item of trace){(summary[item.kind]??=[]).push(item.method);}
 for(const kind of ['bootstrap','setup','evaluate','press','type','click','snapshot','waitFor','concurrent-snapshot','release-marker','successor','late-check','native-timeout','after-timeout','unicode','cleanup'])assert(summary[kind]?.length,`No actual frames for ${kind}`);
 assert(summary['native-timeout'].includes('Runtime.terminateExecution'));assert(summary.press.includes('Input.dispatchKeyEvent'));
 adapter.finish(bootstrap.scope);adapter.finish(cleanup.scope);adapter.dispose();assert.equal(Object.getOwnPropertyDescriptor(WebSocket.prototype,'send').value,descriptor.value);
 assert(artifacts.healthy);assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);
 fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,actualPinnedPlaywright:true,wholeNativePage:true,taggedOutsideCdpParams:true,retiredCallbackNotRelabeledAsSuccessor:true,lateMutationRefusedBeforeWire:true,unscopedMutationRefusedBeforeWire:true,faultCallbackFailureCannotBypassFence:true,nativeTimeoutRetainsItsContext:true,exactIsolatedUtf16:true,originalSendRestored:true,summary,nativeRecursiveCleanupRequests:artifacts.receipts.length,executedRecursiveRemovals:0,qualification:'Synthetic owner ticket issuer validates the private transport boundary; actual Rust authority/broker binding remains inactive.'},null,2)+'\n');
}catch(error){failure=error;fs.writeFileSync('/out/failure.txt',String(error.stack).replace(/wss?:\/\/[^\s)]+/g,'[owned-CDP]')+'\n');}
finally{
 if(browser)await browser.close().catch(()=>{});if(relay)await relay.close().catch(()=>{});upstream?.close();Object.defineProperty(WebSocket.prototype,'send',descriptor);
 if(child.exitCode===null&&child.signalCode===null){child.kill('SIGTERM');await Promise.race([once(child,'close'),new Promise(resolve=>setTimeout(resolve,5000))]);if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await once(child,'close');}}
}
if(failure)throw failure;console.log('NATIVE_PAGE_SCOPE_TRANSPORT_PASS');
