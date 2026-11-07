// Transparent observation of public ws.send on the exact private Page endpoint.
// This proves correlation only; it does not mint an authorization capability.
import fs from 'node:fs';import path from 'node:path';import assert from 'node:assert/strict';import {spawn} from 'node:child_process';import {once} from 'node:events';import {createRequire} from 'node:module';import {AsyncLocalStorage} from 'node:async_hooks';
import {RetainingArtifactFs} from '../../runtime/preview/retaining-artifact-fs.mjs';import {TabCdpRelay} from '../../runtime/preview/tab-cdp-relay.mjs';import {attachPage} from '../../runtime/preview/page-worker.mjs';import {FixtureCdp} from './fixture-cdp.mjs';
assert(fs.existsSync('/run/.containerenv'));for(const name of ['tmp','quarantine','profile','guard'])fs.mkdirSync('/out/'+name,{mode:0o700});process.env.TMPDIR='/out/tmp';process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const artifacts=new RetainingArtifactFs({tempRoot:'/out/tmp',quarantineRoot:'/out/quarantine'});artifacts.install();
const require=createRequire('/playwright/package.json'),{chromium}=require('/playwright'),{ws:WebSocket,wsServer:WebSocketServer}=require('/playwright/lib/utilsBundle.js');
const nativePage=await import('/native/apps/server/src/preview/ServerBrowserPage.ts'),nativeContracts=await import('/native/packages/contracts/src/previewAutomation.ts'),Schema=await import('/deps/node_modules/effect/dist/Schema.js');
const storage=new AsyncLocalStorage(),trace=[];let invalid=false,scoped;const descriptor=Object.getOwnPropertyDescriptor(WebSocket.prototype,'send');
const observed={send(data,options,callback){
 try{if(this.url===scoped){const value=JSON.parse(typeof data==='string'?data:data.toString());if(trace.length>=1024)invalid=true;else trace.push({method:value.method,kind:storage.getStore()?.kind??null});}}catch{invalid=true;}
 return Reflect.apply(descriptor.value,this,arguments);
}}.send;
Object.defineProperty(WebSocket.prototype,'send',{...descriptor,value:observed});
const child=spawn('/usr/bin/chromium',['--headless','--no-sandbox','--disable-gpu','--no-first-run','--disable-background-networking','--disable-dev-shm-usage','--remote-debugging-port=0','--user-data-dir=/out/profile','about:blank'],{stdio:['ignore','ignore','pipe']});let browser,relay,upstream,failure;
const run=(kind,work)=>storage.run({kind},work);
try{
 const endpoint=await new Promise((resolve,reject)=>{let text='';const timer=setTimeout(()=>reject(Error('Private Chromium startup deadline')),20000);child.stderr.on('data',chunk=>{text=(text+chunk).slice(-4096);const match=/DevTools listening on (ws:\/\/127\.0\.0\.1:[^\s]+)/.exec(text);if(match){clearTimeout(timer);resolve(match[1]);}});child.once('exit',()=>{clearTimeout(timer);reject(Error('Private Chromium stopped'));});});
 const socket=new WebSocket(endpoint);await once(socket,'open');upstream=new FixtureCdp(socket);const targetId=(await upstream.send('Target.createTarget',{url:'about:blank'})).targetId;
 relay=new TabCdpRelay({upstream,targetId,admitted:()=>true,WebSocketServer});scoped=await relay.listen();
 browser=await run('bootstrap',()=>attachPage({chromium,endpoint:scoped,targetId,nativePage,nativeContracts,Schema,admitted:()=>true,artifactFs:artifacts}));
 await run('setup',()=>browser.invoke('evaluate',{expression:"document.body.innerHTML='<input id=field><button id=button>Click</button>';true"}));
 await run('evaluate',()=>browser.invoke('evaluate',{expression:'({actual:true})'}));
 await run('press',()=>browser.invoke('press',{key:'End'}));
 await run('type',()=>browser.invoke('type',{locator:'#field',text:'private',clear:true}));
 await run('click',()=>browser.invoke('click',{locator:'#button'}));
 await run('snapshot',()=>browser.invoke('snapshot',{}));
 const waiting=run('waitFor',()=>browser.invoke('waitFor',{text:'private-marker',timeoutMs:1500}));
 await Promise.all([run('concurrent-snapshot',()=>browser.invoke('snapshot',{})),run('release-marker',()=>browser.invoke('evaluate',{expression:"document.body.append(document.createTextNode('private-marker'));true"})),waiting]);
 await run('close',()=>browser.close());browser=null;await relay.close();relay=null;
 assert(!invalid);assert(trace.length>20);const summary={};for(const item of trace){(summary[item.kind??'missing']??=[]).push(item.method);}
 for(const kind of ['bootstrap','setup','evaluate','press','type','click','snapshot','waitFor','concurrent-snapshot','release-marker','close'])assert(summary[kind]?.length,`No observed frames for ${kind}`);
 assert(summary.evaluate.includes('Runtime.evaluate'));assert(summary.press.includes('Input.dispatchKeyEvent'));assert(!summary.missing);
 assert(artifacts.healthy);assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);
 fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,actualPinnedPlaywright:true,wholeNativePage:true,observerOnly:true,contextReachedExactEndpoint:true,summary,nativeRecursiveCleanupRequests:artifacts.receipts.length,executedRecursiveRemovals:0,qualification:'Correlation observation only. No authority or production ws adapter is activated.'},null,2)+'\n');
}catch(error){failure=error;fs.writeFileSync('/out/failure.txt',String(error.stack).replace(/wss?:\/\/[^\s)]+/g,'[owned-CDP]')+'\n');}
finally{
 if(browser)await browser.close().catch(()=>{});if(relay)await relay.close().catch(()=>{});upstream?.close();Object.defineProperty(WebSocket.prototype,'send',descriptor);
 if(child.exitCode===null&&child.signalCode===null){child.kill('SIGTERM');await Promise.race([once(child,'close'),new Promise(resolve=>setTimeout(resolve,5000))]);if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await once(child,'close');}}
}
if(failure)throw failure;console.log('NATIVE_PAGE_SCOPE_CONTEXT_PASS');
