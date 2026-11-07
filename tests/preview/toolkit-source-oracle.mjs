// Whole native handler/broker composition with an explicit synthetic host.
// This does not certify a State caller, a CDP action, or public MCP admission.
import './projection-caller-oracle.mjs';
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createNativePreviewToolkit} from '../../runtime/preview/native-toolkit.mjs';
const projection=JSON.parse(fs.readFileSync('/out/proof.json','utf8'));
let rawProjection=projection.cases.find(entry=>entry.status==='running'&&!entry.background).input;
const calls=[],used=[];
let throwNext=false,held=null;
const caller={scope:{environmentId:'environment',capabilities:['preview'],issuedAt:1,requestNamespace:'private',thread:{threadId:'thread',providerSessionId:'actual-provider-session',providerInstanceId:'codex'}},getThreadProjection:async id=>{assert.equal(id,'thread');return rawProjection;}};
const toolkit=await createNativePreviewToolkit({sourceRoot:'/toolkit-native',nativeRoot:'/native',effectRoot:'/deps',platform:{environmentId:'environment',attachmentsDir:'/out/attachments',supportedOperations:['status','evaluate'],captureCaller:async()=>caller,markUsed:async(owner,request)=>{assert.equal(owner,caller);used.push(request.requestId);},invoke:async(owner,tool,request)=>{
 assert.equal(owner,caller);assert(['preview_status','preview_evaluate'].includes(tool));assert.equal(request.agentSessionId,'environment\0actual-provider-session\0thread');assert(['status','evaluate'].includes(request.operation));calls.push({tool,operation:request.operation,agentSessionId:request.agentSessionId});
 if(throwNext){throwNext=false;throw Error('synthetic owner failure\nprivate diagnostic omitted');}
 if(held&&request.operation==='evaluate'){held.entered.resolve(request);return await held.result.promise;}
 return request.operation==='evaluate'?'\ud800':{available:true,visible:false,tabId:null,url:null,title:null,loading:false};
}}});
try{
 const result=await toolkit.invoke('preview_status',{});
 assert.equal(calls.length,1);assert.equal(result.isError,false);
 assert.equal(result.structuredContent.available,true);
 assert.equal(JSON.parse(result.content[0].text).available,true);
 assert.equal(used.length,1);
 const verdicts=[];
 for(const entry of projection.cases.filter(entry=>entry.caller)){
  rawProjection=entry.input;const before=calls.length;
  const response=await toolkit.invoke('preview_status',{});
  // readsAsCaller deliberately does not run assertLiveCaller; actsAsCaller does.
  assert.equal(response.isError,false);assert.equal(calls.length-before,1);
  const beforeWrite=calls.length;
  const write=await toolkit.invoke('preview_evaluate',{expression:'syntheticHostValue'});
  assert.equal(write.isError,!entry.caller.live);
  assert.equal(calls.length-beforeWrite,entry.caller.live?2:0);
  if(entry.caller.live){assert.equal(write.structuredContent.value,'\ud800');assert.equal(JSON.parse(write.content[0].text).value,'\ud800');}
  verdicts.push({status:entry.status,background:entry.background,live:entry.caller.live,response,write});
 }
 rawProjection=projection.cases.find(entry=>entry.status==='running'&&!entry.background).input;
 throwNext=true;const usedBeforeFailure=used.length;
 const failure=await toolkit.invoke('preview_evaluate',{expression:'syntheticFailure'});
 assert.equal(failure.isError,true);assert.equal(used.length,usedBeforeFailure+1);
 assert(JSON.stringify(failure).includes('synthetic owner failure'));assert(!JSON.stringify(failure).includes('private diagnostic omitted'));
 const controller=new AbortController();held={entered:Promise.withResolvers(),result:Promise.withResolvers()};
 const running=toolkit.invoke('preview_evaluate',{expression:'heldOwnerCallback'},{signal:controller.signal});
 const observed=running.then(value=>({value}),error=>({interrupted:true}));
 const entered=await held.entered.promise;const usedBeforeInterrupt=used.length;
 controller.abort();assert.equal((await observed).interrupted,true);
 assert.equal(used.length,usedBeforeInterrupt,'Caller interruption must not pretend the entered callback finally ran');
 held.result.resolve('late owned result');
 const deadline=performance.now()+2000;
 while(!used.includes(entered.requestId)&&performance.now()<deadline)await new Promise(resolve=>setTimeout(resolve,1));
 assert(used.includes(entered.requestId),'Entered callback must finish its original finally after caller interruption');
 held=null;
 const survivor=await toolkit.invoke('preview_status',{});assert.equal(survivor.isError,false);
 fs.writeFileSync('/out/toolkit-proof.json',JSON.stringify({complete:true,wholeNativeHandlers:true,wholeNativeBroker:true,wholeNativeThreadAccess:true,wholeNativeHostCallback:true,calls,used,result,verdicts,failure,callerInterruptedEnteredCallbackDrained:true,hostSurvivedCallerInterruption:true,qualification:'Complete native source composition with a contract-valid synthetic host; actual State and CDP consumers remain unproven.'},null,2)+'\n');
}finally{await toolkit.disposeBroker();}
console.log('WHOLE_NATIVE_TOOLKIT_SOURCE_PASS');
