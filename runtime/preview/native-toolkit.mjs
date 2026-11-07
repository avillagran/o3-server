// Private whole-source toolkit/broker composition. No public catalog installs
// this module. All State, caller and file capabilities are explicit dependencies.
import fs from 'node:fs';import path from 'node:path';import vm from 'node:vm';
import {createRequire,stripTypeScriptTypes} from 'node:module';import {pathToFileURL} from 'node:url';import {createHash} from 'node:crypto';
export async function createNativePreviewToolkit({sourceRoot,nativeRoot,effectRoot,platform}){
 if(typeof vm.SourceTextModule!=='function')throw Error('Exact admitted VM-module runtime required');
 const manifest=JSON.parse(fs.readFileSync(new URL('./toolkit-native-manifest.json',import.meta.url),'utf8'));
 if(manifest.revision!=='83a3674ffd84ffb33de768210c806cb1ac53fd80')throw Error('Toolkit source revision unavailable');
 for(const [name,digest]of Object.entries(manifest.files)){
  if(path.isAbsolute(name)||name.split('/').includes('..'))throw Error('Invalid toolkit manifest');
  const file=path.join(sourceRoot,name),identity=fs.lstatSync(file);
  if(!identity.isFile()||identity.isSymbolicLink()||createHash('sha256').update(fs.readFileSync(file)).digest('hex')!==digest)throw Error('Toolkit source integrity unavailable');
 }
 if(!platform||typeof platform.captureCaller!=='function'||typeof platform.invoke!=='function'||typeof platform.markUsed!=='function'||typeof platform.environmentId!=='string'||!Array.isArray(platform.supportedOperations))throw Error('Owned toolkit platform required');
 const require=createRequire(path.join(effectRoot,'package.json'));
 const load=async name=>name.startsWith('node:')?import(name):import(pathToFileURL(require.resolve(name)).href);
 const Effect=await load('effect/Effect'),Context=await load('effect/Context'),Layer=await load('effect/Layer'),Schema=await load('effect/Schema'),Fiber=await load('effect/Fiber'),Queue=await load('effect/Queue'),Stream=await load('effect/Stream'),Deferred=await load('effect/Deferred'),ManagedRuntime=await load('effect/ManagedRuntime'),SecureRandom=await load('effect/crypto/SecureRandom'),FileSystem=await load('effect/FileSystem');
 const {McpServer}=await load('effect/ai');
 const contracts=await import(pathToFileURL(path.join(nativeRoot,'packages/contracts/src/index.ts')).href);
 const NativePage=await import(pathToFileURL(path.join(nativeRoot,'apps/server/src/preview/ServerBrowserPage.ts')).href);
 // Reuse the complete original host callback, including error classification,
 // finally(markUsed), broker response, and ignored late response semantics.
 // The full browser service is not imported: it owns profile cleanup and launch.
 const browserSource=fs.readFileSync(path.join(sourceRoot,'apps/server/src/preview/ServerBrowser.ts'),'utf8');
 const start='  const handleRequest = ',end='  const mirrorManagerEvent = ';
 if(browserSource.split(start).length!==2)throw Error('Native host callback unavailable');
 const at=browserSource.indexOf(start),until=browserSource.indexOf(end,at);
 if(until<=at)throw Error('Native host callback unavailable');
 const hostMethod=browserSource.slice(at,until);
 if(createHash('sha256').update(hostMethod).digest('hex')!==manifest.methods?.['ServerBrowser.handleRequest'])throw Error('Native host callback integrity unavailable');
 const Threads=Context.Service('o3/NativePreviewThreadManagement'),Config=Context.Service('o3/NativePreviewServerConfig'),Invocation=Context.Service('o3/NativePreviewInvocation');
 const offered=new WeakMap(),issued=new WeakMap(),modules=new Map(),synthetics=new Map();let disposed=false;
 const unavailable=()=>new contracts.OrchestratorMcpFailure({code:'capability_denied',message:'This native preview capability is not owned.'});
 const refused=()=>{throw Error('External toolkit capability not admitted');};
 const refusingEffect=()=>Effect.fail(unavailable());
 const opaque={
  'packages/shared/src/nodeSqliteClient.ts':{readOnly:refused},
  'apps/server/src/orchestration-v2/ThreadFind.ts':{makeThreadFind:refused},
  'apps/server/src/orchestration-v2/WireProjection.ts':{projectTurnItemForWire:refused},
  'apps/server/src/sourceControl/sourceControlDefinitions.ts':{sourceControlDefinitions:Object.freeze({parseChangeRequestUrl:refused,canonicalRepositoryKey:refused,findByRepositoryKey:refused})},
  'apps/server/src/config.ts':{ServerConfig:Config},
  'apps/server/src/orchestration-v2/ThreadManagementService.ts':{ThreadManagementService:Threads},
  'apps/server/src/orchestration-v2/DispatchModeLimit.ts':{DispatchModeLimit:Context.Service('o3/UnusedPreviewDispatchModeLimit')},
  'apps/server/src/peer/PeerForwarding.ts':{PeerForwarding:Context.Service('o3/UnusedPreviewPeerForwarding'),remoteTarget:refused,linkedEnvironmentsOff:refusingEffect()},
  'apps/server/src/mcp/linkOrigin.ts':{assertNotLinked:refused,assertSameLink:refused,callerLinkOrigin:refused},
  // The handlers import only pure ID/path helpers from attachmentStore. Its
  // unrelated HTML/pruning exports cannot acquire ambient filesystem access.
  'packages/shared/src/toolOutput.ts':{htmlRenderFromToolItem:refused,mcpAppFromToolItem:refused,previewRecordingIdFromToolItem:refused},
 };
 const brokerPath='apps/server/src/mcp/PreviewAutomationBroker.ts';
 function synthetic(key,values){
  if(synthetics.has(key))return synthetics.get(key);
  const result=new vm.SyntheticModule(Object.keys(values),function(){for(const [name,value]of Object.entries(values))this.setExport(name,value);},{identifier:key});synthetics.set(key,result);return result;
 }
 function source(name){
  if(modules.has(name))return modules.get(name);
  if(!Object.hasOwn(manifest.files,name)||!name.endsWith('.ts'))throw Error('Unadmitted toolkit source import');
  const result=new vm.SourceTextModule(stripTypeScriptTypes(fs.readFileSync(path.join(sourceRoot,name),'utf8'),{mode:'strip'}),{identifier:name});modules.set(name,result);return result;
 }
 async function linker(specifier,reference){
  if(specifier==='@t3tools/contracts')return synthetic(specifier,contracts);
  if(specifier==='effect/Queue'&&reference.identifier===brokerPath){
   return synthetic('owned-native-broker-queue',{...Queue,offer:(queue,value)=>Effect.suspend(()=>{
    if(value?.type==='request'){
     const fiber=Fiber.getCurrent(),owner=fiber&&Context.getOrUndefined(fiber.context,Invocation);
     if(!owner||!value.request||typeof value.request!=='object')return Effect.fail(unavailable());
     offered.set(value.request,owner);
    }
    return Queue.offer(queue,value);
   })});
  }
  if(specifier==='node:fs'){
   const original=await import('node:fs'),values={};
   for(const [name,value]of Object.entries(original))if(name!=='default')values[name]=typeof value==='function'?refused:value;
   values.default=Object.freeze({...values});return synthetic('denied-ambient-attachment-fs',values);
  }
  if(specifier.startsWith('effect/')||specifier==='effect'||specifier==='node:crypto'||specifier==='node:path')return synthetic(specifier,await load(specifier));
  const name=specifier.startsWith('@t3tools/source-control-core/')?'packages/source-control-core/src/'+specifier.slice('@t3tools/source-control-core/'.length)+'.ts':specifier.startsWith('@t3tools/shared/')?'packages/shared/src/'+specifier.slice('@t3tools/shared/'.length)+'.ts':specifier.startsWith('.')?path.posix.normalize(path.posix.join(path.posix.dirname(reference.identifier),specifier)):null;
  if(!name)throw Error('Unadmitted toolkit dependency');
  if(opaque[name])return synthetic(name,opaque[name]);
  return source(name);
 }
 const handlers=source('apps/server/src/mcp/toolkits/preview/handlers.ts');await handlers.link(linker);await handlers.evaluate();
 const projectionModule=source('apps/server/src/orchestration-v2/ProjectionStore.ts');await projectionModule.link(linker);await projectionModule.evaluate();
 // Broker is an actual service implementation, not a synthetic invoke reducer.
 const brokerModule=source(brokerPath);if(brokerModule.status==='unlinked')await brokerModule.link(linker);if(brokerModule.status==='linked')await brokerModule.evaluate();
 const ctx=modules.get('apps/server/src/mcp/McpInvocationContext.ts').namespace;
 const access=modules.get('apps/server/src/mcp/McpToolAccess.ts').namespace;
 const toolkit=modules.get('apps/server/src/mcp/toolkits/preview/tools.ts').namespace.PreviewToolkit;
 const layers=Layer.merge(access.HandlersLayer.layer(handlers.namespace.layerStandard),access.HandlersLayer.layer(handlers.namespace.layerSnapshot));
 const runtime=ManagedRuntime.make(Layer.provideMerge(brokerModule.namespace.layer,SecureRandom.layerWebCrypto));
 const broker=await runtime.runPromise(brokerModule.namespace.PreviewAutomationBroker);
 const connected=await runtime.runPromise(Deferred.make());
 const host={clientId:contracts.SERVER_BROWSER_AUTOMATION_CLIENT_ID,environmentId:platform.environmentId,supportedOperations:platform.supportedOperations};
 const hostStream=await runtime.runPromise(broker.connect(host,{preferred:true}));
 const hostFiber=runtime.runFork(Effect.scoped(Stream.runForEach(hostStream,event=>{
  if(event.type==='connected')return Deferred.succeed(connected,event.connectionId);
  if(event.type!=='request')return Effect.void;
  const owner=offered.get(event.request);
  if(!owner)return Effect.fail(unavailable());
  const dependencies=vm.createContext({Effect,ServerBrowserPage:NativePage,broker,SERVER_HOST_CLIENT_ID:host.clientId,performance,
   runOperation:request=>platform.invoke(owner.caller,owner.tool,request,{updateCurrentTab:owner.input.updateCurrentTab!==false}),
   markUsed:request=>platform.markUsed(owner.caller,request),
  });
  vm.runInContext(stripTypeScriptTypes(hostMethod+'\nglobalThis.nativeHandleRequest=handleRequest;',{mode:'strip'}),dependencies);
  const work=dependencies.nativeHandleRequest(event.connectionId,event.request);
  // Original server requests run independently; a pending page script cannot
  // prevent a later dialog answer or timeout-termination producer from running.
  return work.pipe(Effect.forkScoped,Effect.asVoid);
 })));
 await runtime.runPromise(Deferred.await(connected));
 return {
  async invoke(tool,input,{signal}={}){
   if(disposed||!Object.hasOwn(toolkit.tools,tool))throw Error('Unadmitted private toolkit operation');
   const caller=await platform.captureCaller(),descriptor=caller?.scope;
   if(!descriptor||descriptor.environmentId!==platform.environmentId||!Array.isArray(descriptor.capabilities)||typeof caller.getThreadProjection!=='function')throw Error('Owned invocation scope unavailable');
   const scope=Object.freeze({...descriptor,capabilities:new Set(descriptor.capabilities)});issued.set(scope,caller);
   const registrations=[];
   const routedBroker={...broker,invoke:request=>{
    if(issued.get(request.scope)!==caller)return Effect.fail(unavailable());
    return broker.invoke(request).pipe(Effect.provideService(Invocation,Object.freeze({caller,tool,input:request})));
   }};
   const threads={getThreadShell:id=>Effect.tryPromise({try:()=>caller.getThreadProjection(id),catch:()=>unavailable()}).pipe(Effect.flatMap(value=>Schema.decodeUnknownEffect(contracts.OrchestrationV2ThreadProjectionJson)(value)),Effect.map(projectionModule.namespace.threadShellFromProjection))};
   let register=McpServer.registerToolkit(toolkit).pipe(Effect.provide(layers),Effect.provideService(McpServer.McpServer,{addTool:value=>Effect.sync(()=>registrations.push(value))}));
   for(const [service,value]of [[ctx.McpInvocationContext,scope],[brokerModule.namespace.PreviewAutomationBroker,routedBroker],[Threads,threads],[Config,{attachmentsDir:platform.attachmentsDir}],[FileSystem.FileSystem,caller.fileSystem??{}]])register=register.pipe(Effect.provideService(service,value));
   try{
    await runtime.runPromise(register);
    const registration=registrations.find(value=>value.tool.name===tool);if(!registration)throw Error('Native toolkit registration unavailable');
    // The actual Effect MCP registration performs native parameter decoding and
    // both content/structuredContent formatting; no Rust Value roundtrip here.
    return await runtime.runPromise(registration.handle(input).pipe(Effect.provide(registration.services)),{signal});
   }finally{issued.delete(scope);}
  },
  // This disposes logical broker/host fibers only. The Rust owner must fence
  // and drain entered execution before retiring the worker or claiming cleanup.
  async disposeBroker(){if(disposed)return;disposed=true;await runtime.runPromise(Fiber.interrupt(hostFiber));await runtime.dispose();},
  sourceRevision:manifest.revision,
 };
}
