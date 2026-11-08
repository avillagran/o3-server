// Real pinned Effect queues and whole native toolkit, with an explicit
// synthetic host. This does not certify Rust admission or a CDP consumer.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import {reserveProducer} from '../../runtime/preview/producer-reservation.mjs';
import {createNativePreviewToolkit} from '../../runtime/preview/native-toolkit.mjs';
const require=createRequire('/deps/package.json');
const load=name=>import(pathToFileURL(require.resolve(name)).href);
const Effect=await load('effect/Effect'),Exit=await load('effect/Exit'),Queue=await load('effect/Queue');
const direct=[];
for(const disposition of ['accepted','closed','interrupted']){
 const queue=await Effect.runPromise(disposition==='interrupted'?Queue.bounded(0):Queue.unbounded());
 if(disposition==='closed')await Effect.runPromise(Queue.shutdown(queue));
 const entered=Promise.withResolvers(),release=Promise.withResolvers(),abort=new AbortController();
 let reserved=0,abandoned=0;
 const work=reserveProducer(Effect,Exit,{
  reserve:async()=>{reserved++;entered.resolve();if(disposition==='interrupted')await release.promise;},
  offer:Queue.offer(queue,'owned producer'),abandon:async()=>{abandoned++;},
 });
 let observed=false;
 const running=Effect.runPromise(work,{signal:abort.signal}).then(value=>({value}),()=>({interrupted:true})).finally(()=>{observed=true;});
 await entered.promise;
 if(disposition==='interrupted'){
  abort.abort();await new Promise(setImmediate);
  assert.equal(observed,false,'The uninterruptible reservation must finish before observer return');
  release.resolve();
 }
 const result=await running;
 assert.equal(reserved,1);assert.equal(abandoned,disposition==='accepted'?0:1);
 if(disposition==='accepted'){assert.equal(result.value,true);assert.equal(await Effect.runPromise(Queue.take(queue)),'owned producer');}
 if(disposition==='closed')assert.equal(result.value,false);
 if(disposition==='interrupted')assert.equal(result.interrupted,true);
 await Effect.runPromise(Queue.shutdown(queue));direct.push({disposition,reserved,abandoned,result});
}
const data=JSON.parse(fs.readFileSync('/caller-data.json','utf8'));
const projection=data.cases.find(entry=>entry.status==='running'&&!entry.background).input;
const events=[],pending=new Map();let reserveHold,executionHold;
const scope={environmentId:'environment',capabilities:['preview'],issuedAt:1,requestNamespace:'private',thread:{threadId:'thread',providerSessionId:'actual-provider-session',providerInstanceId:'codex'}};
const caller=label=>({label,scope,getThreadProjection:async id=>{assert.equal(id,'thread');return projection;}});
const toolkit=await createNativePreviewToolkit({sourceRoot:'/toolkit-native',nativeRoot:'/native',effectRoot:'/deps',platform:{
 environmentId:'environment',attachmentsDir:'/out/attachments',supportedOperations:['status','evaluate'],captureCaller:()=>{throw Error('Explicit fixture caller required');},
 reserveCallback:async(owner,tool,request)=>{
  assert.equal(request.agentSessionId,'environment\0actual-provider-session\0thread');
  assert(!pending.has(request));pending.set(request,{owner,tool,phase:'reserved'});events.push({kind:'reserve',caller:owner.label,id:request.requestId});
  if(reserveHold){const held=reserveHold;reserveHold=null;held.entered.resolve(request);await held.release.promise;}
 },
 abandonCallback:async(owner,request)=>{
  const slot=pending.get(request);assert.equal(slot?.owner,owner);assert.equal(slot.phase,'reserved');
  pending.delete(request);events.push({kind:'abandon',caller:owner.label,id:request.requestId});
 },
 invoke:async(owner,tool,request,metadata)=>{
  const slot=pending.get(request);assert.equal(slot?.owner,owner);assert.equal(slot.tool,tool);assert.equal(slot.phase,'reserved');
  assert(Number.isFinite(metadata.deadlineMs));slot.phase='running';events.push({kind:'invoke',caller:owner.label,id:request.requestId});
  try{
   if(executionHold&&request.operation==='evaluate'){const held=executionHold;executionHold=null;held.entered.resolve(request);return await held.release.promise;}
   return {available:true,visible:false,tabId:null,url:null,title:null,loading:false};
  }finally{slot.phase='responded';}
 },
 markUsed:async(owner,request)=>{
  const slot=pending.get(request);assert.equal(slot?.owner,owner);assert.equal(slot.phase,'responded');
  pending.delete(request);events.push({kind:'markUsed',caller:owner.label,id:request.requestId});owner.used?.resolve();
 },
 endInvocation:async owner=>{events.push({kind:'end',caller:owner.label});},
}});
try{
 const normal=caller('normal');const status=await toolkit.invoke('preview_status',{}, {caller:normal});
 assert.equal(status.isError,false);assert.deepEqual(events.filter(event=>event.caller==='normal').map(event=>event.kind),['reserve','invoke','markUsed','end']);
 const before=caller('before-offer'),abortBefore=new AbortController();
 const reservation={entered:Promise.withResolvers(),release:Promise.withResolvers()};reserveHold=reservation;
 const beforeRun=toolkit.invoke('preview_evaluate',{expression:'never executed'},{caller:before,signal:abortBefore.signal}).then(()=>({returned:true}),()=>({interrupted:true}));
 await reservation.entered.promise;abortBefore.abort();reservation.release.resolve();
 assert.equal((await beforeRun).interrupted,true);
 assert.deepEqual(events.filter(event=>event.caller==='before-offer').map(event=>event.kind),['reserve','abandon','end']);
 const after=caller('entered');after.used=Promise.withResolvers();const abortAfter=new AbortController();
 const execution={entered:Promise.withResolvers(),release:Promise.withResolvers()};executionHold=execution;
 const afterRun=toolkit.invoke('preview_evaluate',{expression:'held execution'},{caller:after,signal:abortAfter.signal}).then(()=>({returned:true}),()=>({interrupted:true}));
 const request=await execution.entered.promise;abortAfter.abort();assert.equal((await afterRun).interrupted,true);
 assert.equal(pending.get(request).phase,'running');
 assert.deepEqual(events.filter(event=>event.caller==='entered').map(event=>event.kind),['reserve','invoke','end']);
 execution.release.resolve('\ud800');await after.used.promise;
 assert.deepEqual(events.filter(event=>event.caller==='entered').map(event=>event.kind),['reserve','invoke','end','markUsed']);
 assert.equal(pending.size,0);
 fs.writeFileSync('/out/producer-proof.json',JSON.stringify({complete:true,direct,events,wholeNativeHandlers:true,wholeNativeBroker:true,wholeNativeHostCallback:true,qualification:'Pinned native Effect queues and whole source composition with explicit synthetic host; Rust owner, original absolute deadline conversion and CDP consumers remain unproven.'},null,2)+'\n');
}finally{await toolkit.disposeBroker();}
console.log('NATIVE_TOOLKIT_PRODUCERS_PASS');
