// Whole native handler/broker composition with an explicit synthetic host.
// This does not certify a State caller, a CDP action, or public MCP admission.
import './projection-caller-oracle.mjs';
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {createNativePreviewToolkit} from '../../runtime/preview/native-toolkit.mjs';
const projection=JSON.parse(fs.readFileSync('/out/proof.json','utf8'));
let rawProjection=projection.cases.find(entry=>entry.status==='running'&&!entry.background).input;
const calls=[];
const caller={scope:{environmentId:'environment',capabilities:['preview'],issuedAt:1,requestNamespace:'private',thread:{threadId:'thread',providerSessionId:'actual-provider-session',providerInstanceId:'codex'}},getThreadProjection:async id=>{assert.equal(id,'thread');return rawProjection;}};
const toolkit=await createNativePreviewToolkit({sourceRoot:'/toolkit-native',nativeRoot:'/native',effectRoot:'/deps',platform:{environmentId:'environment',attachmentsDir:'/out/attachments',supportedOperations:['status','evaluate'],captureCaller:async()=>caller,invoke:async(owner,tool,request)=>{
 assert.equal(owner,caller);assert(['preview_status','preview_evaluate'].includes(tool));assert.equal(request.agentSessionId,'environment\0actual-provider-session\0thread');assert(['status','evaluate'].includes(request.operation));calls.push({tool,operation:request.operation,agentSessionId:request.agentSessionId});
 return {ok:true,result:request.operation==='evaluate'?'\ud800':{available:true,visible:false,tabId:null,url:null,title:null,loading:false}};
}}});
try{
 const result=await toolkit.invoke('preview_status',{});
 assert.equal(calls.length,1);assert.equal(result.isError,false);
 assert.equal(result.structuredContent.available,true);
 assert.equal(JSON.parse(result.content[0].text).available,true);
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
 fs.writeFileSync('/out/toolkit-proof.json',JSON.stringify({complete:true,wholeNativeHandlers:true,wholeNativeBroker:true,wholeNativeThreadAccess:true,calls,result,verdicts,qualification:'Complete native source composition with a contract-valid synthetic host; actual State and CDP consumers remain unproven.'},null,2)+'\n');
}finally{await toolkit.close();}
console.log('WHOLE_NATIVE_TOOLKIT_SOURCE_PASS');
