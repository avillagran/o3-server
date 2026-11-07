import fs from 'node:fs';import assert from 'node:assert/strict';import {EventEmitter,once} from 'node:events';import {createRequire} from 'node:module';import {TabCdpRelay} from '../../runtime/preview/tab-cdp-relay.mjs';
assert(fs.existsSync('/run/.containerenv'));fs.mkdirSync('/out/guard',{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');const require=createRequire('/playwright/package.json');const {ws:WebSocket,wsServer:WebSocketServer}=require('/playwright/lib/utilsBundle.js');
class Upstream extends EventEmitter{calls=[];resolveAttach;async send(method,params,sessionId){this.calls.push({method,params,sessionId});if(method==='Target.attachToTarget')return new Promise(resolve=>{this.resolveAttach=resolve;});if(method==='Target.getTargetInfo')return {targetInfo:{targetId:'owned',type:'page',browserContextId:'context',url:'about:blank'}};return {};}}
const upstream=new Upstream();let admitted=true;const relay=new TabCdpRelay({upstream,targetId:'owned',admitted:()=>admitted,WebSocketServer});const endpoint=await relay.listen();const client=new WebSocket(endpoint);await once(client,'open');let id=0;
function call(method,params={},sessionId){const key=++id;return new Promise((resolve,reject)=>{const listener=data=>{const r=JSON.parse(data);if(r.id===key){client.off('message',listener);resolve(r);}};client.on('message',listener);client.send(JSON.stringify({id:key,method,params,...(sessionId?{sessionId}:{})}));});}
const events=[];client.on('message',data=>{const r=JSON.parse(data);if(!r.id)events.push(r);});
const virtual=(await call('Target.attachToBrowserTarget')).result.sessionId;assert.equal(upstream.calls.length,0);
for(const [method,params,session]of [['Target.attachToTarget',{targetId:'foreign',flatten:true}],['Target.getTargetInfo',{targetId:'foreign'}],['Target.detachFromTarget',{sessionId:'foreign'}],['Runtime.evaluate',{expression:'1'},'foreign'],['Browser.close',{},virtual],['Target.createBrowserContext',{},virtual],['Browser.setDownloadBehavior',{behavior:'allow',downloadPath:'/tmp/foreign'},virtual]])assert((await call(method,params,session)).error);
assert.equal(upstream.calls.length,0);upstream.emit('event',{method:'Target.targetCreated',params:{targetInfo:{targetId:'foreign'}}});upstream.emit('event',{sessionId:'foreign',method:'Runtime.consoleAPICalled',params:{}});upstream.emit('event',{sessionId:virtual,method:'Target.attachedToTarget',params:{sessionId:'foreign',targetInfo:{type:'iframe',targetId:'foreign'}}});await new Promise(resolve=>setTimeout(resolve,10));assert.equal(events.length,0);
const pending=call('Target.attachToTarget',{targetId:'owned',flatten:true});while(!upstream.resolveAttach)await new Promise(resolve=>setTimeout(resolve,1));admitted=false;upstream.resolveAttach({sessionId:'owned-pending'});assert((await pending).error);await relay.close();assert(upstream.calls.some(c=>c.method==='Target.detachFromTarget'&&c.params.sessionId==='owned-pending'));
assert.equal(upstream.calls.filter(c=>c.method==='Target.attachToTarget').length,1);
async function closeBeforeReply(timeout){
 const raw=new Upstream();const grant=new TabCdpRelay({upstream:raw,targetId:'owned',admitted:()=>true,WebSocketServer,cleanupTimeoutMs:timeout?25:1000});
 const ws=new WebSocket(await grant.listen());await once(ws,'open');ws.send(JSON.stringify({id:1,method:'Target.attachToTarget',params:{targetId:'owned',flatten:true}}));
 while(!raw.resolveAttach)await new Promise(resolve=>setTimeout(resolve,1));
 let complete=false;const closing=grant.close().then(()=>{complete=true;return null;},error=>error);
 if(timeout){const error=await closing;assert(error.message.includes('unknown'));assert.equal(grant.cleanup.state,'unknown');assert.equal(grant.cleanup.pendingAttachments,1);assert(!complete);}
 else{await new Promise(resolve=>setTimeout(resolve,10));assert(!complete);assert.equal(grant.cleanup.state,'draining');}
 raw.resolveAttach({sessionId:timeout?'late-after-deadline':'late-after-close'});
 if(!timeout){assert.equal(await closing,null);assert(complete);assert.equal(grant.cleanup.state,'complete');}
 for(let attempt=0;grant.cleanup.pendingAttachments&&attempt<100;attempt++)await new Promise(resolve=>setTimeout(resolve,1));
 assert.equal(grant.cleanup.pendingAttachments,0);assert(raw.calls.some(c=>c.method==='Target.detachFromTarget'&&c.params.sessionId===(timeout?'late-after-deadline':'late-after-close')));
 assert(grant.cleanup.receipts.some(r=>r.kind==='detached-owned-session'));
 if(timeout)assert.equal(grant.cleanup.state,'unknown');
}
await closeBeforeReply(false);await closeBeforeReply(true);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,crossTargetSessionContextMutationsRefused:7,foreignAndVirtualEventsFiltered:3,revokedPendingAttachReaped:true,virtualBrowserNeverUpstream:true,closeBeforeAttachReplyWaits:true,lateReplyDetachedAfterDeadline:true,deadlineRemainsExplicitUnknown:true},null,2)+'\n');console.log('TAB_CDP_RELAY_BOUNDARY_PASS');
