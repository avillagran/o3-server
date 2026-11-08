// Execute the complete pinned original credential registry and toolkit. Only
// environment/server-address/clock services and host callbacks are synthetic.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import {createHash} from 'node:crypto';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
import {createNativePreviewToolkit} from '../../runtime/preview/native-toolkit.mjs';
const revision='83a3674ffd84ffb33de768210c806cb1ac53fd80';
const registryPath='apps/server/src/mcp/McpSessionRegistry.ts';
const registrySha256='324094b7befdfc2cf488d95a7393948ccec55ae63fafe8c281ccd27ee89c8345';
const source=fs.readFileSync('/toolkit-native/'+registryPath,'utf8');
assert.equal(createHash('sha256').update(source).digest('hex'),registrySha256);
const require=createRequire('/deps/package.json');
const load=async name=>import(pathToFileURL(require.resolve(name)).href);
const Effect=await load('effect/Effect'),Context=await load('effect/Context');
const SecureRandom=await load('effect/crypto/SecureRandom'),Digest=await load('effect/crypto/Digest');
const {HttpServer}=await load('effect/http');
const Environment=Context.Service('fixture/native-registry-environment');
const linked=new Map();
const original=new vm.SourceTextModule(stripTypeScriptTypes(source,{mode:'strip'}),{identifier:registryPath});
await original.link(async specifier=>{
 if(linked.has(specifier))return linked.get(specifier);
 const values=specifier==='../environment/ServerEnvironment.ts'?{ServerEnvironment:Environment}
  :['./McpInvocationContext.ts','@t3tools/provider-core/server/mcpSession'].includes(specifier)?{}
  :specifier==='@t3tools/contracts'?await import('/native/packages/contracts/src/index.ts')
  :await load(specifier);
 const module=new vm.SyntheticModule(Object.keys(values),function(){for(const [name,value]of Object.entries(values))this.setExport(name,value);});
 linked.set(specifier,module);return module;
});
await original.evaluate();
let now=1000;
const registry=await Effect.runPromise(original.namespace.__testing.make({now:()=>now,livenessWindowMs:1000}).pipe(
 Effect.provideService(Environment,{getEnvironmentId:Effect.succeed('environment')}),
 Effect.provideService(HttpServer.HttpServer,{address:{_tag:'UnixAddress',path:'/private/unused'}}),
 Effect.provide(SecureRandom.layerWebCrypto),Effect.provide(Digest.layerWebCrypto),
));
const run=effect=>Effect.runPromise(effect.pipe(Effect.provide(SecureRandom.layerWebCrypto),Effect.provide(Digest.layerWebCrypto)));
const issue=request=>run(registry.issue({threadId:'thread',providerInstanceId:'codex',...request}));
const token=credential=>credential.config.authorizationHeader.replace(/^Bearer\s+/, '');
const resolve=credential=>run(registry.resolve(token(credential)));
const descriptor=scope=>({...scope,capabilities:[...scope.capabilities]});
const first=await issue({});
const firstScope=await resolve(first);
assert.equal(firstScope.thread.providerSessionId,first.config.providerSessionId);
assert.equal(firstScope.requestNamespace,first.config.providerSessionId);
assert(firstScope.capabilities.has('preview'));
await run(registry.revokeProviderSession('unrelated-runtime-provider-session'));
assert.equal((await resolve(first)).thread.providerSessionId,first.config.providerSessionId);
// Original registry identity is supplied to the whole original toolkit, rather
// than a hand-written session string masquerading as a runtime DB binding.
const fixtures=JSON.parse(fs.readFileSync('/caller-data.json','utf8')).cases;
let projection=fixtures.find(entry=>entry.status==='running'&&!entry.background).input;
let current=firstScope;const calls=[],marked=[];
const toolkit=await createNativePreviewToolkit({sourceRoot:'/toolkit-native',nativeRoot:'/native',effectRoot:'/deps',platform:{
 environmentId:'environment',attachmentsDir:'/out/attachments',supportedOperations:['status','evaluate'],
 captureCaller:async()=>({scope:descriptor(current),getThreadProjection:async id=>{assert.equal(id,'thread');return projection;}}),
 markUsed:async(_caller,request)=>{marked.push(request.requestId);},
 invoke:async(_caller,_tool,request)=>{
  assert.equal(request.agentSessionId,`${current.environmentId}\0${current.thread.providerSessionId}\0${current.thread.threadId}`);
  calls.push(request.operation);
  return request.operation==='evaluate'?'synthetic result':{available:true,visible:false,tabId:null,url:null,title:null,loading:false};
 },
}});
const cases=[];
try{
 for(const entry of fixtures.filter(entry=>entry.caller)){
  projection=entry.input;let before=calls.length;
  const read=await toolkit.invoke('preview_status',{});
  assert.equal(read.isError,false);assert.equal(calls.length-before,1);
  before=calls.length;const write=await toolkit.invoke('preview_evaluate',{expression:'synthetic owned callback'});
  assert.equal(write.isError,!entry.caller.live);assert.equal(calls.length-before,entry.caller.live?2:0);
  cases.push({name:entry.name,status:entry.status,background:entry.background,readAllowed:true,writeAllowed:!write.isError});
 }
 projection=fixtures.find(entry=>entry.status==='running'&&!entry.background).input;
 const foreign=await issue({providerInstanceId:'foreign-provider'});const foreignScope=await resolve(foreign);current=foreignScope;
 let before=calls.length;assert.equal((await toolkit.invoke('preview_status',{})).isError,false);assert.equal(calls.length-before,1);
 before=calls.length;assert.equal((await toolkit.invoke('preview_evaluate',{expression:'denied foreign write'})).isError,true);assert.equal(calls.length,before);
 current=firstScope;
 // A newly issued withheld-capability credential cannot dispatch a callback.
 // Issuing it does not mutate another immutable credential's capability set.
 const withheld=await issue({browserToolsAvailable:false});const withheldScope=await resolve(withheld);
 assert(!withheldScope.capabilities.has('preview'));assert((await resolve(first)).capabilities.has('preview'));
 current=withheldScope;before=calls.length;assert.equal((await toolkit.invoke('preview_status',{})).isError,true);assert.equal(calls.length,before);
 await run(registry.revokeThread('thread'));assert.equal(await resolve(first),undefined);assert.equal(await resolve(withheld),undefined);
 const replacement=await issue({});const replacementScope=await resolve(replacement);
 assert.notEqual(replacementScope.thread.providerSessionId,firstScope.thread.providerSessionId);
 assert.equal(replacementScope.requestNamespace,replacementScope.thread.providerSessionId);
 now+=1001;assert.equal(await resolve(replacement),undefined);
 fs.writeFileSync('/out/mcp-session-identity-proof.json',JSON.stringify({complete:true,revision,registry:{path:registryPath,sha256:registrySha256,wholeModule:true},originalIssuedScopeRecords:{first:descriptor(firstScope),foreign:descriptor(foreignScope),withheld:descriptor(withheldScope),replacement:descriptor(replacementScope)},credentialIdentityMatchesScopeAndNamespace:true,runtimeSessionRevocationDoesNotRevokeCredential:true,credentialReplacementDoesNotRetagOldScope:true,foreignReadonlyAllowedForeignWriteDenied:true,issuedCapabilitiesImmutableUntilCredentialRevocation:true,withheldCapabilityDispatchesNoCallback:true,livenessExpiryUsesOriginalRegistry:true,cases,qualification:'Whole original Registry issue/resolve/revoke and original toolkit/threadAccess/broker with controlled environment/address/clock and explicitly synthetic host callbacks. No actual provider process, CDP, UI-settings propagation or public MCP acceptance.'},null,2)+'\n');
}finally{await toolkit.disposeBroker();}
console.log('NATIVE_MCP_SESSION_IDENTITY_PASS');
