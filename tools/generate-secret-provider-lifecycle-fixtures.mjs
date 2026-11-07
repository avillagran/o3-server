#!/usr/bin/env node
// Execute native whole patch, credential echo, write guard and removed-reference methods.
import fs from 'node:fs';
import path from 'node:path';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import {pinnedSource} from './native-source-input.mjs';
import {loadSecretStoreNative} from './secret-store-native.mjs';
const [checkout,output]=process.argv.slice(2),revision='83a3674ffd84ffb33de768210c806cb1ac53fd80';
const contracts=checkout+'/packages/contracts',require=createRequire(contracts+'/package.json');
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href),Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const sources={},source=pinnedSource(checkout,revision,sources),manifest=JSON.parse(fs.readFileSync(checkout+'/SOURCE.json'));
if(manifest.revision!==revision)throw Error('Wrong native source');
for(const file of Object.keys(manifest.files))source(file);
const native=await loadSecretStoreNative(checkout,contracts,path.join(path.dirname(output),'runtime'),source);
const C=native.C;
if(!Schema.is(C.ProviderInstanceId)('codex'))throw Error('Canonical schema identity required');
const shared=await import(pathToFileURL(checkout+'/packages/shared/src/serverSettings.ts').href);
const methods=native.methods.transactionMethods({},'/fixture/settings.json',native.settings.secrets,native.definitions);
const ref=(provider='vault')=>({provider,reference:'op://Private/Item/key'});
const provider=(account='initial')=>({kind:'one_password',label:'Private vault',config:{account}});
const base=Schema.decodeUnknownSync(C.ServerSettings)({...Schema.encodeSync(C.ServerSettings)(C.DEFAULT_SERVER_SETTINGS),secretProviders:{vault:provider(),keep:provider('other'),future:{kind:'future_kind',label:'F'.repeat(80),config:{opaqueCredential:'must-stay-private'}}},providerInstances:{custom:{driver:'codex',enabled:true,environment:[{name:'TOKEN',value:'',secret:ref(),sensitive:true}]},annotated:{driver:'antigravity',config:{apiKey:ref(),opaque:'untouched'}},future:{driver:'future_driver',config:{apiKey:ref(),opaque:'untouched'}}},sourceControlHosts:{github:{tokens:{'fixture.test':ref(),'keep.test':ref('keep'),'missing.test':ref('missing'),'literal.test':'unchanged-private'}}},usageLimitSources:{source:{kind:'cliproxy',url:'http://127.0.0.1:1',managementKey:ref(),enabled:false}},easDeviceHosts:[{kind:'eas',id:'eas-root',label:'EAS',accessToken:ref()}],simlockDeviceHosts:[{kind:'simlock',id:'simlock-root',label:'Simlock',gatewayUrl:'http://127.0.0.1:9',agentToken:ref()}]});
const patches=[{}, {secretProviders:{}},{secretProviders:{vault:null}}, {secretProviders:{vault:{kind:'one_password',label:'Replacement',config:{account:'replacement'}}}}, {secretProviders:{added:provider('added')}},{secretProviders:{missing:null}},{secretProviders:{vault:null},sourceControlHosts:{github:{tokens:{'fixture.test':'••••••'}}}},{secretProviders:{vault:null,keep:null}}];
const cases=[];
for(const [index,raw]of patches.entries()){
 const patch=Schema.decodeUnknownSync(C.ServerSettingsPatch)(raw);
 await Effect.runPromise(methods.ensureWrites(patch,'/fixture/settings.json'));
 const patched=shared.applyServerSettingsPatch(base,patch),echoed=methods.echo(base,patched);
 const result=native.settings.secrets.withoutRemovedSecretProviderReferences(base,echoed);
 cases.push({label:'patch-'+index,current:Schema.encodeSync(C.ServerSettings)(base),patch,result:Schema.encodeSync(C.ServerSettings)(result),public:Schema.encodeSync(C.ServerSettings)(methods.redact(result,{secretReferences:false})),scopes:C.requiredScopesForServerSettingsPatch(patch)});
}
const guards=[];
for(const kind of ['one_password','future_kind'])for(const config of [undefined,null,{}, {account:'valid'}, {account:'-bad'},{account:'a'.repeat(253)},{account:'a'.repeat(254)},{account:'valid',extra:'kept'}]){
 const patch={secretProviders:{vault:{kind,label:'Private vault',...(config===undefined?{}:{config})}}};
 const outcome=await Effect.runPromise(Effect.result(methods.ensureWrites(patch,'/fixture/settings.json')));
 guards.push({patch,...(outcome._tag==='Success'?{allowed:true}:{error:{operation:outcome.failure.operation,message:outcome.failure.message,detail:outcome.failure.cause.message}})});
}
const scopeCases=[];
for(const patch of [{},{secretProviders:{}},{secretProviders:{vault:null}},{secretProviders:{vault:provider()}},{secretProviders:{vault:null},enableAgentBrowserAccess:false},{providerInstances:{}},{usageLimitSources:{}},{defaultRuntimeMode:'auto'}])scopeCases.push({patch,scopes:C.requiredScopesForServerSettingsPatch(patch)});
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,cases,guards,scopeCases,boundaries:['Whole actual shared applyServerSettingsPatch and server credential echo/removal/guard methods','No filesystem or secret provider process runs','Runtime consumers compare supported settings groups; cloud group belongs to its separate foundation']},null,2)+'\n');
console.log(cases.length+' whole patch/cleanup traces, '+guards.length+' loaded-kind guards, '+scopeCases.length+' native authority decisions');
