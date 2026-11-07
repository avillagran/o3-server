// Complete native projection and ThreadAccess modules. The fixtures are decoded
// by the full pinned contracts; no shallow projection schema is substituted.
import fs from 'node:fs';import vm from 'node:vm';import path from 'node:path';import assert from 'node:assert/strict';
import {stripTypeScriptTypes,createRequire} from 'node:module';import {pathToFileURL} from 'node:url';import {createHash} from 'node:crypto';
assert(fs.existsSync('/run/.containerenv'));fs.mkdirSync('/out/guard',{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const require=createRequire('/deps/package.json'),load=async name=>name.startsWith('node:')?import(name):import(pathToFileURL(require.resolve(name)).href);
const Effect=await load('effect/Effect'),Context=await load('effect/Context'),Schema=await load('effect/Schema');
const contracts=await import('/native/packages/contracts/src/index.ts');
const manifest=JSON.parse(fs.readFileSync('/source/runtime/preview/toolkit-native-manifest.json','utf8'));
for(const [name,digest]of Object.entries(manifest.files)){const file='/toolkit-native/'+name;assert(fs.lstatSync(file).isFile()&&!fs.lstatSync(file).isSymbolicLink());assert.equal(createHash('sha256').update(fs.readFileSync(file)).digest('hex'),digest);}
const modules=new Map(),synthetics=new Map(),Threads=Context.Service('oracle/ActualThreadService');
const refused=()=>{throw Error('Unadmitted external source capability');};
const opaque={
 'packages/shared/src/nodeSqliteClient.ts':{readOnly:refused},
 'apps/server/src/orchestration-v2/ThreadFind.ts':{makeThreadFind:refused},
 'apps/server/src/orchestration-v2/WireProjection.ts':{projectTurnItemForWire:refused},
 'apps/server/src/sourceControl/sourceControlDefinitions.ts':{sourceControlDefinitions:Object.freeze({parseChangeRequestUrl:refused,canonicalRepositoryKey:refused,findByRepositoryKey:refused})},
 'apps/server/src/orchestration-v2/ThreadManagementService.ts':{ThreadManagementService:Threads},
 'packages/shared/src/toolOutput.ts':{htmlRenderFromToolItem:refused,mcpAppFromToolItem:refused,previewRecordingIdFromToolItem:refused},
};
function synthetic(key,values){if(synthetics.has(key))return synthetics.get(key);const module=new vm.SyntheticModule(Object.keys(values),function(){for(const [name,value]of Object.entries(values))this.setExport(name,value);});synthetics.set(key,module);return module;}
function source(name){if(modules.has(name))return modules.get(name);assert(Object.hasOwn(manifest.files,name),name);const module=new vm.SourceTextModule(stripTypeScriptTypes(fs.readFileSync('/toolkit-native/'+name,'utf8'),{mode:'strip'}),{identifier:name});modules.set(name,module);return module;}
async function linker(specifier,reference){
 if(specifier==='@t3tools/contracts')return synthetic(specifier,contracts);
 if(specifier==='node:fs'){const original=await import('node:fs'),safe={};for(const [name,value]of Object.entries(original))if(name!=='default')safe[name]=typeof value==='function'?refused:value;safe.default=Object.freeze({...safe});return synthetic('denied-filesystem',safe);}
 if(specifier.startsWith('effect/')||specifier==='node:crypto'||specifier==='node:path')return synthetic(specifier,await load(specifier));
 const name=specifier.startsWith('@t3tools/source-control-core/')?'packages/source-control-core/src/'+specifier.slice('@t3tools/source-control-core/'.length)+'.ts':specifier.startsWith('@t3tools/shared/')?'packages/shared/src/'+specifier.slice('@t3tools/shared/'.length)+'.ts':path.posix.normalize(path.posix.join(path.posix.dirname(reference.identifier),specifier));
 return opaque[name]?synthetic(name,opaque[name]):source(name);
}
const projectionModule=source('apps/server/src/orchestration-v2/ProjectionStore.ts');await projectionModule.link(linker);await projectionModule.evaluate();
const access=source('apps/server/src/mcp/threadAccess.ts');await access.link(linker);await access.evaluate();
const context=modules.get('apps/server/src/mcp/McpInvocationContext.ts').namespace.McpInvocationContext;
const decodeProjection=Schema.decodeUnknownSync(contracts.OrchestrationV2ThreadProjectionJson),encodeShell=Schema.encodeUnknownSync(contracts.OrchestrationV2ThreadShellJson),decodeShell=Schema.decodeUnknownSync(contracts.OrchestrationV2ThreadShellJson);
export function projectNative(raw){const shell=encodeShell(projectionModule.namespace.threadShellFromProjection(decodeProjection(raw)));decodeShell(shell);return shell;}
const at='2026-11-07T00:00:00.000Z',later='2026-11-07T00:00:01.000Z';
const thread={id:'thread',projectId:'project',title:'Private caller',createdBy:'user',creationSource:'web',providerInstanceId:'codex',modelSelection:{instanceId:'codex',model:'private'},runtimeMode:'full-access',interactionMode:'default',branch:null,worktreePath:null,activeProviderThreadId:'provider-thread',lineage:{parentThreadId:null,relationshipToParent:null,rootThreadId:'thread'},forkedFrom:null,createdAt:at,updatedAt:later,archivedAt:null,deletedAt:null};
const providerThread={id:'provider-thread',driver:'codex',providerInstanceId:'codex',providerSessionId:'actual-provider-session',appThreadId:'thread',ownerNodeId:null,nativeThreadRef:null,nativeConversationHeadRef:null,status:'active',firstRunOrdinal:1,lastRunOrdinal:1,handoffIds:[],forkedFrom:null,pendingBackgroundTasks:[],createdAt:at,updatedAt:later};
const run=(status,ordinal=1)=>({id:'run-'+ordinal,threadId:'thread',ordinal,providerInstanceId:'codex',modelSelection:thread.modelSelection,providerThreadId:'provider-thread',userMessageId:'message-'+ordinal,rootNodeId:null,activeAttemptId:null,status,requestedAt:at,startedAt:later,completedAt:['completed','failed','interrupted'].includes(status)?later:null,checkpointId:null,contextHandoffId:null});
function input(status,background=false){
 const rawThread=Schema.decodeUnknownSync(contracts.OrchestrationV2AppThreadJson)(thread);
 const empty=projectionModule.namespace.emptyProjection({payload:rawThread,occurredAt:rawThread.createdAt});
 const raw=Schema.encodeUnknownSync(contracts.OrchestrationV2ThreadProjectionJson)(empty);
 // Canonical database reads return the authoritative thread timestamp, unlike
 // an in-memory thread.created projection before subsequent events are applied.
 raw.updatedAt=raw.thread.updatedAt;
 raw.runs=[run(status)];raw.providerThreads=[{...providerThread,pendingBackgroundTasks:background?[{kind:'subagent',taskId:'background-agent'}]:[]}];
 return raw;
}
const cases=[];
for(const status of ['preparing','starting','running','waiting','completed','failed','interrupted']){
 for(const background of [false,true]){
  const raw=input(status,background),projection=decodeProjection(raw),shell=projectionModule.namespace.threadShellFromProjection(projection),encoded=encodeShell(shell);
  decodeShell(encoded);
  const scope={environmentId:'environment',capabilities:new Set(['preview']),issuedAt:1,requestNamespace:'private',thread:{threadId:'thread',providerSessionId:'actual-provider-session',providerInstanceId:'codex'}};
  const verdict=await Effect.runPromise(access.namespace.loadCaller().pipe(Effect.tap(access.namespace.assertLiveCaller),Effect.match({onSuccess:()=>({live:true}),onFailure:error=>({live:false,code:error.code})}),Effect.provideService(context,scope),Effect.provideService(Threads,{getThreadShell:()=>Effect.succeed(shell)})));
  cases.push({status,background,input:raw,expectedShell:encoded,caller:verdict});
 }
}
// A later completed run does not erase an earlier actual active run.
const multiple=input('running');multiple.runs.push(run('completed',2));const multipleShell=encodeShell(projectionModule.namespace.threadShellFromProjection(decodeProjection(multiple)));decodeShell(multipleShell);
cases.push({name:'completed-latest-with-earlier-active',input:multiple,expectedShell:multipleShell});
for(const [name,mutate]of [
 ['waiting-latest-with-earlier-active',raw=>raw.runs.push(run('waiting',2))],
 ['requested-time-fallback',raw=>{raw.runs[0].startedAt=null;}],
 ['work-time-precedence',raw=>{raw.runs[0].workStartedAt='2026-11-07T00:00:02.000Z';}],
]){
 const raw=input('running');mutate(raw);
 const shell=encodeShell(projectionModule.namespace.threadShellFromProjection(decodeProjection(raw)));decodeShell(shell);
 cases.push({name,input:raw,expectedShell:shell});
}
for(const failedCard of [false,true]){
 const raw=input(failedCard?'running':'failed');
 raw.messages=[{id:'message-1',threadId:'thread',runId:'run-1',nodeId:null,role:'user',text:'/compact',attachments:[],streaming:false,createdAt:at,updatedAt:later,createdBy:'agent',creationSource:'server'}];
 if(failedCard)raw.turnItems=[{id:'compaction-1',threadId:'thread',runId:'run-1',nodeId:null,providerThreadId:'provider-thread',providerTurnId:null,nativeItemRef:null,parentItemId:null,ordinal:1,status:'failed',title:null,startedAt:at,completedAt:later,updatedAt:later,type:'compaction',driver:'codex'}];
 const shell=encodeShell(projectionModule.namespace.threadShellFromProjection(decodeProjection(raw)));decodeShell(shell);
 assert.equal(shell.latestRunId,'run-1');assert.equal(shell.activeRunId,failedCard?'run-1':null);
 cases.push({name:failedCard?'failed-compaction-card-retains-running':'failed-idle-run-remains-visible',input:raw,expectedShell:shell});
}
for(const entry of cases){
 const shell=entry.expectedShell;
 if(['completed','failed','interrupted'].includes(entry.status)){
  assert.equal(shell.activityRunStatus,null);assert.equal(shell.activityRunStartedAt,null);assert.equal(shell.activeRunId,null);
 }
 if(entry.status==='waiting')assert.equal(shell.activeRunId,null);
}
assert.equal(cases.find(entry=>entry.name==='waiting-latest-with-earlier-active').expectedShell.activeRunId,'run-1');
assert.equal(cases.find(entry=>entry.name==='waiting-latest-with-earlier-active').expectedShell.activityRunStatus,'waiting');
assert.equal(cases.find(entry=>entry.name==='requested-time-fallback').expectedShell.activityRunStartedAt,at);
assert.equal(cases.find(entry=>entry.name==='work-time-precedence').expectedShell.activityRunStartedAt,'2026-11-07T00:00:02.000Z');
for(const [name,text,createdBy,creationSource,idleCompaction,silent]of [
 ['legacy-idle-compact','/compact','agent','server',undefined,true],
 ['explicit-idle-compact','/compact','user','web','onReturn',true],
 ['manual-compact','/compact','user','web',undefined,false],
 ['js-bom-trim-compact','\ufeff/CoMpAcT\u2029','agent','server',undefined,true],
 ['non-js-whitespace-compact','\u0085/compact','agent','server',undefined,false],
 ['non-command-compact','/compact more','agent','server',undefined,false],
]){
 const raw=input('running');
 const message={id:'message-1',threadId:'thread',runId:'run-1',nodeId:null,role:'user',text,attachments:[],streaming:false,createdAt:at,updatedAt:later,createdBy,creationSource};
 if(idleCompaction!==undefined)message.idleCompaction=idleCompaction;
 raw.messages=[message];
 const shell=encodeShell(projectionModule.namespace.threadShellFromProjection(decodeProjection(raw)));decodeShell(shell);
 assert.equal(shell.activeRunId,silent?null:'run-1');assert.equal(shell.activityRunStatus,silent?null:'running');
 cases.push({name,input:raw,expectedShell:shell});
}
assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,wholeNativeProjectionStore:true,wholeNativeThreadAccess:true,fullProjectionAndShellCodecs:true,source:manifest,cases,qualification:'Native source oracle with complete contract-validated synthetic projections; no State/credential or public MCP acceptance claim.'},null,2)+'\n');
console.log('WHOLE_NATIVE_PROJECTION_CALLER_PASS');
