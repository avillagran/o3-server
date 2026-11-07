// Original complete requireTab/runOperation methods, not copied control logic.
// Only the single-tab data store, output/status and actual I/O are injected.
import fs from 'node:fs';import vm from 'node:vm';import {stripTypeScriptTypes,createRequire} from 'node:module';import {pathToFileURL} from 'node:url';import {createHash} from 'node:crypto';import assert from 'node:assert/strict';
assert(fs.existsSync('/run/.containerenv'));fs.mkdirSync('/out/guard',{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const require=createRequire('/deps/package.json');const load=async name=>import(pathToFileURL(require.resolve(name)).href);
const Effect=await load('effect/Effect'),{constVoid}=await load('effect/Function');
const {SessionControl,BrowserControlInterrupted}=await import('/native/apps/server/src/preview/SessionControl.ts');const NativePage=await import('/native/apps/server/src/preview/ServerBrowserPage.ts');
const original=fs.readFileSync('/out/native-consumer.ts','utf8'),provenance=JSON.parse(fs.readFileSync('/out/native-consumer-source.json','utf8'));
const hash=value=>createHash('sha256').update(value).digest('hex');assert.equal(hash(original),provenance.sha256);
function section(start,end){assert.equal(original.split(start).length,2);const at=original.indexOf(start),until=original.indexOf(end,at);assert(until>at);return original.slice(at,until);}
const selected={requireTab:section('  const requireTab = ','  const markUsed = '),runOperation:section('  const runOperation = ','  const executeTabOperation = '),READ_OPERATIONS:section('const READ_OPERATIONS:','\n\n'),tabKey:original.split('\n').find(line=>line.startsWith('const tabKey = '))};
for(const [name,text]of Object.entries(selected))assert.equal(hash(text),provenance.methods[name]);
function world(owner='agent'){
 const trace=[],tabs=new Map();let invalidations=0,execute=async request=>({operation:request.operation});
 const page={};const invalidateRefs=p=>{assert.equal(p,page);invalidations++;trace.push('invalidate');NativePage.invalidateRefs(p);};
 const tab={threadId:'thread',tabId:'tab',control:new SessionControl(owner,()=>invalidateRefs(page)),dialog:null,page,agentOperating:false};
 const context=vm.createContext({Effect,constVoid,BrowserControlInterrupted,ServerBrowserPage:{...NativePage,invalidateRefs},tabs,runPromise:Effect.runPromise,performance,
  executeTabOperation:async(_tab,request)=>{assert.equal(_tab,tab);trace.push('execute:'+request.operation);return execute(request);},
  updateFileChooserInterception:current=>{assert.equal(current,tab);trace.push('interception:'+current.agentOperating);},
  statusWithTitle:async(current,agent)=>{trace.push('status');return {available:!!current,agent};}
 });
 const source=stripTypeScriptTypes(Object.values(selected).join('\n')+'\nglobalThis.consumer={runOperation,tabKey};',{mode:'strip'});vm.runInContext(source,context);tabs.set(context.consumer.tabKey('thread','tab'),tab);
 // The data adapter uses the source-defined key, never an inferred path/epoch.
 context.findTab=(thread,id)=>Effect.succeed(tabs.get(context.consumer.tabKey(thread,id)));
 return {tab,trace,rawRun:context.consumer.runOperation,run:(operation,agent='agent')=>context.consumer.runOperation({operation,input:operation==='click'?{locator:'#target'}:operation==='evaluate'?{expression:"'finished'"}:{},threadId:'thread',tabId:'tab',tabIdExplicit:true,agentSessionId:agent,timeoutMs:60000},performance.now()+60000),setExecute:value=>{execute=value;},invalidations:()=>invalidations};
}
const cases=[];
{
 const w=world(),entered=Promise.withResolvers(),write=Promise.withResolvers();w.setExecute(async()=>{entered.resolve();await write.promise;w.trace.push('transport-write');return 'finished';});
 const running=w.run('evaluate');await entered.promise;assert.equal(w.tab.agentOperating,true);
 const take=w.tab.control.take('human');assert.equal(w.invalidations(),1);write.resolve();assert.equal(await running,'finished');await take;
 assert.equal(w.invalidations(),2);assert.equal(w.tab.agentOperating,false);assert(w.trace.indexOf('transport-write')<w.trace.lastIndexOf('invalidate'));
 cases.push({name:'entered-operation-drains-and-finally-invalidates-again',trace:w.trace});await w.tab.control.close();
}
{
 const w=world();await w.tab.control.take('human');let error;try{await w.run('snapshot');}catch(cause){error={tag:cause._tag,reason:cause.reason};}assert.equal(error.reason,'humanControl');assert(!w.trace.includes('execute:snapshot'));
 const before=w.invalidations();const result=await w.run('snapshot','foreign');assert.equal(result.operation,'snapshot');assert.equal(w.invalidations(),before);assert(!w.trace.includes('interception:true'));
 cases.push({name:'owner-read-during-human-control-vs-foreign-observer',trace:w.trace,error});await w.tab.control.close();
}
{
 const w=world();await w.tab.control.take('human');let error;try{await w.run('click','foreign');}catch(cause){error={tag:cause._tag,reason:cause.reason};}assert.equal(error.reason,'agentMismatch');assert(!w.trace.includes('execute:click'));
 const status=await w.run('status');assert.equal(status.available,true);assert(!w.trace.some(event=>event.startsWith('interception:')));
 cases.push({name:'foreign-mutation-denied-and-status-bypasses-action-queue',trace:w.trace,error});await w.tab.control.close();
}
{
 const w=world();let error;try{await w.rawRun({operation:'click',input:{locator:'#target'},threadId:'foreign-thread',tabId:'tab',tabIdExplicit:true,agentSessionId:'agent',timeoutMs:60000},performance.now()+60000);}catch(cause){error={tag:cause.tag??cause._tag,message:cause.message};}assert.equal(error.tag,'PreviewAutomationTabNotFoundError');assert(!w.trace.includes('execute:click'));
 cases.push({name:'explicit-tab-in-another-thread-is-not-found',trace:w.trace,error});await w.tab.control.close();
}
assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,wholeNativeMethods:['requireTab','runOperation'],source:provenance,injectedBoundaries:'Single-tab data store, I/O, interception observer and status output only; not real MCP auth/CDP',cases},null,2)+'\n');console.log('NATIVE_CONTROL_CONSUMER_PASS');
