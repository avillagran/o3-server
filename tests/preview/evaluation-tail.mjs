// Whole native timeout and scheduler functions; CDP is an explicit deferred
// service boundary. This does not certify a real browser or gateway transport.
import fs from 'node:fs';
import assert from 'node:assert/strict';
assert(fs.existsSync('/run/.containerenv'));
fs.mkdirSync('/out/guard',{mode:0o700});
process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const {evaluate}=await import('/native/apps/server/src/preview/ServerBrowserPage.ts');
const {SessionControl}=await import('/native/apps/server/src/preview/SessionControl.ts');
const control=new SessionControl('private-agent');
const events=[];let releaseTerminate,terminated=false;
const termination=new Promise(resolve=>{releaseTerminate=()=>{terminated=true;events.push('terminate-result');resolve({});};});
const cdp={send(method){
 events.push(method);
 if(method==='Runtime.evaluate')return new Promise(()=>{});
 if(method==='Runtime.terminateExecution')return termination;
 throw Error('Unexpected CDP operation');
}};
let ended=false;
await assert.rejects(control.agent('private-agent',async()=>{
 try{return await evaluate(cdp,{expression:'private-deferred',awaitPromise:true,returnByValue:true},20);}
 finally{ended=true;events.push('callback-finally');}
}),error=>error.tag==='PreviewAutomationTimeoutError');
assert(ended);assert(!terminated);
assert.deepEqual(events,['Runtime.evaluate','Runtime.terminateExecution','callback-finally']);
await control.agent('private-agent',async()=>{assert(!terminated);events.push('successor-entered');});
releaseTerminate();await termination;await control.close();
assert.deepEqual(events,['Runtime.evaluate','Runtime.terminateExecution','callback-finally','successor-entered','terminate-result']);
assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,wholeNativePageEvaluate:true,wholeNativeSessionControl:true,events,terminationEnqueuedBeforeCallbackFinally:true,terminationResultDoesNotRetainNativeTail:true,qualification:'Deferred CDP service only. A transport must preserve the already-sent old callback command without relabeling it as a successor; this does not authorize a new command after callback completion.'},null,2)+'\n');
console.log('WHOLE_NATIVE_EVALUATION_TAIL_PASS');
