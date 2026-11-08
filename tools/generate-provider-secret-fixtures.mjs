#!/usr/bin/env node
// Execute native registry secret mapping with actual declared settings schemas.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output,override]=process.argv.slice(2);
if(!output)throw Error('usage: generate-provider-secret-fixtures.mjs CHECKOUT CONTRACTS OUTPUT_DIRECTORY [REVISION]');
const revision=override??'6577c6b6185167c0e502fa01f9e8e8f2dee6ff25';
const require=createRequire(contracts+'/package.json');if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
fs.mkdirSync(output,{recursive:true});const sources={};
function source(file){const text=execFileSync('git',['-C',checkout,'show',revision+':'+file],{encoding:'utf8'});sources[file]=crypto.createHash('sha256').update(text).digest('hex');return text;}
const seenContracts=new Set();function checkedContract(file){if(seenContracts.has(file))return;seenContracts.add(file);const text=source(path.relative(checkout,file));if(fs.readFileSync(file,'utf8')!==text)throw Error('Contract mismatch '+file);for(const m of text.matchAll(/\bfrom\s+["'](\.[^"']+)["']/g))checkedContract(path.resolve(path.dirname(file),m[1]));}checkedContract(contracts+'/src/index.ts');
const C=await import(pathToFileURL(contracts+'/src/index.ts').href);
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const S=await import(pathToFileURL(require.resolve('effect/Schema')).href);
function imports(text){return text.replace(/\bfrom\s+(["'])(effect\/[^"']+)\1/g,(_,q,specifier)=>'from '+q+pathToFileURL(require.resolve(specifier)).href+q);}
const resolverFile=path.join(output,'resolver.mjs');fs.writeFileSync(resolverFile,imports(stripTypeScriptTypes(source('packages/secret-provider-core/src/server/SecretResolver.ts'),{mode:'strip'})));
const Resolver=await import(pathToFileURL(resolverFile).href);
const native=source('apps/server/src/provider/ProviderInstanceRegistry.ts');
const mappingFile=path.join(output,'mapping.mjs');
const header=`import {secretReferencesInSettings,withSettingsSecrets,providerInstanceConfigEnabledFlag} from ${JSON.stringify(pathToFileURL(contracts+'/src/index.ts').href)};\nimport * as Effect from ${JSON.stringify(pathToFileURL(require.resolve('effect/Effect')).href)};\nimport * as SecretResolver from ${JSON.stringify(pathToFileURL(resolverFile).href)};\n`;
const mapping=native.slice(native.indexOf('const secretReferencesIn ='),native.indexOf('type BlockedReason ='));
fs.writeFileSync(mappingFile,header+stripTypeScriptTypes(mapping,{mode:'strip'})+'\nexport {secretReferencesOf,readInstanceSecrets,resolveEntryEnabled,withoutSecretReferences};\n');
const M=await import(pathToFileURL(mappingFile).href);
const ref=(provider='personal',reference='op://Private/Item/key')=>({provider,reference});
const keyed=S.Struct({primary:C.SettingsSecretValue,keyed:S.Record(S.String,C.SettingsSecretValue),ordinary:S.Unknown});
const drivers={codex:{configSchema:C.CodexSettings},claudeAgent:{configSchema:C.ClaudeSettings},antigravity:{configSchema:C.AntigravitySettings},keyed:{configSchema:keyed}};
const declaredFields={};
for(const[kind,driver]of Object.entries(drivers))declaredFields[kind]=Object.keys(driver.configSchema.fields).flatMap(field=>{
 if(C.secretReferencesInSettings(driver.configSchema,{[field]:ref()}).length)return [{field,keyed:false}];
 if(C.secretReferencesInSettings(driver.configSchema,{[field]:{key:ref()}}).length)return [{field,keyed:true}];
 return [];
});
const cases=[];
async function add(label,input,values={},blocked,encoded=false){
 if(!encoded){const authored=input;try{input=S.encodeSync(C.ProviderInstanceConfig)({...input,...(input.environment?{environment:input.environment.map(v=>({sensitive:false,...v,name:v.name.trim()}))}:{})});}catch{cases.push({label,input,values,invalidEntry:true,calls:[]});return;}if(input.environment)input.environment.forEach((v,i)=>v.name=authored.environment[i].name);}
 const item={label,input,values,...(blocked?{blocked}:{}),calls:[]};
 let entry;try{entry=S.decodeUnknownSync(C.ProviderInstanceConfig)(input);}catch{item.invalidEntry=true;cases.push(item);return;}
 item.entry=entry;
 if(blocked){item.unavailable=blocked;cases.push(item);return;}
 const driver=drivers[entry.driver];if(!driver){item.unavailable=`Driver '${entry.driver}' is not registered in this build.`;cases.push(item);return;}
 let typed;try{typed=S.decodeUnknownSync(driver.configSchema)(entry.config??{});}catch(error){item.invalidConfig=true;item.nativeDetail=error.message;cases.push(item);return;}
 item.typedConfig=typed;item.enabled=M.resolveEntryEnabled(entry,typed);
 item.references=M.secretReferencesOf(new Map(Object.entries(drivers)),entry);
 let result;
 if(item.enabled)result=await Effect.runPromise(M.readInstanceSecrets(driver,entry).pipe(Effect.provideService(Resolver.SecretResolver,{read:references=>Effect.sync(()=>{item.calls.push(references);return references.map(r=>values[r.provider+'|'+r.reference]);})})));
 else result={secrets:{environment:M.withoutSecretReferences(entry.environment??[]),config:[]},unreadable:[]};
 item.secrets=result.secrets;item.unreadable=result.unreadable;
 item.resolvedConfig=result.secrets.config.length===0?typed:C.withSettingsSecrets(typed,result.secrets.config);
 if(result.unreadable.length)item.unavailable=`Could not read ${result.unreadable.join(', ')} from its secret provider. Unlock it, then refresh.`;
 cases.push(item);
}
for(const driver of ['codex','claudeAgent'])for(const enabled of [undefined,true,false])for(const value of ['literal',ref(),ref('work'),{provider:'bad id',reference:'op://a/b/c'}])for(const answer of [undefined,'','  ',' value\n']){
 const input={driver,...(enabled===undefined?{}:{enabled}),environment:[{name:' KEY ',value,sensitive:false}]};
 await add('environment:'+cases.length,input,answer===undefined?{}:{'personal|op://Private/Item/key':answer,'work|op://Private/Item/key':answer});
}
for(const config of [undefined,{},null,false,{enabled:false},{enabled:'bad'},{binaryPath:'  '},{apiKey:ref()}])for(const enabled of [undefined,false])await add('config:'+cases.length,{driver:'codex',...(config===undefined?{}:{config}),...(enabled===undefined?{}:{enabled}),environment:[{name:'KEY',value:ref()}]},{'personal|op://Private/Item/key':'resolved'});
for(const value of [ref(),'literal','',{'host-a':ref(),'host-b':ref('work'),literal:'saved'}])for(const answer of [undefined,'','\uFEFF',' value\n'])await add('declared:'+cases.length,{driver:'keyed',config:{primary:ref(),keyed:value,ordinary:ref()},environment:[{name:'FIRST',value:ref('first')},{name:'LITERAL',value:'keep',sensitive:false},{name:'SECOND',value:ref()}]},answer===undefined?{}:{'personal|op://Private/Item/key':answer,'work|op://Private/Item/key':answer,'first|op://Private/Item/key':answer});
await add('antigravity declaration',{driver:'antigravity',enabled:true,config:{apiKey:ref()},environment:[{name:'KEY',value:ref()}]},{'personal|op://Private/Item/key':'config secret'});
await add('unknown driver',{driver:'notRegistered',environment:[{name:'KEY',value:ref()}]});
await add('policy before invalid config',{driver:'codex',config:{enabled:'invalid'},environment:[{name:'KEY',value:ref()}]}, {},'Administrator forbids this driver.');
for(const value of [ref(),{provider:' personal ',reference:' op://a/b/c '},{provider:'vault',reference:'token'}])await add('encoded environment:'+cases.length,{driver:'claudeAgent',environment:[{name:' KEY ',value:'',secret:value}]},{'personal|op://Private/Item/key':'secret','personal|op://a/b/c':'trimmed'},undefined,true);
for(const reference of [' op://Private/Item/key ','op://Private/Item/key\n'])await add('raw config reference:'+cases.length,{driver:'keyed',config:{primary:{provider:'personal',reference},keyed:{},ordinary:'keep'},environment:[]});
const outputFixture={revision,effect:'4.1.2',sources,declaredFields,cases};fs.writeFileSync(path.join(output,'provider-secrets.json'),JSON.stringify(outputFixture,null,2)+'\n');console.log(cases.length+' native provider secret mapping cases');
