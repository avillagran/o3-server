// Whole native broker, actual Effect scheduling and stream generations. Only
// the random UUID source and host transport are synthetic private boundaries.
import fs from 'node:fs';import vm from 'node:vm';import {stripTypeScriptTypes,createRequire} from 'node:module';import {pathToFileURL} from 'node:url';import {createHash} from 'node:crypto';import assert from 'node:assert/strict';
assert(fs.existsSync('/run/.containerenv'));fs.mkdirSync('/out/guard',{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const require=createRequire('/deps/package.json'),load=async name=>import(pathToFileURL(require.resolve(name)).href);
const Effect=await load('effect/Effect'),Deferred=await load('effect/Deferred'),Stream=await load('effect/Stream'),SecureRandom=await load('effect/crypto/SecureRandom');
const provenance=JSON.parse(fs.readFileSync('/out/native-broker-source.json','utf8')),source=fs.readFileSync('/out/native-broker.ts','utf8');assert.equal(createHash('sha256').update(source).digest('hex'),provenance.sha256);
const module=new vm.SourceTextModule(stripTypeScriptTypes(source,{mode:'strip'}));const linked=new Map();
await module.link(async specifier=>{
 if(linked.has(specifier))return linked.get(specifier);
 // This import is used exclusively in erased type annotations in the broker;
 // authentication itself is deliberately outside this source-only proof.
 const values=specifier==='./McpInvocationContext.ts'?{}:specifier==='@t3tools/contracts'?await import('/native/packages/contracts/src/index.ts'):await load(specifier);
 const synthetic=new vm.SyntheticModule(Object.keys(values),function(){for(const [name,value]of Object.entries(values))this.setExport(name,value);});linked.set(specifier,synthetic);return synthetic;
});await module.evaluate();
let sequence=0;const random={uuidV4:()=>Effect.succeed(`00000000-0000-4000-8000-${String(++sequence).padStart(12,'0')}`)};
const cases=await Effect.runPromise(Effect.scoped(Effect.gen(function*(){
 const broker=yield* module.namespace.make,connected=yield* Deferred.make();const events=[];let answer=false;
 const host={clientId:'private-desktop',environmentId:'private-environment',supportedOperations:['status']};
 const stream=yield* broker.connect(host);
 yield* Effect.forkChild(Stream.runForEach(stream,event=>{
  events.push(event);
  if(event.type==='connected')return Deferred.succeed(connected,event.connectionId);
  if(event.type==='request'&&answer)return broker.respond({clientId:host.clientId,connectionId:event.connectionId,requestId:event.request.requestId,ok:true,result:{tabId:'successful-tab'}});
  return Effect.void;
 }));
 const connectionId=yield* Deferred.await(connected);
 const scope={environmentId:host.environmentId,thread:{threadId:'private-thread',providerInstanceId:'private-provider',providerSessionId:'private-session'}};
 const input={scope,operation:'status',input:{},timeoutMs:20,updateCurrentTab:false};
 const caught=effect=>effect.pipe(Effect.match({onFailure:error=>({failure:error._tag}),onSuccess:value=>({value})}));
 const first=yield* caught(broker.invoke(input));assert.equal(first.failure,'PreviewAutomationTimeoutError');
 const request=events.find(event=>event.type==='request');assert(request);assert.equal(request.connectionId,connectionId);
 yield* broker.respond({clientId:host.clientId,connectionId,requestId:request.request.requestId,ok:true,result:{tabId:'late-tab'}});
 answer=true;const second=yield* caught(broker.invoke(input));assert.equal(second.value.tabId,'successful-tab');
 const requests=events.filter(event=>event.type==='request');assert.equal(requests.length,2);assert.equal(requests[1].connectionId,connectionId);assert.equal(requests[1].request.tabId,undefined);
 answer=false;const started=performance.now();const third=yield* caught(broker.invoke({...input,updateCurrentTab:true}));const elapsed=performance.now()-started;
 assert.equal(third.failure,'PreviewAutomationTimeoutError');assert(elapsed>=1000&&elapsed<4000);
 const after=yield* caught(broker.invoke(input));assert.equal(after.failure,'PreviewAutomationNoAvailableHostError');
 return [{name:'metadata-timeout-keeps-host-and-drains-late-reply',sameConnection:true,metadataDoesNotChangeCurrentTab:true},{name:'current-tab-timeout-has-native-grace-and-evicts-host',elapsedMs:elapsed,subsequentError:after.failure}];
})).pipe(Effect.provideService(SecureRandom.SecureRandom,random)));
assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,wholeNativeBroker:true,source:provenance,injectedBoundaries:'Random UUID and synthetic host stream; no auth/MCP/real CDP admission claim',cases},null,2)+'\n');console.log('NATIVE_BROKER_TIMEOUT_PASS');
