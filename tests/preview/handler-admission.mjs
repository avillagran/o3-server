// Complete native declarations and Effect MCP registration. External caller and
// broker services deliberately refuse execution; no synthetic access reducer.
import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
assert(fs.existsSync('/run/.containerenv'));
fs.mkdirSync('/out/guard',{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const require=createRequire('/deps/package.json');
const load=async name=>import(pathToFileURL(require.resolve(name)).href);
const Effect=await load('effect/Effect'),Context=await load('effect/Context'),Layer=await load('effect/Layer'),Schema=await load('effect/Schema'),FileSystem=await load('effect/FileSystem'),{McpServer}=await load('effect/ai');
const contracts=await import('/native/packages/contracts/src/index.ts');
const previewContracts=await import('/native/packages/contracts/src/previewAutomation.ts');
const originalPage=await import('/native/apps/server/src/preview/ServerBrowserPage.ts');
const Broker=Context.Service('oracle/NativePreviewBroker'),Config=Context.Service('oracle/NativePreviewConfig'),Threads=Context.Service('oracle/NativeThreadManagement'),Forwarding=Context.Service('oracle/NativePeerForwarding');
let writerChecks=0;const brokerCalls=[];
const unexpected=()=>{throw Error('External boundary not admitted by this oracle');};
const callerFailure=()=>new contracts.OrchestratorMcpFailure({code:'parent_not_active',message:'Private caller service boundary refused'});
const opaque={
 '../orchestration-v2/DispatchModeLimit.ts':{DispatchModeLimit:Context.Service('oracle/DispatchModeLimit')},
 '../peer/PeerForwarding.ts':{PeerForwarding:Forwarding,remoteTarget:unexpected,linkedEnvironmentsOff:Effect.fail(callerFailure())},
 './linkOrigin.ts':{assertNotLinked:unexpected,assertSameLink:unexpected,callerLinkOrigin:unexpected},
 './delegatedTaskModes.ts':{resolveInteractionMode:unexpected,resolveRuntimeMode:unexpected},
 './threadAccess.ts':{assertFullAccess:unexpected,assertLiveCaller:unexpected,assertTargetWithinLimits:unexpected,
  loadCaller:()=>Effect.suspend(()=>{writerChecks++;return Effect.fail(callerFailure());}),readCaller:()=>Effect.fail(callerFailure()),unavailable:unexpected},
 '@t3tools/shared/threadLinks':{formatThreadLink:unexpected,hostedThreadLink:unexpected},
 '../../../attachmentStore.ts':{parseAttachmentUuid:unexpected,parseAttachmentFileExtension:unexpected,PENDING_ATTACHMENT_THREAD_SEGMENT:'pending',toSafeThreadAttachmentSegment:unexpected},
 '../../../attachmentPaths.ts':{resolveAttachmentRelativePath:unexpected},
 '../../../config.ts':{ServerConfig:Config},
 '../../../orchestration-v2/ThreadManagementService.ts':{ThreadManagementService:Threads},
 '../../PreviewAutomationBroker.ts':{PreviewAutomationBroker:Broker},
};
const modules=new Map(),synthetics=new Map();
function synthetic(key,values){if(synthetics.has(key))return synthetics.get(key);const module=new vm.SyntheticModule(Object.keys(values),function(){for(const [name,value]of Object.entries(values))this.setExport(name,value);});synthetics.set(key,module);return module;}
const paths={'context':'/out/native-policy/McpInvocationContext.ts','access':'/out/native-policy/McpToolAccess.ts','handlers':'/out/native-policy/handlers.ts','tools':'/native/apps/server/src/mcp/toolkits/preview/tools.ts'};
for(const [key,file]of Object.entries(paths))modules.set(key,new vm.SourceTextModule(stripTypeScriptTypes(fs.readFileSync(file,'utf8'),{mode:'strip'}),{identifier:key}));
async function linker(specifier){
 if(['./McpInvocationContext.ts','../../McpInvocationContext.ts'].includes(specifier))return modules.get('context');
 if(specifier==='../../McpToolAccess.ts')return modules.get('access');
 if(specifier==='./tools.ts')return modules.get('tools');
 if(opaque[specifier])return synthetic(specifier,opaque[specifier]);
 if(specifier==='@t3tools/contracts')return synthetic(specifier,contracts);
 return synthetic(specifier,await load(specifier));
}
await modules.get('handlers').link(linker);await modules.get('handlers').evaluate();
const access=modules.get('access').namespace,ctx=modules.get('context').namespace,toolkit=modules.get('tools').namespace.PreviewToolkit;
const actualHandlers=Layer.merge(access.HandlersLayer.layer(modules.get('handlers').namespace.layerStandard),access.HandlersLayer.layer(modules.get('handlers').namespace.layerSnapshot));
const registrations=[];
const broker={invoke:request=>Effect.suspend(()=>{brokerCalls.push({operation:request.operation,input:request.input});return Effect.fail(new contracts.OrchestratorMcpFailure({code:'orchestration_error',message:'Private broker service boundary refused'}));})};
const scope={capabilities:new Set(['preview']),thread:{threadId:'private-thread',providerSessionId:'private-session',providerInstanceId:'codex'}};
let register=McpServer.registerToolkit(toolkit).pipe(Effect.provide(actualHandlers),Effect.provideService(McpServer.McpServer,{addTool:tool=>Effect.sync(()=>{registrations.push(tool);})}));
for(const [service,value]of [[ctx.McpInvocationContext,scope],[Broker,broker],[Threads,{}],[Config,{attachmentsDir:'/out/attachments'}],[FileSystem.FileSystem,{}]])register=register.pipe(Effect.provideService(service,value));
await Effect.runPromise(register);assert.equal(registrations.length,12);
const inputs={preview_dialog:{accept:true},preview_status:{},preview_open:{},preview_navigate:{url:'https://example.invalid'},preview_resize:{mode:'fill'},preview_set_appearance:{colorScheme:'dark'},preview_snapshot:{save:true,includeImage:true},preview_act:{action:'press',key:'Enter'},preview_evaluate:{expression:'1'},preview_wait_for:{text:'private'},preview_recording_start:{},preview_recording_stop:{}};
const read=new Set(['preview_status','preview_snapshot','preview_wait_for']);const cases=[];
for(const registration of registrations){
 const writes=writerChecks,invokes=brokerCalls.length;
 const result=await Effect.runPromise(registration.handle(inputs[registration.tool.name]).pipe(Effect.provide(registration.services)));
 assert.equal(result.isError,true);
 if(read.has(registration.tool.name)){assert.equal(writerChecks,writes);assert.equal(brokerCalls.length,invokes+1);}
 else{assert.equal(writerChecks,writes+1);assert.equal(brokerCalls.length,invokes);}
 cases.push({tool:registration.tool.name,readsAsCaller:read.has(registration.tool.name),writerBoundaryChecks:writerChecks-writes,brokerInvocations:brokerCalls.length-invokes});
}
const snapshot=brokerCalls.find(call=>call.operation==='snapshot');assert(snapshot);assert(!Object.hasOwn(snapshot.input,'save'));assert(!Object.hasOwn(snapshot.input,'includeImage'));
const decode=Schema.decodeUnknownSync(previewContracts.PreviewAutomationWaitForInput);
assert.throws(()=>decode({expression:'globalThis.notAllowed=true'}));
const text='\ud800");globalThis.notAllowed=true;//';
const decoded=decode({text,expression:'globalThis.notAllowed=true'});assert.deepEqual(Object.keys(decoded),['text']);
const realm={document:{body:{innerText:text}}};realm.globalThis=realm;const expressions=[];
await originalPage.waitFor({evaluate:async expression=>{expressions.push(expression);return vm.runInNewContext(expression,realm);}},decoded);
assert.equal(realm.notAllowed,undefined);assert.equal(expressions.length,1);assert(expressions[0].includes('\\ud800'));
assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,wholeNativeHandlers:true,wholeNativeMcpToolAccess:true,wholeNativeInvocationContext:true,wholeEffectMcpRegistration:true,cases,snapshotDeprecatedFlagsStripped:true,waitForExpressionNotAnInput:true,waitForTextQuotedByWholeNativeFunction:true,isolatedUtf16Retained:true,qualification:'External caller/broker/attachment services are explicit refusing boundaries; this proves original declaration routing and codecs, not real provider or filesystem execution.'},null,2)+'\n');
console.log('WHOLE_NATIVE_PREVIEW_HANDLER_ADMISSION_PASS');
