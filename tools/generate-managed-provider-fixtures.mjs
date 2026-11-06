#!/usr/bin/env node
// Execute the complete pinned provider-policy module and its real client schemas.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-managed-provider-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='c55db3e9c72fdf584f4b54192fb39beb88667b59';
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const directory=path.dirname(output);fs.mkdirSync(directory,{recursive:true});
const sources={},loaded=new Map();
function module(file){
 if(loaded.has(file))return loaded.get(file);
 const target=path.join(directory,crypto.createHash('sha256').update(file).digest('hex')+'.mjs');
 loaded.set(file,target);
 const source=execFileSync('git',['-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'});
 sources[file]=crypto.createHash('sha256').update(source).digest('hex');
 const code=stripTypeScriptTypes(source).replace(/from\s+(["'])([^"']+)\1/g,(all,quote,specifier)=>{
  let destination;
  if(specifier==='@t3tools/contracts')destination=contracts+'/src/index.ts';
  else if(specifier.startsWith('effect/'))destination=require.resolve(specifier);
  else if(specifier.startsWith('./')||specifier.startsWith('../'))destination=module(path.posix.normalize(path.posix.join(path.posix.dirname(file),specifier)));
  else if(specifier.startsWith('@t3tools/')){
   const [,pkg,entry]=specifier.match(/^@t3tools\/([^/]+)\/(.+)$/)??[];
   if(!pkg)throw Error('Unhandled package '+specifier);
   destination=module(`packages/${pkg}/src/${entry}.ts`);
  }else return all;
  return 'from '+JSON.stringify(pathToFileURL(destination).href);
 });
 fs.writeFileSync(target,code);return target;
}
const native=await import(pathToFileURL(module('apps/server/src/policy/managedProviders.ts')).href);
const contractsModule=await import(pathToFileURL(contracts+'/src/index.ts').href);
const Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const policies=[
 {name:'empty',limits:{providers:{}}},
 {name:'allowed-codex',limits:{providers:{allowedDrivers:['codex']}}},
 {name:'locked-codex',limits:{providers:{instances:{codex:{locked:true,binaryPath:'/managed/codex',launchArgs:'--managed --bound',environment:[{name:'CODEX_HOME',value:'/managed/home'},{name:'TOKEN',value:'managed-value'}]}}}}},
 {name:'default-codex',limits:{providers:{instances:{codex:{locked:false,binaryPath:'/managed/codex',launchArgs:'--managed',environment:[{name:'TOKEN',value:'managed-value'}]}}}}},
 {name:'locked-claude',limits:{providers:{instances:{claudeAgent:{locked:true,binaryPath:'/managed/claude',launchArgs:'--managed',environment:[{name:'CLAUDE_CONFIG_DIR',value:'/managed/config'}]}}}}},
 {name:'opencode-pin',limits:{providers:{instances:{opencode:{locked:true,binaryPath:'/managed/opencode'}}}}},
 {name:'ceiling',limits:{maxRuntimeMode:'approval-required',providers:{}}},
 {name:'ceiling-pins',limits:{maxRuntimeMode:'auto',providers:{instances:{codex:{locked:false,binaryPath:'/managed/codex',environment:[{name:'TOKEN',value:'managed-value'}]}}}}},
];
const empty=()=>({providerInstances:{},projectSettingsOverrides:{},sourceControlWriterModelSelection:null,reviewGuideModelSelection:null});
const layouts=[empty(),{...empty(),providerInstances:{codex:{driver:'codex',config:{binaryPath:'/user/codex',launchArgs:'--user',homePath:'/user/home',shadowHomePath:'/user/shadow',setupMode:'managed'},environment:[{name:'TOKEN',value:'user-value',sensitive:true},{name:'OTHER',value:'other',sensitive:false},{name:'T3CODE_CODEX_LAUNCH_ARGS',value:'--override',sensitive:false}]},claudeAgent:{driver:'claudeAgent',config:{binaryPath:'/user/claude',homePath:'/user/config'},environment:[{name:'CLAUDE_CONFIG_DIR',value:'/user/config',sensitive:false}]},custom:{driver:'codex',enabled:false,config:{binaryPath:'/user/custom'}}}},{...empty(),providerInstances:{other:{driver:'claudeAgent',enabled:false},opencode:{driver:'opencode',config:{binaryPath:'/user/open',serverUrl:'http://localhost:8888'}},agent:{driver:'acpRegistry',config:{agentId:'fixture',commandPath:'/user/agent',commandArgs:['--user']}}},sourceControlWriterModelSelection:{instanceId:'other',model:'fixture'},reviewGuideModelSelection:{instanceId:'other',model:'fixture'}},{...empty(),providerInstances:{codex:{driver:'codex',environment:[{name:'Token',value:'mixed',sensitive:false},{name:'TOKEN',value:'upper',sensitive:false},{name:'t3code_codex_launch_args',value:'--lower',sensitive:false}]}},projectSettingsOverrides:{p:{providerInstances:{codex:{enabled:false},other:{enabled:true}}}}}];
const cases=[];
for(const platform of ['linux','darwin','win32'])for(const item of policies){
 const document=Schema.decodeUnknownSync(contractsModule.ManagedPolicyDocument)({schemaVersion:1,organization:'Fixture Corp',limits:item.limits});
 const made=native.makeManagedProviders(document.limits.providers??{},document.limits.maxRuntimeMode,platform);
 if(made._tag!=='Valid')throw Error(item.name+': '+made.reason);
 for(let index=0;index<layouts.length;index++){
  const current=structuredClone(layouts[index]);const shown=made.providers.applyLimits(current);
  const changes=[['echo',shown],['unchanged',current],['removed',{...current,providerInstances:{}}],['new-default',{...current,providerInstances:{...shown.providerInstances,codex:{...(shown.providerInstances.codex??{driver:'codex'}),displayName:'Renamed'}}}],['new-instance',{...current,providerInstances:{...current.providerInstances,newClaude:{driver:'claudeAgent',enabled:true}}}],['project-enable',{...current,projectSettingsOverrides:{p:{providerInstances:{other:{enabled:true}}}}}],['pin-mutation',{...current,providerInstances:{...current.providerInstances,codex:{driver:'codex',config:{binaryPath:'/changed/codex'},environment:[{name:'TOKEN',value:'changed',sensitive:false}]}}}],['clear-environment',{...shown,providerInstances:{...shown.providerInstances,codex:{...(shown.providerInstances.codex??{driver:'codex'}),environment:[]}}}]];
  cases.push({name:`${platform}:${item.name}:${index}`,platform,policy:document,current,shown,locks:made.providers.lockedInstanceFields,changes:changes.map(([name,next])=>({name,next,expected:made.providers.vetWrite(current,next)}))});
 }
}
const hostEnvironment=[undefined,{organization:'Fixture Corp',lockedSettings:[]}].map(summary=>({summary:summary??null,input:{T3CODE_CODEX_LAUNCH_ARGS:'--host',t3code_codex_launch_args:'--lower',OTHER:'retained'},expected:native.providerHostEnvironment(summary,{T3CODE_CODEX_LAUNCH_ARGS:'--host',t3code_codex_launch_args:'--lower',OTHER:'retained'})}));
const nativeSettings=await import(pathToFileURL(module('packages/shared/src/serverSettings.ts')).href);
const textGenerationFallback=[];
const textDrivers=['codex','claudeAgent','cursor','grok','muse','pi','opencode','antigravity','fx','kiro','acpRegistry'];
const fallbackPolicies=[...policies,...textDrivers.map(driver=>({name:driver,limits:{providers:{allowedDrivers:[driver]}}}))];
const fallbackLayouts=[...layouts,{...empty(),providerInstances:Object.fromEntries(textDrivers.map(driver=>[driver,{driver,enabled:true}]))},{...empty(),providerInstances:{codex:{driver:'codex',enabled:false},claudeAgent:{driver:'claudeAgent',enabled:false},custom:{driver:'claudeAgent',enabled:true}}},{...empty(),providerInstances:{codex:{driver:'codex',config:{customModels:['\ufeffmy-custom \t',{slug:'other'}]}}},defaultModelSelection:{instanceId:'codex',model:'my-custom',options:[{id:'reasoningEffort',value:'low'}]}}];
for(const item of fallbackPolicies)for(const current of fallbackLayouts){
 const document=Schema.decodeUnknownSync(contractsModule.ManagedPolicyDocument)({schemaVersion:1,organization:'Fixture Corp',limits:item.limits});
 const allowed=item.limits.providers?.allowedDrivers;
 for(const includeConfiguredInstances of [false,true]){
  const enabled=(id,driver)=>{
   const configured=current.providerInstances[id];
   return (!allowed||allowed.includes(configured?.driver??driver))&&contractsModule.resolveProviderInstanceEnabled(configured??{driver,config:{}});
  };
  const options={includeConfiguredInstances,defaultModelSelection:current.defaultModelSelection??null};
  textGenerationFallback.push({policy:document,current,includeConfiguredInstances,expected:nativeSettings.fallbackTextGenerationModelSelection(current.providerInstances,enabled,options)});
 }
}
const fixture={revision,effect:'4.1.2',sources,cases,hostEnvironment,textGenerationFallback};
const serialized=Object.entries(fixture).map(([key,value])=>'  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))).join(',\n');
fs.writeFileSync(output,'{\n'+serialized+'\n}\n');
console.log(`${cases.length} overlays/locks, ${cases.reduce((n,c)=>n+c.changes.length,0)} writes, ${hostEnvironment.length} host environments, ${textGenerationFallback.length} text fallbacks`);
