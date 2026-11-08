// Private broker process: no browser endpoint or tab target is admitted here.
// All browser work requires a Rust continuation and a separate per-tab owner.
import fs from 'node:fs';
import path from 'node:path';
import {MAGIC,MAX_FRAME_BYTES,FrameDecoder} from './framed-ipc.mjs';
import {ToolkitPipeClient} from './toolkit-pipe-client.mjs';
import {createToolkitDispatcher} from './toolkit-dispatcher.mjs';
if(process.versions.node!=='26.13.0')process.exit(2);
const keys=(value,allowed)=>value&&typeof value==='object'&&!Array.isArray(value)&&Object.keys(value).every(key=>allowed.includes(key));
let nonce,client,dispatcher,initializing=false,closing=false,lastId=0,finishTask;const invocations=new Map();
// A single stdout writer preserves ordering between continuation producers and
// source end markers. Console output is not a second protocol channel.
let writeTail=Promise.resolve();
function send(packet){
 const body=Buffer.from(JSON.stringify(packet));if(body.length>MAX_FRAME_BYTES)throw Error('Owned frame limit');
 const header=Buffer.alloc(12);MAGIC.copy(header);header.writeUInt32LE(body.length,8);
 const task=writeTail.then(()=>new Promise((resolve,reject)=>process.stdout.write(Buffer.concat([header,body]),error=>error?reject(error):resolve())));
 writeTail=task.catch(()=>{});return task;
}
function fail(){process.exitCode=2;void finish(false);}
async function initialize(packet){
 if(initializing||closing||!keys(packet,['kind','protocol','nonce','resourceId','configuration'])||packet.protocol!==1||typeof packet.nonce!=='string'||!/^[a-f0-9]{48}$/.test(packet.nonce)||typeof packet.resourceId!=='string'||!packet.resourceId)throw Error('Owned initialization unavailable');
 initializing=true;nonce=packet.nonce;const c=packet.configuration;
 if(!keys(c,['sourceRoot','nativeRoot','effectRoot','environmentId','attachmentsDir','supportedOperations'])||!['sourceRoot','nativeRoot','effectRoot','attachmentsDir'].every(key=>typeof c[key]==='string'&&path.isAbsolute(c[key]))||typeof c.environmentId!=='string'||!Array.isArray(c.supportedOperations))throw Error('Owned configuration unavailable');
 // The same immutable recursive-removal guard is installed before importing
 // original native modules. No browser profile/cleanup service is imported.
 const home=process.env.HOME;if(!home||!path.isAbsolute(home))throw Error('Owned home unavailable');
 fs.mkdirSync(path.join(home,'guard'),{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR=path.join(home,'guard');await import('./fs-guard.mjs');
 client=new ToolkitPipeClient({nonce,send});
 dispatcher=await createToolkitDispatcher({client,...c});
 if(closing)return;await send({kind:'ready',protocol:1,nonce});
}
async function invoke(packet){
 if(!dispatcher||closing||!keys(packet,['kind','id','nonce','operation','input','timeoutMs','caller'])||packet.nonce!==nonce||!Number.isSafeInteger(packet.id)||packet.id<=lastId||typeof packet.operation!=='string'||typeof packet.caller!=='string'||!/^[a-f0-9]{96}$/.test(packet.caller)||!Number.isSafeInteger(packet.timeoutMs)||packet.timeoutMs<1||packet.timeoutMs>120000)throw Error('Owned invocation unavailable');
 lastId=packet.id;const controller=new AbortController();
 const operation=dispatcher.invoke(packet.caller,packet.operation,packet.input,{signal:controller.signal});invocations.set(packet.id,{controller,operation});
 try{const value=await operation;await send({kind:'result',nonce,id:packet.id,ok:true,value});}
 catch{await send({kind:'result',nonce,id:packet.id,ok:false,error:{tag:'PreviewAutomationFailure',message:'Owned toolkit invocation failed.'}});}
 finally{invocations.delete(packet.id);}
}
function finish(success=true){
 if(finishTask)return finishTask;closing=true;
 finishTask=(async()=>{
  try{
   for(const {controller}of invocations.values())controller.abort();
   await Promise.allSettled([...invocations.values()].map(value=>value.operation));
   const receipt=await dispatcher?.close();if(!receipt?.known)success=false;
   const guardRefusals=fs.readdirSync(path.join(process.env.HOME,'guard')).filter(name=>name.endsWith('.attempts.jsonl')).length;
   if(guardRefusals)success=false;
   await send({kind:'closed',nonce,artifactHealthy:success,guardRefusals,nativeRecursiveRequests:0,executedRecursiveRemovals:0});
  }catch{success=false;}
  client?.close();process.exit(success?0:2);
 })();return finishTask;
}
const decoder=new FrameDecoder(packet=>{
 if(packet.kind==='init')void initialize(packet).catch(fail);
 else if(packet.kind==='service-result')client?.accept(packet);
 else if(packet.kind==='invoke')void invoke(packet).catch(fail);
 else if(packet.kind==='close'&&packet.nonce===nonce&&keys(packet,['kind','nonce']))void finish();
 else throw Error('Owned frame unavailable');
});
process.stdin.on('data',chunk=>{try{decoder.feed(chunk);}catch{fail();}});
process.stdin.on('end',()=>void finish(false));
process.on('SIGTERM',()=>void finish(false));process.on('SIGINT',()=>void finish(false));
