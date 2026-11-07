// Whole native SessionControl with only its monotonic clock injected. There is
// no browser, filesystem reducer, transport stub success or copied scheduler.
import fs from 'node:fs';import vm from 'node:vm';import {stripTypeScriptTypes,createRequire} from 'node:module';import {pathToFileURL} from 'node:url';import assert from 'node:assert/strict';
assert(fs.existsSync('/run/.containerenv'));fs.mkdirSync('/out/guard',{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const require=createRequire('/deps/package.json'),modules=new Map();let clock=0;
const context=vm.createContext({performance:{now:()=>clock},DOMException});
const source=fs.readFileSync('/native/apps/server/src/preview/SessionControl.ts','utf8');
const module=new vm.SourceTextModule(stripTypeScriptTypes(source,{mode:'strip'}),{context});
await module.link(async specifier=>{
 if(modules.has(specifier))return modules.get(specifier);
 const values=specifier==='@t3tools/contracts'?await import('/native/packages/contracts/src/index.ts'):await import(pathToFileURL(require.resolve(specifier)).href);
 const synthetic=new vm.SyntheticModule(Object.keys(values),function(){for(const [name,value]of Object.entries(values))this.setExport(name,value);},{context});modules.set(specifier,synthetic);return synthetic;
});await module.evaluate();const {SessionControl}=module.namespace;
const deferred=()=>Promise.withResolvers();
const failed=(promise,trace,label)=>promise.then(value=>({value}),error=>{const failure={tag:error._tag??error.name,reason:error.reason??null,message:error.message};trace.push({event:label,error:failure});return {error:failure};});
const cases=[];
{
 const trace=[],entered=deferred(),write=deferred();let takeoverDone=false;
 const control=new SessionControl('agent',()=>trace.push({event:'generation',value:control.generation}));
 const running=control.agent('agent',async()=>{trace.push('running-enter');entered.resolve();await write.promise;trace.push('running-write');return 'running-success';});await entered.promise;
 const queued=failed(control.agent('agent',async()=>{trace.push('queued-write');}),trace,'queued-rejected');
 const takeover=control.take('human').then(()=>{takeoverDone=true;});
 const human=control.human('human',async()=>{trace.push('human-write');});
 assert.equal(control.controller,'human');assert.equal(control.generation,1);assert.equal(takeoverDone,false);assert(!trace.includes('running-write')&&!trace.includes('human-write'));
 write.resolve();assert.equal(await running,'running-success');const rejected=await queued;await takeover;await human;
 assert.equal(rejected.error.reason,'interrupted');assert(!trace.includes('queued-write'));assert(trace.indexOf('running-write')<trace.indexOf('human-write'));
 cases.push({name:'take-drains-entered-callback-rejects-control-queue',trace,controller:control.controller,generation:control.generation});await control.close();
}
{
 const trace=[],entered=deferred(),write=deferred(),handoff=deferred();let releaseDone=false;
 const control=new SessionControl('agent',()=>trace.push({event:'generation',value:control.generation}));await control.take('human');
 const running=control.human('human',async()=>{trace.push('human-enter');entered.resolve();await write.promise;trace.push('human-running-write');});await entered.promise;
 const queued=failed(control.human('human',async()=>trace.push('queued-human-write')),trace,'queued-human-rejected');
 control.trackUntilHandoff(handoff.promise);
 const released=control.release('human',async()=>trace.push('after-drain')).then(()=>{releaseDone=true;});
 const agent=control.agent('agent',async()=>trace.push('new-agent-write'));
 assert.equal(control.controller,null);assert.equal(control.generation,2);write.resolve();await running;const rejected=await queued;
 assert.equal(rejected.error.reason,'interrupted');assert(!releaseDone&&!trace.includes('after-drain')&&!trace.includes('new-agent-write'));
 handoff.resolve();await released;await agent;assert(!trace.includes('queued-human-write'));assert(trace.indexOf('human-running-write')<trace.indexOf('after-drain'));assert(trace.indexOf('after-drain')<trace.indexOf('new-agent-write'));
 cases.push({name:'release-drains-running-human-and-tracked-handoff',trace,controller:control.controller,generation:control.generation});await control.close();
}
{
 const trace=[],entered=deferred(),write=deferred();let closeDone=false;
 const control=new SessionControl('agent',()=>trace.push({event:'generation',value:control.generation}));
 const running=control.agent('agent',async()=>{trace.push('running-enter');entered.resolve();await write.promise;trace.push('running-write-after-close-request');return 'finished';});await entered.promise;
 const queued=failed(control.agent('agent',async()=>trace.push('queued-write')),trace,'queued-closed');
 const closing=control.close().then(()=>{closeDone=true;});
 let newcomer;try{await control.agent('agent',async()=>trace.push('new-write'));assert.fail('closed call admitted');}catch(error){newcomer={tag:error._tag??error.name,reason:error.reason};}
 assert.equal(newcomer.reason,'closed');assert.equal(closeDone,false);write.resolve();assert.equal(await running,'finished');const rejected=await queued;await closing;
 assert.equal(rejected.error.reason,'closed');assert(!trace.includes('queued-write')&&!trace.includes('new-write'));
 cases.push({name:'close-drains-entered-callback-before-resource-close',trace,newcomer});
}
{
 const trace=[],entered=deferred(),write=deferred();clock=0;const control=new SessionControl('agent');
 const running=control.agent('agent',async()=>{trace.push('entered-before-deadline');entered.resolve();await write.promise;trace.push('write-after-deadline');return 'entered-success';},10);await entered.promise;
 const queued=failed(control.agent('agent',async()=>trace.push('expired-queued-write'),10),trace,'deadline-queued');clock=20;write.resolve();assert.equal(await running,'entered-success');const rejected=await queued;
 assert.equal(rejected.error.tag,'TimeoutError');assert(!trace.includes('expired-queued-write'));
 cases.push({name:'deadline-checked-at-callback-entry-not-each-transport-write',trace});await control.close();
}
assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,wholeNativeSessionControl:true,injectedBoundaries:'Monotonic clock and deferred callbacks only',cases},null,2)+'\n');console.log('NATIVE_CONTROL_QUEUE_DRAIN_PASS');
