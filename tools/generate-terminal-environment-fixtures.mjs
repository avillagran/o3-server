#!/usr/bin/env node
// Execute the pinned terminal helper and its real schema/Effect dependency closure.
import fs from 'node:fs';
import crypto from 'node:crypto';
import NodePath from 'node:path';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,staged,output]=process.argv.slice(2);
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87';
if(!output)throw Error('usage: generate-terminal-environment-fixtures.mjs CHECKOUT STAGED_NATIVE OUTPUT');
const digest=value=>crypto.createHash('sha256').update(value).digest('hex');
const provenance=JSON.parse(fs.readFileSync(staged+'/SOURCE.json','utf8'));
if(provenance.revision!==revision)throw Error('Pinned contract staging required');
for(const [path,hash]of Object.entries(provenance.files))if(digest(fs.readFileSync(staged+'/'+path))!==hash)throw Error('Changed staged input: '+path);
const sources={};const source=path=>{const text=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});sources[path]=digest(text);return text;};
const slice=(text,from,to)=>{const start=text.indexOf(from),end=text.indexOf(to,start);if(start<0||end<0)throw Error('Missing pinned boundary: '+from);return text.slice(start,end);};
const js=text=>stripTypeScriptTypes(text.replaceAll('export const','const').replaceAll('export function','function'));
const require=createRequire(staged+'/package.json');const load=async name=>import(pathToFileURL(require.resolve('effect/'+name)).href);
const Effect=await load('Effect'),Schema=await load('Schema'),Option=await load('Option'),Path=await load('Path');
if(JSON.parse(fs.readFileSync(require.resolve('effect/package.json'),'utf8')).version!=='4.1.2')throw Error('Effect 4.1.2 required');
const contracts={};for(const name of ['settings','providerInstance','terminal']){
 const path=`packages/contracts/src/${name}.ts`;if(digest(fs.readFileSync(staged+'/'+path))!==digest(source(path)))throw Error('Contracts differ from pin');Object.assign(contracts,await import(pathToFileURL(staged+'/'+path).href));
}
if(!Schema.is(contracts.ProviderInstanceId)('codex'))throw Error('Canonical schema module identity check failed');
const hostPath='packages/shared/src/HostProcess.ts';if(digest(fs.readFileSync(staged+'/'+hostPath))!==digest(source(hostPath)))throw Error('HostProcess differs from pin');
const HostProcess=await import(pathToFileURL(staged+'/'+hostPath).href);
// Only metadata is evaluated: driver constructors, filesystem layers and launchers never run.
const imports=text=>[...text.matchAll(/import\s*\{([\s\S]*?)\}\s*from\s*"([^"]+)"/g)];
function resolveImport(specifier,from){
 if(specifier.startsWith('.'))return NodePath.posix.normalize(NodePath.posix.join(NodePath.posix.dirname(from),specifier));
 const match=specifier.match(/^@t3tools\/([^/]+)(\/.*)?$/);if(!match)throw Error('Unexpected driver import '+specifier);
 const base='packages/'+match[1],pkg=JSON.parse(source(base+'/package.json')),entry=pkg.exports['.'+(match[2]??'')];
 return NodePath.posix.normalize(base+'/'+(typeof entry==='string'?entry:entry.import));
}
function definingFile(path,name){
 const text=source(path);if(text.includes('export const '+name))return path;
 for(const match of text.matchAll(/export\s*\{([^}]+)\}\s*from\s*"([^"]+)"/g))if(match[1].split(',').some(value=>value.trim()===name))return definingFile(resolveImport(match[2],path),name);
 throw Error('Driver export not found '+name);
}
function constant(path,name){
 const text=source(path),match=text.match(new RegExp('(?:export )?const '+name+' = (ProviderDriverKind\\.make\\([^;]+\\));'));
 if(match)return new Function('ProviderDriverKind','return '+match[1])(contracts.ProviderDriverKind);
 for(const item of imports(text))if(item[1].split(',').some(value=>value.trim()===name))return constant(resolveImport(item[2],path),name);
 throw Error('Driver kind constant not found '+name);
}
function objectAt(text,start){let depth=0,quote=null,escape=false;for(let index=start;index<text.length;index++){const c=text[index];if(quote){if(escape)escape=false;else if(c==='\\')escape=true;else if(c===quote)quote=null;}else if(c==='"'||c==="'")quote=c;else if(c==='{')depth++;else if(c==='}'&&--depth===0)return text.slice(start,index+1);}throw Error('Unclosed metadata');}
const driversPath='apps/server/src/provider/builtInDrivers.ts',driversSource=source(driversPath);
const names=slice(driversSource,'export const BUILT_IN_DRIVERS:','export type BuiltInUsageReadersEnv').match(/=\s*\[([\s\S]*?)\]/)?.[1].split(',').map(value=>value.trim()).filter(Boolean);if(!names||names.length!==11)throw Error('Pinned driver list changed');
const drivers=names.map(name=>{
 const item=imports(driversSource).find(item=>item[1].split(',').some(value=>value.trim()===name));if(!item)throw Error('Driver import missing');
 const path=definingFile(resolveImport(item[2],driversPath),name),text=source(path),definition=text.slice(text.indexOf('export const '+name));
 const kind=definition.match(/driverKind:\s*(\w+)/)?.[1],start=definition.indexOf('{',definition.indexOf('metadata:'));
 if(!kind||start<0)throw Error('Driver metadata missing');
 return{driverKind:constant(path,kind),metadata:new Function('return ('+objectAt(definition,start)+')')()};
});
const manager=source('apps/server/src/terminal/Manager.ts');
const decoders=['decodeClaudeSettings','decodeCodexSettings'].map(name=>{const line=manager.match(new RegExp('^const '+name+' = .*;$','m'))?.[0];if(!line)throw Error('Missing native decoder');return line;}).join('\n');
const fragments=[decoders,
 slice(source('packages/provider-core/src/server/pathExpansion.ts'),'export function expandHomePath','\n}').concat('\n}'),
 source('packages/provider-core/src/server/instanceEnvironment.ts').replace(/^import[\s\S]*?from "[^\"]+";\s*/gm,''),
 slice(source('apps/server/src/provider/Drivers/CodexHomeLayout.ts'),'function resolveHomePath','const CodexShadowHomeContext'),
 slice(source('apps/server/src/provider/Drivers/ClaudeHome.ts'),'export const resolveClaudeHomePath','// A Claude Code session'),
 slice(source('apps/server/src/provider/ProviderInstanceRegistryHydration.ts'),'export const deriveProviderInstanceConfigMap','export const DEV_REPLAY_INSTANCE_ID'),
 slice(manager,'export const resolveProviderInstanceTerminalEnvironment','/** @public */'),
];
const bindings={Effect,Schema,Option,Path,HostProcess,NodePath,...contracts,BUILT_IN_DRIVERS:drivers};
const helper=new Function(...Object.keys(bindings),js(fragments.join('\n'))+';return resolveProviderInstanceTerminalEnvironment;')(...Object.values(bindings));
const prototypeIds=['constructor','toString','toLocaleString','valueOf','hasOwnProperty','isPrototypeOf','propertyIsEnumerable'];
const inputs=[];
const add=(value)=>inputs.push({home:'/fixture-home',cwd:'/fixture-cwd',platform:'linux',settings:{providerInstances:{},projectSettingsOverrides:{}},...value});
for(const id of [...drivers.map(driver=>driver.driverKind),'notInstalled',...prototypeIds,'__proto__','',' codex ','not.valid'])add({id,env:{BASE:'input'}});
const paths=['',' ','~','~/','~\\','~/../outside','relative','../relative','/abs',' /space ','\ufeff~/trim\u00a0','~/a/../../b','//double//','./','~/trailing/','~other','~\\back\\slashes'];
for(const driver of ['codex','claudeAgent'])for(const homePath of paths)for(const shadowHomePath of ['',' ','~/shadow',' relative '])add({id:'custom',env:{CODEX_HOME:'explicit-codex',CLAUDE_CONFIG_DIR:'explicit-claude'},settings:{providerInstances:{custom:{driver,config:{homePath,shadowHomePath}}},projectSettingsOverrides:{}}});
for(const driver of ['codex','claudeAgent','cursor','acpRegistry'])for(const config of [null,false,[],0,'bad',{}, {homePath:null},{homePath:42},{shadowHomePath:null},{shadowHomePath:7},{homePath:'~/valid',extra:'unknown'}])add({id:'custom',settings:{providerInstances:{custom:{driver,config}},projectSettingsOverrides:{}}});
for(const platform of ['linux','darwin'])for(const name of ['CODEX_HOME','CLAUDE_CONFIG_DIR','PATH','mixedCase','__proto__','constructor','toString'])for(const value of ['~/configured','',{_tag:'secret_ref',providerId:'fixture',key:'synthetic'}])for(const own of [false,true]){
 const env=JSON.parse(own?'{}':'{"BASE":"input"}');if(own)Object.defineProperty(env,name,{value:'explicit',writable:true,enumerable:true,configurable:true});
 add({id:'custom',platform,env,settings:{providerInstances:{custom:{driver:'cursor',environment:[{name,value}]}},projectSettingsOverrides:{}}});
}
for(const id of [...prototypeIds,'__proto__','custom'])for(const driver of ['codex','claudeAgent','cursor']){
 const providerInstances=JSON.parse('{}');Object.defineProperty(providerInstances,id,{value:{driver,environment:[{name:'TOKEN',value:'configured'}]},enumerable:true});
 add({id,env:{TOKEN:'explicit'},settings:{providerInstances,projectSettingsOverrides:{p:{providerInstances:{[id]:{enabled:true}}}}}});
}
add({id:'codex',settingsError:{_tag:'SyntheticSettingsError',message:'Synthetic settings unavailable'}});
const cases=[];
for(const input of inputs){
 const serverSettings={getSettings:input.settingsError?Effect.fail(input.settingsError):Effect.succeed(input.settings)};
 const path={...NodePath.posix,resolve:(...parts)=>NodePath.posix.resolve(input.cwd,...parts)};
 let outcome;
 try{const effect=helper({serverSettings,path,rawProviderInstanceId:input.id,env:input.env}).pipe(Effect.provideService(HostProcess.HomeDirectory,input.home),Effect.provideService(HostProcess.Platform,input.platform));outcome={ok:true,value:await Effect.runPromise(effect)};}
 catch(error){outcome={ok:false,error:{tag:error?._tag??error?.name??'Error',message:String(error?.message??error),...(error?.providerInstanceId===undefined?{}:{providerInstanceId:error.providerInstanceId}),...(error?.cause?{cause:error.cause}:{} )}};}
 let rawIdIsDecoded;try{rawIdIsDecoded=Schema.is(contracts.ProviderInstanceId)(input.id);}catch(error){throw new Error(`Decoded-id guard failed at case ${cases.length}, ${typeof input.id} ${String(input.id)}`,{cause:error});}
 cases.push({input,rawIdIsDecoded,expected:outcome});
}
const idSchemaCases=['',' ',' codex ','\ufeffcodex\u00a0','a'.repeat(64),'a'.repeat(65),'not.valid','__proto__','constructor','toString','日本',null,7,false,{},[]].map(input=>{
 const observe=run=>{try{return{ok:true,value:run()};}catch(error){return{ok:false,message:String(error.message)};}};
 return{input,decode:observe(()=>Schema.decodeUnknownSync(contracts.ProviderInstanceId)(input)),make:observe(()=>contracts.ProviderInstanceId.make(input))};
});
const terminalInputCases=[];
for(const operation of ['TerminalOpenInput','TerminalAttachInput'])for(const choice of [{omitted:true},...idSchemaCases.map(row=>({value:row.input}))]){
 const input={threadId:'thread',terminalId:'terminal',cwd:'/fixture-cwd',...(choice.omitted?{}:{providerInstanceId:choice.value})};
 let expected;try{expected={ok:true,value:Schema.decodeUnknownSync(contracts[operation])(input)};}catch(error){expected={ok:false,message:String(error.message)};}
 terminalInputCases.push({operation,input,expected});
}
fs.writeFileSync(output,JSON.stringify({revision,effectVersion:'4.1.2',canonicalEffectSchemaModule:require.resolve('effect/Schema'),scope:'Whole native terminal environment helper, real contract schemas, derive/default metadata, merge/pathExpansion/CodexHomeLayout/ClaudeHome; inert settings and fixture-bound Node POSIX Path services',sources,stagedProvenanceSha256:digest(fs.readFileSync(staged+'/SOURCE.json')),drivers,cases,idSchemaCases,terminalInputCases},null,2)+'\n');
console.log(`${cases.length} whole native terminal environment cases`);
