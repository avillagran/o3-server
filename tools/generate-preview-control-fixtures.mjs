#!/usr/bin/env node
// Execute complete native control and popup-adoption bodies with deferred synthetic work.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,staged,output]=process.argv.slice(2);
if(!output)throw Error('CHECKOUT STAGED OUTPUT required');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',hash=value=>crypto.createHash('sha256').update(value).digest('hex'),sources={};
const source=path=>{const value=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});sources[path]=hash(value);return value;};
const provenance=JSON.parse(fs.readFileSync(staged+'/SOURCE.json','utf8'));if(provenance.revision!==revision)throw Error('Pinned staging required');
for(const[path,digest]of Object.entries(provenance.files))if(hash(fs.readFileSync(staged+'/'+path))!==digest)throw Error('Changed source '+path);
const require=createRequire(staged+'/package.json'),canonical=async name=>import(pathToFileURL(require.resolve('effect/'+name)).href);
if(JSON.parse(fs.readFileSync(require.resolve('effect/package.json'),'utf8')).version!=='4.1.2')throw Error('Effect4.1.2 required');
const Effect=await canonical('Effect'),Schema=await canonical('Schema');
const contracts=await import(pathToFileURL(staged+'/packages/contracts/src/previewAutomation.ts').href);
const provider=await import(pathToFileURL(staged+'/packages/contracts/src/providerInstance.ts').href);if(!Schema.is(provider.ProviderInstanceId)('codex'))throw Error('Canonical Effect identity required');
const controlSource=source('apps/server/src/preview/SessionControl.ts');
const code=stripTypeScriptTypes(controlSource.slice(controlSource.indexOf('export class BrowserControlInterrupted')).replaceAll('export class','class'));
const browser=source('apps/server/src/preview/ServerBrowser.ts');
const part=(text,start,end)=>{const a=text.indexOf(start),b=text.indexOf(end,a);if(a<0||b<0)throw Error('Pinned source boundary missing');return text.slice(a,b);};
const AsyncFunction=Object.getPrototypeOf(async function(){}).constructor;
const popupBody=stripTypeScriptTypes(part(browser,'  const adoptPopup =','  const ensureTab ='));
const tabKeyBody=stripTypeScriptTypes(part(browser,'const tabKey =','\n',browser.indexOf('const tabKey =')));
const flush=async()=>{for(let index=0;index<40;index++)await Promise.resolve();};
const errorValue=error=>Object.fromEntries(['_tag','name','message','reason'].filter(key=>error[key]!==undefined).map(key=>[key,error[key]]));
async function runTrace(input){
 let now=1000,changes=0;const events=[],gates=new Map(),outcomes=new Map();
 const Control=new Function('Schema','Effect','PreviewAutomationControlReason','performance',code+';return SessionControl;')(Schema,Effect,contracts.PreviewAutomationControlReason,{now:()=>now});
 const control=new Control(input.agentId,()=>{changes++;events.push({event:'generation',controller:control.controller,generation:control.generation});});
 const gate=name=>{if(!gates.has(name)){let resolve,reject;const promise=new Promise((ok,no)=>{resolve=ok;reject=no;});promise.catch(()=>{});gates.set(name,{promise,resolve,reject});}return gates.get(name);};
 const runner=operation=>async(value=operation.value??operation.name)=>{events.push({event:'start',name:operation.name,value});if(operation.hold)await gate(operation.hold).promise;if(operation.fail)throw Error('Synthetic action rejected');events.push({event:'end',name:operation.name,value});return value;};
 const steps=[];
 for(const operation of input.operations){
  const before=events.length;let work;
  try{
   switch(operation.op){
    case 'agent':work=control.agent(operation.agent,runner(operation),operation.deadline);break;
    case 'human':work=control.human(operation.viewer,runner(operation));break;
    case 'system':work=control.system(runner(operation));break;
    case 'observe':work=control.observe(runner(operation));break;
    case 'latest':work=control.humanLatest(operation.viewer,operation.key,operation.value,runner(operation),operation.sum?((waiting,next)=>waiting+next):undefined);break;
    case 'take':work=control.take(operation.viewer);break;
    case 'release':work=control.release(operation.viewer,operation.after?runner({...operation,name:operation.after}):undefined);break;
    case 'disconnect':work=control.disconnect(operation.viewer,operation.after?runner({...operation,name:operation.after}):undefined);break;
    case 'close':work=control.close();break;
    case 'track':control.track(gate(operation.gate).promise);break;
    case 'handoff':control.trackUntilHandoff(gate(operation.gate).promise);break;
    case 'resolve':gate(operation.gate).resolve();break;
    case 'reject':gate(operation.gate).reject(Error('Synthetic tracked work rejected'));break;
    case 'clock':now=operation.now;break;
    case 'flush':break;
    default:throw Error('Unknown operation');
   }
   if(work!==undefined){outcomes.set(operation.name,{state:'pending'});Promise.resolve(work).then(value=>outcomes.set(operation.name,{state:'fulfilled',value:value??null}),error=>outcomes.set(operation.name,{state:'rejected',error:errorValue(error)}));}
  }catch(error){outcomes.set(operation.name,{state:'rejected',error:errorValue(error)});}
  await flush();
  steps.push({operation,expected:{agentId:control.agentId,controller:control.controller,generation:control.generation,changes,canA:control.agentCanActNow('A'),canB:control.agentCanActNow('B'),events:events.slice(before),outcomes:Object.fromEntries(outcomes)}});
 }
 for(const item of gates.values())item.resolve();await flush();
 return{input,steps};
}
const scenarios=[];
for(const agentId of [null,'A','B'])for(const agent of ['A','B'])for(const viewer of [null,'human']){
 const operations=[];if(viewer)operations.push({op:'take',viewer,name:'take'});
 operations.push({op:'agent',agent,name:'agent'},{op:'observe',name:'observe'},{op:'human',viewer:'human',name:'human'},{op:'system',name:'system'},{op:'close',name:'close'},{op:'observe',name:'after-close'});
 scenarios.push({agentId,operations});
}
for(const agentId of [null,'A'])for(const final of ['resolve','reject']){
 scenarios.push({agentId,operations:[{op:'agent',agent:'A',name:'running',hold:'run'},{op:'agent',agent:'A',name:'queued'},{op:'take',viewer:'one',name:'take-one'},{op:'take',viewer:'two',name:'take-two'},{op:'human',viewer:'one',name:'human'},{op:final,gate:'run'},{op:'release',viewer:'two',name:'wrong-release'},{op:'release',viewer:'one',name:'release',after:'drained'},{op:'agent',agent:'A',name:'after-release'},{op:'close',name:'close'}]});
}
for(const deadline of [999,1000,1001])for(const queued of [false,true]){
 const operations=[];if(queued)operations.push({op:'system',name:'blocker',hold:'run'});operations.push({op:'agent',agent:'A',name:'deadline',deadline});if(queued)operations.push({op:'clock',now:deadline},{op:'resolve',gate:'run'});operations.push({op:'close',name:'close'});scenarios.push({agentId:'A',operations});
}
for(const kind of ['track','handoff'])for(const resolve of ['resolve','reject']){
 scenarios.push({agentId:'A',operations:[{op:'agent',agent:'A',name:'first',hold:'run'},{op:kind,gate:'tracked'},{op:'system',name:'next'},{op:'resolve',gate:'run'},{op:'take',viewer:'human',name:'take'},{op:'release',viewer:'human',name:'release',after:'release-drain'},{op:resolve,gate:'tracked'},{op:'close',name:'close'}]});
}
for(const sum of [false,true])for(const intervening of [false,true])for(const takeover of [false,true]){
 const operations=[{op:'take',viewer:'human',name:'take'},{op:'human',viewer:'human',name:'running',hold:'run'},{op:'latest',viewer:'human',key:'hover',value:1,name:'first',sum}];
 if(intervening)operations.push({op:'system',name:'intervening'});
 operations.push({op:'latest',viewer:'human',key:'hover',value:2,name:'second',sum},{op:'latest',viewer:'human',key:'wheel',value:3,name:'wheel',sum});
 if(takeover)operations.push({op:'release',viewer:'human',name:'release'});
 operations.push({op:'resolve',gate:'run'},{op:'close',name:'close'});scenarios.push({agentId:null,operations});
}
for(const kind of ['agent','human','system','observe','latest'])for(const repeated of [false,true]){
 const operations=[{op:'take',viewer:'human',name:'take'},{op:'human',viewer:'human',name:'running',hold:'run'},{op:'close',name:'close'}];if(repeated)operations.push({op:'close',name:'close-again'});
 operations.push({op:kind,agent:'A',viewer:'human',key:'hover',value:4,name:'closed-action'},{op:'take',viewer:'human',name:'closed-take'},{op:'disconnect',viewer:'human',name:'closed-disconnect'},{op:'resolve',gate:'run'});scenarios.push({agentId:'A',operations});
}
scenarios.push({agentId:'A',operations:[{op:'take',viewer:'human',name:'take'},{op:'take',viewer:'human',name:'same-take'},{op:'handoff',gate:'navigation'},{op:'human',viewer:'human',name:'input'},{op:'disconnect',viewer:'other',name:'other-disconnect'},{op:'disconnect',viewer:'human',name:'disconnect',after:'release-input'},{op:'agent',agent:'A',name:'agent'},{op:'resolve',gate:'navigation'},{op:'close',name:'close'}]});
const traces=[];for(const input of scenarios)traces.push(await runTrace(input));
const popupCases=[];
for(const url of ['https://example.com/','HTTP://example.com/','http:relative','about:blank','data:text/plain,private','file:///owned',''])for(const agentId of [null,'A'])for(const closing of [false,true])for(const failure of [false,true])for(const controller of [null,'human']){
 const input={url,agentId,closing,failure,controller,profileId:agentId===null?undefined:'profile',thread:'thread\u0000owned',openerTab:'opener'};
 const run=new AsyncFunction('input','Effect',`${tabKeyBody}\n${popupBody}\n const events=[],adoptedPages=new Map(),constVoid=()=>{};const popup={url:()=>input.url,close:async()=>events.push({event:'close'})};const runPromise=Effect.runPromise;
 const openTab=options=>Effect.promise(async()=>{const{beforePublish,...plain}=options;events.push({event:'open',input:plain});if(input.failure)throw Error('Synthetic adoption rejected');const snapshot={threadId:options.threadId,tabId:'adopted'};beforePublish(snapshot);events.push({event:'published',receipt:adoptedPages.get(tabKey(snapshot.threadId,snapshot.tabId))?.page===popup});return snapshot;});
 const opener={closing:input.closing,threadId:input.thread,tabId:input.openerTab,profileId:input.profileId,control:{agentId:input.agentId,controller:input.controller},viewers:[{id:'human',push:message=>events.push({event:'push',message})}]};
 await adoptPopup(opener,popup);return{events,receipts:[...adoptedPages].map(([key,value])=>({key,openerTabId:value.openerTabId,samePage:value.page===popup}))};`);
 popupCases.push({input,expected:await run(input,Effect)});
}
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,canonicalEffectSchemaModule:require.resolve('effect/Schema'),stagingSha256:hash(fs.readFileSync(staged+'/SOURCE.json')),traces,popupCases},null,2)+'\n');
console.log(`${traces.length} whole native SessionControl traces; ${popupCases.length} actual adoptPopup traces`);
