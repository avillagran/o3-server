// Actual no-tab Node entry, original toolkit and framed continuation client.
// The parent is an explicit synthetic Rust-service peer, never a CDP owner.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {spawn} from 'node:child_process';
import {MAGIC,FrameDecoder} from '../../runtime/preview/framed-ipc.mjs';
const nonce='d'.repeat(48),denied='a'.repeat(96),valid='c'.repeat(96),late='e'.repeat(96);
const projection=JSON.parse(fs.readFileSync('/caller-data.json','utf8')).cases.find(entry=>entry.status==='running'&&!entry.background).input;
const scope={environmentId:'environment',capabilities:['preview'],issuedAt:1,requestNamespace:'private',thread:{threadId:'thread',providerSessionId:'actual-provider-session',providerInstanceId:'codex'}};
fs.mkdirSync('/out/entry-home',{mode:0o700});
const child=spawn(process.execPath,['--experimental-vm-modules','/source/runtime/preview/toolkit-entry.mjs'],{env:{HOME:'/out/entry-home',XDG_CONFIG_HOME:'/out/entry-home/config',XDG_CACHE_HOME:'/out/entry-home/cache'},stdio:['pipe','pipe','pipe']});
const exited=new Promise((resolve,reject)=>{child.once('error',reject);child.once('exit',(code,signal)=>resolve({code,signal}));});
let writes=Promise.resolve(),lastService=0,nextCallback=0,stderr='',failure;
const ready=Promise.withResolvers(),closed=Promise.withResolvers(),results=new Map(),callbacks=new Map(),ended=new Set(),events=[];
const lateEntered=Promise.withResolvers(),lateUsed=Promise.withResolvers();let held;
function send(packet){const body=Buffer.from(JSON.stringify(packet)),header=Buffer.alloc(12);MAGIC.copy(header);header.writeUInt32LE(body.length,8);writes=writes.then(()=>new Promise((resolve,reject)=>child.stdin.write(Buffer.concat([header,body]),error=>error?reject(error):resolve())));return writes;}
function fail(error){failure??=error;ready.reject(error);closed.reject(error);for(const value of results.values())value.reject(error);child.kill('SIGKILL');}
const decoder=new FrameDecoder(packet=>{
 try{
  assert.equal(packet.nonce,nonce);
  if(packet.kind==='ready'){assert.equal(packet.protocol,1);ready.resolve();return;}
  if(packet.kind==='closed'){closed.resolve(packet);return;}
  if(packet.kind==='result'){const pending=results.get(packet.id);assert(pending);results.delete(packet.id);pending.resolve(packet);return;}
  assert.equal(packet.kind,'service');assert(packet.id>lastService);lastService=packet.id;
  assert([denied,valid,late].includes(packet.caller));events.push({caller:packet.caller===denied?'denied':packet.caller===late?'late':'valid',method:packet.method});
  let payload=null,ok=true,callbackReceipt;
  switch(packet.method){
   case 'callerScope':ok=packet.caller!==denied;payload=ok?scope:{};break;
   case 'projection':assert([valid,late].includes(packet.caller));assert.deepEqual(packet.input,{threadId:'thread'});payload=projection;break;
   case 'reserveCallback':{
    assert(!ended.has(packet.caller));assert.equal(packet.input.tool,'preview_evaluate');
    const request=packet.input.request;assert.equal(request.threadId,'thread');assert.equal(request.agentSessionId,'environment\0actual-provider-session\0thread');
    assert(['evaluate','status'].includes(request.operation));
    callbackReceipt=(++nextCallback).toString(16).padStart(96,'0');
    callbacks.set(callbackReceipt,{caller:packet.caller,request,phase:'reserved'});payload={};break;
   }
   case 'runCallback':{
    assert.deepEqual(Object.keys(packet.input).sort(),['callbackReceipt','deadlineMs','sourceDeadline']);
    const slot=callbacks.get(packet.input.callbackReceipt);assert.equal(slot?.caller,packet.caller);assert.equal(slot.phase,'reserved');
    assert(Number.isFinite(packet.input.deadlineMs));
    assert.deepEqual(Object.keys(packet.input.sourceDeadline).sort(),['lowerNs','upperNs']);
    const lower=BigInt(packet.input.sourceDeadline.lowerNs),upper=BigInt(packet.input.sourceDeadline.upperNs);assert(lower<=upper);
    if(packet.caller===late){
     assert.equal(slot.request.operation,'evaluate');slot.phase='running';held={packet,slot};lateEntered.resolve();return;
    }
    slot.phase='responded';payload={ok:true,value:slot.request.operation==='evaluate'?'\ud800':{available:true,visible:false,tabId:null,url:null,title:null,loading:false}};break;
   }
   case 'markUsed':{
    assert.deepEqual(Object.keys(packet.input),['callbackReceipt']);
    const slot=callbacks.get(packet.input.callbackReceipt);assert.equal(slot?.caller,packet.caller);assert.equal(slot.phase,'responded');callbacks.delete(packet.input.callbackReceipt);if(packet.caller===late)lateUsed.resolve();break;
   }
   case 'endInvocation':assert.deepEqual(packet.input,{});assert(!ended.has(packet.caller));ended.add(packet.caller);break;
   default:throw Error('Unexpected synthetic service request');
  }
  void send({kind:'service-result',nonce,id:packet.id,ok,payload,...(callbackReceipt?{callbackReceipt}:{})}).catch(fail);
 }catch(error){fail(error);}
});
child.stdout.on('data',chunk=>{try{decoder.feed(chunk);}catch(error){fail(error);}});
child.stderr.on('data',chunk=>{stderr+=chunk;assert(stderr.length<65536);});
try{
 await send({kind:'init',protocol:1,nonce,resourceId:'synthetic-no-tab-resource',configuration:{sourceRoot:'/toolkit-native',nativeRoot:'/native',effectRoot:'/deps',environmentId:'environment',attachmentsDir:'/out/attachments',supportedOperations:['status','evaluate']}});
 await ready.promise;
 const first=Promise.withResolvers();results.set(1,first);
 await send({kind:'invoke',id:1,nonce,operation:'preview_evaluate',input:{expression:'refused publication'},timeoutMs:15000,caller:denied});
 const refused=await first.promise;assert.equal(refused.ok,false);assert(ended.has(denied));
 assert.deepEqual(events.filter(event=>event.caller==='denied').map(event=>event.method),['callerScope','endInvocation']);
 const second=Promise.withResolvers();results.set(2,second);
 await send({kind:'invoke',id:2,nonce,operation:'preview_evaluate',input:{expression:'synthetic owned execution',caller:denied,ownerScope:{receipt:'model'}},timeoutMs:15000,caller:valid});
 const success=await second.promise;assert.equal(success.ok,true);assert.equal(success.value.isError,false);
 assert.equal(success.value.structuredContent.value,'\ud800');assert.equal(JSON.parse(success.value.content[0].text).value,'\ud800');
 assert(ended.has(valid));assert.equal(callbacks.size,0);assert.equal(nextCallback,2);
 // A completed observer can receive a racing cancellation without affecting a
 // later invocation. The admitted callback survives its own observer abort.
 await send({kind:'cancel-invocation',nonce,id:2});
 const third=Promise.withResolvers();results.set(3,third);
 await send({kind:'invoke',id:3,nonce,operation:'preview_evaluate',input:{expression:'held original callback'},timeoutMs:15000,caller:late});
 await lateEntered.promise;await send({kind:'cancel-invocation',nonce,id:3});
 const interrupted=await third.promise;assert.equal(interrupted.ok,false);assert(ended.has(late));
 assert.equal(held.slot.phase,'running');assert.equal(callbacks.size,1);
 assert.deepEqual(events.filter(event=>event.caller==='late').map(event=>event.method),['callerScope','projection','reserveCallback','runCallback','endInvocation']);
 held.slot.phase='responded';await send({kind:'service-result',nonce,id:held.packet.id,ok:true,payload:{ok:true,value:'late original result'}});
 await lateUsed.promise;assert.equal(callbacks.size,0);assert.equal(nextCallback,3,'Cancelled handler must not start a later status producer');
 assert.deepEqual(events.filter(event=>event.caller==='late').map(event=>event.method),['callerScope','projection','reserveCallback','runCallback','endInvocation','markUsed']);
 await send({kind:'close',nonce});const receipt=await closed.promise;const terminal=await exited;
 assert.deepEqual(terminal,{code:0,signal:null});assert.equal(receipt.artifactHealthy,true);assert.equal(receipt.guardRefusals,0);assert.equal(receipt.executedRecursiveRemovals,0);
 if(failure)throw failure;
 fs.writeFileSync('/out/entry-peer-proof.json',JSON.stringify({complete:true,actualEntry:true,noTabResource:true,publicationFailureEndedOnce:true,originalToolkitBothOutputChannelsPreserveUtf16:true,modelInputDidNotReplaceCaller:true,frozenCallbackPayload:true,originalSourceDeadlineBoundsPresent:true,observerCancellationEndedSourceBeforeCallbackFinally:true,cancelledHandlerDidNotStartMetadataProducer:true,lateCallbackMarkedOnce:true,events,terminal,receipt,qualification:'Actual Node entry and whole original toolkit with an explicit synthetic continuation-service peer. No Rust service owner, CDP grant or outer MCP transport acceptance.'},null,2)+'\n');
}finally{if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await exited;}if(stderr)fs.writeFileSync('/out/entry-peer-stderr.log',stderr);}
console.log('NATIVE_TOOLKIT_ENTRY_PEER_PASS');
