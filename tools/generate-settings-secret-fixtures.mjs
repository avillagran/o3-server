#!/usr/bin/env node
// Execute native mapping, collection and selective invalidation against actual schema registries.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import {loadManagedSettingsNative} from './managed-settings-native.mjs';
const[checkout,contracts,output,override]=process.argv.slice(2);if(!output)throw Error('usage: generate-settings-secret-fixtures.mjs CHECKOUT CONTRACTS OUTPUT_DIRECTORY [REVISION]');
const revision=override??'32a0531748c44b25072d7495ce0b9bd7e9450a87';const require=createRequire(contracts+'/package.json');if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');fs.mkdirSync(output,{recursive:true});const sources={};
function source(file){const text=execFileSync('git',['-C',checkout,'show',revision+':'+file],{encoding:'utf8'});sources[file]=crypto.createHash('sha256').update(text).digest('hex');return text;}
const checked=new Set();function check(file){if(checked.has(file))return;checked.add(file);const text=source(path.relative(checkout,file));if(fs.readFileSync(file,'utf8')!==text)throw Error('Contract source mismatch '+file);for(const m of text.matchAll(/\bfrom\s+["'](\.[^"']+)["']/g))check(path.resolve(path.dirname(file),m[1]));}check(contracts+'/src/index.ts');
const C=await import(pathToFileURL(contracts+'/src/index.ts').href);const S=await import(pathToFileURL(require.resolve('effect/Schema')).href);const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const native=await loadManagedSettingsNative(checkout,contracts,path.join(output,'runtime'),source);
const ref=(provider='vault',reference='op://Private/Item/key')=>({provider,reference});
function referenceFields(schema){return Object.keys(schema.fields).flatMap(field=>{if(C.secretReferencesInSettings(schema,{[field]:ref()}).length)return[{field,keyed:false}];if(C.secretReferencesInSettings(schema,{[field]:{host:ref()}}).length)return[{field,keyed:true}];return[];});}
const descriptor=(schema)=>({secretFields:C.secretSettingsFields(schema),referenceFields:referenceFields(schema)});
const descriptors={providers:Object.fromEntries([...native.secrets.providerSettingsSchemas].map(([kind,schema])=>[kind,descriptor(schema)])),hosts:Object.fromEntries([...native.hosts.hostSettingsSchemas].map(([kind,schema])=>[kind,descriptor(schema)])),usage:descriptor(C.UsageLimitSourceConfig),eas:descriptor(C.EasDeviceHostConfig),simlock:descriptor(C.SimlockDeviceHostConfig)};
const base={providerInstances:{},sourceControlHosts:{},usageLimitSources:{},easDeviceHosts:[],simlockDeviceHosts:[],secretProviders:{}};
const values=['literal','  literal \n','','••••••',ref(),ref(' other '),null,42,[],['literal'],{' EXAMPLE.COM ':'value','example.com':'last',' Ref.EXAMPLE ':ref(),'other':42}, {provider:'vault',reference:' op://Private/Item/key '}];
const cases=[];
async function add(label,patch){
 const input={...base,...patch};const calls=[];const mapped=native.secrets.mapSettingsSecrets(input,secret=>{calls.push(secret);return{value:'mapped:'+secret.value,redacted:secret.redacted};});const valueCalls=[];const valueMapped=native.secrets.mapSettingsSecretValues(input,(value,name)=>{valueCalls.push({value,name});return typeof value==='string'?'value:'+value:value;});
 const invalidations=[];await Effect.runPromise(native.hosts.invalidateSettingsSecrets({invalidate:references=>Effect.sync(()=>invalidations.push(references??null))},Effect.succeed(input)));
 const removedCurrent={...base,secretProviders:{vault:{kind:'one_password',label:'Vault',config:{account:'work'}},keep:{kind:'one_password',label:'Keep',config:{account:'keep'}}}};const removed=native.secrets.withoutRemovedSecretProviderReferences(removedCurrent,input);
 cases.push({label,input,calls,mapped,valueCalls,valueMapped,invalidations,removedCurrent,removed});
}
for(const [kind,d]of Object.entries(descriptors.providers))for(const field of d.secretFields)for(const value of values)await add('provider:'+cases.length,{providerInstances:{custom:{driver:kind,config:{[field]:value,ordinary:'untouched'}}}});
for(const [kind,d]of Object.entries(descriptors.hosts))for(const field of d.secretFields)for(const value of values)await add('host:'+cases.length,{sourceControlHosts:{[kind]:{[field]:value,ordinary:'untouched'}}});
for(const value of values){await add('usage:'+cases.length,{usageLimitSources:{usage:{managementKey:value,url:'http://fixture.invalid'}}});await add('device:'+cases.length,{easDeviceHosts:[{id:'same',accessToken:value}],simlockDeviceHosts:[{id:'same',agentToken:value}]});}
for(const sensitive of [true,false])for(const valueRedacted of [true,false,undefined])for(const value of ['literal','',ref(),ref('keep')])await add('environment:'+cases.length,{providerInstances:{custom:{driver:'codex',environment:[{name:'TOKEN',value,sensitive,...(valueRedacted===undefined?{}:{valueRedacted})}]}}});
await add('combined',{providerInstances:{p:{driver:'antigravity',config:{apiKey:ref()},environment:[{name:'ENV',value:ref(),sensitive:true}]}},sourceControlHosts:{github:{tokens:{' GITHUB.COM ':ref(),literal:'saved'},signIns:{' GITHUB.COM ':'session'}}},usageLimitSources:{usage:{managementKey:ref()}},easDeviceHosts:[{id:'eas',accessToken:ref()}],simlockDeviceHosts:[{id:'sim',agentToken:ref()}],secretProviders:{keep:{kind:'one_password'}}});
await add('host-registry-order',{sourceControlHosts:{bitbucket:{accessToken:ref('vault','op://Private/Bitbucket/token')},github:{tokens:{host:ref('vault','op://Private/Github/token')}},gitcafe:{token:ref('vault','op://Private/Gitcafe/token')}}});
const failureInvalidations=[];await Effect.runPromise(native.hosts.invalidateSettingsSecrets({invalidate:references=>Effect.sync(()=>failureInvalidations.push(references??null))},Effect.fail('unreadable settings')));
const hostDecode=[];for(const [kind,schema]of native.hosts.hostSettingsSchemas)for(const input of [{},null,{tokens:{' \ufeffΟΣ.İ.test ':'literal'}},{tokens:{' GITHUB.COM ':' value ',GITHUB_COM:ref()},hosts:{' GITHUB.COM ':{account:' user '}}},{tokens:{' GITHUB.COM ':ref()},signIns:{' GITHUB.COM ':ref()}}]){let decoded;try{decoded=S.decodeUnknownSync(schema)(input??{});}catch{decoded=S.decodeUnknownSync(schema)({});}hostDecode.push({kind,input,decoded});}
const resolveCases=[];
for(const [category,schemas] of [['hosts',native.hosts.hostSettingsSchemas],['providers',native.secrets.providerSettingsSchemas],['usage',new Map([[null,C.UsageLimitSourceConfig]])]])for(const[kind,schema]of schemas)for(const slot of referenceFields(schema))for(const answer of [undefined,'',' \t\ufeff ',' resolved value ']){
 const input={[slot.field]:slot.keyed?{' Fixture.TEST ':ref()}:ref()};const calls=[];
 const resolved=await Effect.runPromise(native.values.resolveSettingsSecrets({read:references=>Effect.sync(()=>{calls.push(references);return references.map(()=>answer);})},schema,input));
 resolveCases.push({category,kind,input,answer:answer??null,calls,resolved});
}
const literalCases=[];for(const value of ['', ' token ',ref(),ref('missing')])for(const providers of [{},{vault:{label:' Private vault '}}]){
 const result=await Effect.runPromise(Effect.result(C.requireSettingsSecretLiteral(value,providers)));
 literalCases.push({value,providers,...(result._tag==='Success'?{literal:result.success}:{error:result.failure.message})});
}
fs.writeFileSync(path.join(output,'settings-secrets.json'),JSON.stringify({revision,effect:'4.1.2',sources,descriptors,cases,failureInvalidations,hostDecode,resolveCases,literalCases},null,2)+'\n');console.log(cases.length+' native mapping/invalidation/removal cases; '+hostDecode.length+' host decoding cases');
