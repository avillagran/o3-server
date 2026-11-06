#!/usr/bin/env node
// Compile the complete pinned settings AST into a closed, reviewable Rust decoder program.
import fs from 'node:fs';
import {loadManagedSettingsNative} from './managed-settings-native.mjs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output,referenceOverride]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-managed-settings-codec.mjs CHECKOUT CONTRACTS OUTPUT_DIRECTORY');
const revision=referenceOverride??'c55db3e9c72fdf584f4b54192fb39beb88667b59';
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const S=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const NativeAnnotations=await import(pathToFileURL(path.join(path.dirname(require.resolve('effect/Schema')),'internal/schema/annotations.js')).href);
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const Option=await import(pathToFileURL(require.resolve('effect/Option')).href);
const Duration=await import(pathToFileURL(require.resolve('effect/Duration')).href);
const C=await import(pathToFileURL(contracts+'/src/index.ts').href);
fs.mkdirSync(output,{recursive:true});
const sources={};
function source(file){const value=execFileSync('git',['-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'});sources[file]=crypto.createHash('sha256').update(value).digest('hex');return value;}
for(const file of ['settings.ts','baseSchemas.ts','model.ts','device.ts','speech.ts','providerInstance.ts','settingsSecretValue.ts','secretReference.ts']){
 const name='packages/contracts/src/'+file;const original=source(name);
 if(fs.readFileSync(path.join(contracts,'src',file),'utf8')!==original)throw Error('Contract source mismatch: '+name);
}
const checkedContracts=new Set();
function checkContract(file){
 if(checkedContracts.has(file))return;checkedContracts.add(file);
 const relative=path.relative(checkout,file);const original=source(relative);
 if(fs.readFileSync(file,'utf8')!==original)throw Error('Contract source mismatch: '+relative);
 for(const match of original.matchAll(/\bfrom\s+["'](\.[^"']+)["']/g))checkContract(path.resolve(path.dirname(file),match[1]));
}
checkContract(contracts+'/src/index.ts');
const nodes=[];const ids=new WeakMap();const nodeAsts=[];const transformSources={};
const clone=value=>value===undefined?null:JSON.parse(JSON.stringify(value));
function plain(value){
 if(Duration.isDuration(value))return Duration.toMillis(value);
 if(Array.isArray(value))return value.map(plain);
 if(value&&typeof value==='object')return Object.fromEntries(Object.entries(value).filter(([,v])=>v!==undefined).map(([k,v])=>[k,plain(v)]));
 return value;
}
function checkedFilter(check,where){
 const a=check.annotations;
 if(a?.representation){const kind=a.representation.id.replace('effect/schema/','');if(!['isMaxLength','isMinLength','isInt','isBetween','isPattern','isFinite','isGreaterThanOrEqualTo'].includes(kind))throw Error('Unsupported refinement '+kind);return {kind,...clone(a.representation.payload??{}),...(a.expected?{expected:a.expected}:{})};}
 if(a?.expected==='a non-blank string')return {kind:'nonBlank',expected:a.expected};
 if(a===undefined){
  if(where.includes('speechProjectCustomWords'))return {kind:'speechWords'};
  if(where.endsWith('.id')&&/(deviceHosts|easDeviceHosts|simlockDeviceHosts)/.test(where))return {kind:'configuredHostId'};
  if(/(?:deviceHosts|easDeviceHosts|simlockDeviceHosts)$/.test(where))return {kind:'uniqueHostIds'};
  if(where.endsWith('easDeviceHosts.*'))return {kind:'easBounds'};
 }
 throw Error('Unsupported settings filter at '+where+': '+JSON.stringify(a));
}
function transformKind(transformation,where){
 const decode=transformation.decode, encode=transformation.encode;
 const text=decode.transform?.toString()??decode._tag;
 const tag=(kind)=>{transformSources[kind]??={decode:text,encode:encode.transform?.toString()??encode._tag};return kind;};
 if(decode._tag==='Passthrough')return tag('identity');
 if(text.includes('Option.filter(o, Predicate.isNotUndefined)'))return tag('default');
 if(text.includes('Effect.succeed(trim(value))'))return tag('trim');
 if(text.includes('Effect.succeed(value || fallback)'))return tag('nonEmptyFallback');
 if(text.includes('instanceIdSource'))return tag('modelSelection');
 if(text.includes('coerceLegacyOptionsObjectToArray'))return tag('legacyOptions');
 if(text.includes('Option.isSome(decodeValue(raw))')&&text.includes('undefined'))return tag('safeOptional');
 if(text.includes('mode: value.length > 0 ? "custom"'))return tag('legacyWritingStyle');
 if(text.includes('values.filter((value) => typeof value === "string")'))return tag('stringArray');
 if(text.includes('Duration_.millis'))return tag('durationMillis');
 if(text.includes('Option.isSome(decodeValue(raw))')&&text.includes('null'))return tag('safeNullable');
 if(text.includes('Option.isSome(raw) && Option.isSome(decodeValue(raw.value))'))return tag('safeNullableKey');
 if(text.includes('mode === \"token\" ? \"paragraph\" : mode'))return tag('retiredStream');
 if(text.includes('secret === undefined ? variable'))return tag('environmentSecret');
 throw Error('Unsupported settings transform at '+where+': '+text);
}
function compile(ast,where){
 if(ids.has(ast))return ids.get(ast);
 const id=nodes.length;ids.set(ast,id);nodes.push(null);nodeAsts.push(ast);
 const node={tag:ast._tag,expected:NativeAnnotations.getExpected(ast)};
 if(ast.context?.isOptional)node.optional=true;
 if(ast.checks?.length)node.checks=ast.checks.map(check=>checkedFilter(check,where));
 switch(ast._tag){
  case 'String':case 'Number':case 'Boolean':case 'Null':case 'Undefined':case 'Unknown':break;
  case 'Literal':node.literal=clone(ast.literal);break;
  case 'Union':node.types=ast.types.map(type=>compile(type,where));break;
  case 'Arrays':
   node.elements=ast.elements.map(element=>compile(element,where+'.*'));
   node.rest=ast.rest.map(element=>compile(element,where+'.*'));break;
  case 'Objects':
   node.properties=ast.propertySignatures.map(({name,type})=>({name,type:compile(type,where?where+'.'+name:name)}));
   node.indexes=ast.indexSignatures.map(({parameter,type})=>({key:compile(parameter,where+'.#key'),value:compile(type,where+'.*')}));break;
  case 'Declaration':
   if(ast.annotations?.representation?.id!=='effect/schema/Duration')throw Error('Unknown declaration '+where);
   node.declaration='duration';break;
  default:throw Error('Unsupported AST '+ast._tag+' at '+where);
 }
 if(ast.encoding?.length){
  node.codecs=ast.encoding.map(({to,transformation})=>({from:compile(to,where),transform:transformKind(transformation,where)}));
  if(node.codecs.some(codec=>codec.transform==='nonEmptyFallback')){const link=ast.encoding.find(({transformation})=>transformation.decode.transform?.toString().includes('Effect.succeed(value || fallback)'));node.fallback=plain(Effect.runSync(link.transformation.decode.transform('')));}
  if(node.codecs.some(codec=>codec.transform==='default')){
   const link=ast.encoding.find(({transformation})=>transformation.decode.transform?.toString().includes('Option.filter(o, Predicate.isNotUndefined)'));
   const selected=Effect.runSync(link.transformation.decode.transform(Option.none()));
   const decoded=Option.isSome(selected)?selected.value:undefined;
   if(decoded===undefined)throw Error('Default must be explicit at '+where);
   node.default=plain(decoded);
  }
 }
 nodes[id]=node;return id;
}
const root=compile(C.ServerSettings.ast,'');
const defaults=plain(C.DEFAULT_SERVER_SETTINGS);
const encodedDefaults=S.encodeSync(C.ServerSettings)(C.DEFAULT_SERVER_SETTINGS);
const fields=Object.fromEntries(Object.entries(C.ServerSettings.fields).map(([name,schema])=>[name,compile(schema.ast,name)]));
const codecCases=[{label:'empty',empty:true}];
for(const [name,schema]of Object.entries(C.ServerSettings.fields)){
 const values=[null,true,false,0,1,-1,1.5,'',' ',' native ',[],{},['a'],{future:'x'}];
 for(const [index,value]of values.entries())addCase(name,value,name+':'+index);
 addCase(name,undefined,name+':missing');
}
function addCase(key,value,label){
 const input={...encodedDefaults};if(value===undefined)delete input[key];else input[key]=value;
 const item={label,key,...(value===undefined?{missing:true}:{value})};
 try{
  const decoded=plain(S.decodeUnknownSync(C.ServerSettings)(input));
  const encoded=S.encodeSync(C.ServerSettings)(S.decodeUnknownSync(C.ServerSettings)(input));
  item.decoded=key in decoded?{value:decoded[key]}:{missing:true};
  item.encoded=key in encoded?{value:encoded[key]}:{missing:true};
  for(const name of Object.keys(defaults))if(name!==key&&JSON.stringify(decoded[name])!==JSON.stringify(defaults[name]))throw Error('Cross-field codec change '+key+' -> '+name);
 }catch(error){if(String(error).includes('Cross-field'))throw error;item.invalid=true;}
 codecCases.push(item);
}
for(const value of [
 {provider:'codex',model:' model ',options:{b:true,a:'first'}},
 {instanceId:' custom ',model:' m ',options:[{id:' b ',value:true},{id:'a',value:1}]},
 {provider:'claudeAgent',instanceId:'codex',model:'x'},
])addCase('defaultModelSelection',value,'model:'+codecCases.length);
for(const value of [
 {' custom ':{driver:'codex'}},
 {custom:{driver:'codex',environment:[{name:' KEY ',value:'value'}]}},
 {custom:{driver:'codex',environment:[{name:'KEY',secret:{provider:'vault',reference:'key'}}]}},
])addCase('providerInstances',value,'provider:'+codecCases.length);
for(const value of [
 {mode:'custom'},
 {mode:'custom',customInstructions:'  exact  ',followChangeRequestTemplates:false},
 'legacy text',
])addCase('sourceControlWritingStyle',value,'style:'+codecCases.length);
for(const key of ['deviceHosts','easDeviceHosts','simlockDeviceHosts'])for(const id of ['node','local',' local ','node space']){
 const v=key==='deviceHosts'?{id,label:' Host ',target:'node'}:key==='easDeviceHosts'?{kind:'eas',id,label:'Host'}:{kind:'simlock',id,label:'Host',gatewayUrl:'https://example.com'};
 addCase(key,[v],key+':id:'+id);
 addCase(key,[v,v],key+':duplicate:'+id);
}
const native=await loadManagedSettingsNative(checkout,contracts,path.join(output,'native-runtime'),source);
const managedCases=[];
function delta(value){return difference(value,encodedDefaults,[]);}
function difference(value,base,path){
 if(JSON.stringify(value)===JSON.stringify(base))return [];
 if(value&&base&&typeof value==='object'&&typeof base==='object'&&!Array.isArray(value)&&!Array.isArray(base))return [...new Set([...Object.keys(base),...Object.keys(value)])].flatMap(key=>key in value?difference(value[key],base[key],[...path,key]):[{path:[...path,key],missing:true}]);
 return [{path,value}];
}
function managedCase(label,settings,maxRuntimeMode,current={},writes=[]){
 const item={label,settings,...(maxRuntimeMode?{maxRuntimeMode}:{}),current};
 const made=native.settings.makeManagedSettings(settings,maxRuntimeMode);
 if(made._tag==='Invalid'){item.invalid=made.reason;managedCases.push(item);return;}
 const policy=made.settings;
 item.defaults=delta(S.encodeSync(C.ServerSettings)(policy.defaults));
 item.lockedPaths=policy.lockedPaths;
 const decoded=S.decodeUnknownSync(C.ServerSettings)({...encodedDefaults,...current});
 item.applied=delta(S.encodeSync(C.ServerSettings)(policy.applyLocks(decoded)));
 item.fills=[{},null,{compactIdleThreads:true},{idleCompaction:'onReturn'},current].map(value=>({value,result:policy.withDefaults(value)}));
 item.writes=writes.map(({patch,next=patch})=>{
  const value=S.decodeUnknownSync(C.ServerSettings)({...S.encodeSync(C.ServerSettings)(decoded),...next});
  const verdict=policy.vetWrite(decoded,value,patch);
  return {patch,next,...(verdict._tag==='Locked'?{locked:verdict.path}:{allowed:delta(S.encodeSync(C.ServerSettings)(verdict.settings))})};
 });
 item.keeps=[];
 if(policy.keepStored)for(const stored of [{},current,{...current,compactIdleThreads:true},Object.fromEntries(Object.entries(settings).filter(([key,v])=>!v.locked&&!key.includes('.')).map(([key,v])=>[key,v.value]))])for(const atomic of [[],['backgroundActivity','providerInstances','storageCleanup']]){
  const settings=S.decodeUnknownSync(C.ServerSettings)(policy.withDefaults(stored));
  item.keeps.push({stored,atomic,result:plain(policy.keepStored({},settings,stored,key=>atomic.includes(key)))});
 }
 managedCases.push(item);
}
for(const [key,value]of Object.entries(encodedDefaults))for(const locked of [false,true])for(const input of [value,null,{invalid:true}])managedCase(key+':'+locked+':'+managedCases.length,{[key]:{value:input,locked}});
for(const path of ['', '.bad','bad.','notASetting','worktreeCleanup.afterDays','storageCleanup.worktreeAfterDays','providerInstances.custom.config.binaryPath','observability.otlpTracesUrl','compactIdleThreads','defaultModelSelection.model','favorites.x'])managedCase('path:'+path,{[path]:{value:true,locked:true}});
for(const mode of ['approval-required','auto-accept-edits','auto','full-access'])for(const locked of [false,true])managedCase('runtime:'+mode+':'+locked,{defaultRuntimeMode:{value:mode,locked}},'auto-accept-edits',{},[{patch:{defaultRuntimeMode:'full-access'}},{patch:{defaultRuntimeMode:'approval-required'}}]);
const scenarios=[
 {enableAgentBrowserAccess:{value:false,locked:true}},
 {worktreeCleanup:{value:null,locked:true}},
 {storageCleanup:{value:encodedDefaults.storageCleanup,locked:true},worktreeCleanup:{value:null,locked:true}},
 {idleCompaction:{value:'beforeCacheExpires',locked:true}},
 {idleCompaction:{value:'onReturn',locked:false}},
 {'storageCleanup.logsAfterDays':{value:30,locked:false}},
 {backgroundActivity:{value:{schemaVersion:1,profile:'balanced',overrides:{}},locked:false}},
 {'backgroundActivity.overrides.pauseWhenOnBattery':{value:true,locked:false}},
 {'providerInstances.custom':{value:{driver:'codex',enabled:false},locked:true}},
 {'providerInstances.custom':{value:{driver:'codex',enabled:false},locked:false}},
];
for(const [index,settings]of scenarios.entries())managedCase('scenario:'+index,settings,'auto',{
 enableAgentBrowserAccess:true,defaultRuntimeMode:'full-access',projectSettingsOverrides:{project:{enableAgentBrowserAccess:true,defaultRuntimeMode:'full-access',providerInstances:{custom:{driver:'codex',enabled:true}}}},providerInstances:{custom:{driver:'codex',enabled:true}},
},[
 {patch:{}},{patch:{enableAgentBrowserAccess:false}},{patch:{enableAgentBrowserAccess:true}},
 {patch:{projectSettingsOverrides:{project:{enableAgentBrowserAccess:true,defaultRuntimeMode:'full-access'}}}},
 {patch:{projectSettingsOverrides:{newProject:{enableAgentBrowserAccess:false,providerInstances:{custom:{driver:'codex',enabled:false}}}}}},
 {patch:{providerInstances:{custom:{driver:'codex',enabled:true}}}},
 {patch:{providerInstances:{custom:{enabled:false}}},next:{providerInstances:{custom:{driver:'codex',enabled:false}}}},
 {patch:{idleCompaction:'onReturn'}},{patch:{compactIdleThreads:false},next:{idleCompaction:'onReturn',compactIdleThreads:false}},
]);
for(const value of ['secret','••••••','',{provider:'vault',reference:'token'},{provider:' bad ',reference:'token'},{host:'secret'},{host:'••••••'},{host:{provider:'vault',reference:'token'}}]){
 for(const [driver,key]of [['antigravity','apiKey'],['opencode','serverPassword'],['codex','apiKey']])managedCase('provider-secret:'+managedCases.length,{'providerInstances.custom':{value:{driver,config:{[key]:value}},locked:true}});
 for(const [kind,key]of [['github','tokens'],['bitbucket','accessToken'],['gitcafe','token']])managedCase('host-secret:'+managedCases.length,{sourceControlHosts:{value:{[kind]:{[key]:value}},locked:true}});
}
for(const sensitive of [true,false])for(const valueRedacted of [true,false])for(const value of ['secret','',{provider:'vault',reference:'token'}])managedCase('environment:'+managedCases.length,{'providerInstances.custom':{value:{driver:'codex',environment:[{name:'KEY',value,sensitive,valueRedacted}]},locked:true}});
fs.writeFileSync(path.join(output,'managed-fixtures.json'),JSON.stringify({revision,effect:'4.1.2',cases:managedCases},null,2)+'\n');
const secretFields={providers:Object.fromEntries(native.secrets.providerSecretFields),hosts:Object.fromEntries(native.hosts.SOURCE_CONTROL_HOST_SECRET_FIELDS),usage:native.secrets.usageLimitSecretFields,eas:native.secrets.easDeviceHostSecretFields,simlock:native.secrets.simlockDeviceHostSecretFields};
const secretReference=compile(C.SecretReference.ast,'secretReference');
const patchRoot=compile(C.ServerSettingsPatch.ast,'patch');
const persistedRoot=compile(native.persisted.PersistedServerSettings.ast,'persisted');
const providerConfigRoots={codex:compile(C.CodexSettings.ast,'providerConfig.codex'),claudeAgent:compile(C.ClaudeSettings.ast,'providerConfig.claudeAgent')};
const providerInstanceRoot=compile(C.ProviderInstanceConfig.ast,'providerInstance');
const providerConfigDefaults={codex:plain(S.decodeUnknownSync(C.CodexSettings)({})),claudeAgent:plain(S.decodeUnknownSync(C.ClaudeSettings)({}))};
for(const file of ['apps/server/src/provider/Drivers/CodexDriver.ts','apps/server/src/provider/Drivers/ClaudeDriver.ts'])source(file);
const sparseCases=[{input:{},encoded:S.encodeSync(C.ServerSettings)({})}];
for(const key of Object.keys(encodedDefaults)){const value=C.DEFAULT_SERVER_SETTINGS[key];sparseCases.push({input:plain({[key]:value}),encoded:S.encodeSync(C.ServerSettings)({[key]:value})});}
fs.writeFileSync(path.join(output,'sparse-fixtures.json'),JSON.stringify(sparseCases,null,2)+'\n');
const providerConfigCases=[];
for(const [driver,schema]of Object.entries({codex:C.CodexSettings,claudeAgent:C.ClaudeSettings})){
 const base=S.encodeSync(schema)(S.decodeUnknownSync(schema)({}));
 for(const key of Object.keys(schema.fields))for(const value of [undefined,null,true,false,0,'',' native ',[],{},[{slug:' model ',name:' Name '}],[{slug:0}],{invalid:true}]){
  const input={...base};if(value===undefined)delete input[key];else input[key]=value;
  const c={driver,input};try{const decoded=S.decodeUnknownSync(schema)(input);c.decoded=plain(decoded);}catch(error){c.error=error.message;}providerConfigCases.push(c);
 }
 for(const input of [null,false,[],{}, {customModels:[{slug:' ',name:'n'}]}, ...[{}, {type:'select'}, {type:'boolean',id:0}, {type:'boolean',id:' ',label:'x'}].map(value=>({customModels:[{slug:'x',name:'n',capabilities:{optionDescriptors:[value]}}]}))]){const c={driver,input};try{c.decoded=plain(S.decodeUnknownSync(schema)(input));}catch(error){c.error=error.message;}providerConfigCases.push(c);}
}
fs.writeFileSync(path.join(output,'provider-config-fixtures.json'),JSON.stringify(providerConfigCases,null,2)+'\n');
const nodeValues=['gpu:\r','gpu:\u2028','valid\n','a\nb','\u0085',null,true,false,0,1,-1,1.5,9007199254740992,'',' ',' native ','\uFEFFok\uFEFF','node','local','onReturn',[],{},['a'],[null],{future:'x'},{id:'node',term:'word',aliases:[]},{provider:'vault',reference:'token'}];
const nodeCases=[];
for(const [index,ast]of nodeAsts.entries())for(const [valueIndex,value]of [...nodeValues,undefined].entries()){
 const item={node:index,valueIndex};
 try{const wrapped=S.Struct({value:S.make(ast)});const decoded=S.decodeUnknownSync(wrapped)(value===undefined?{}:{value});item.decoded=decoded.value===undefined?{missing:true}:{value:plain(decoded.value)};const encoded=S.encodeSync(wrapped)(decoded);item.encoded=encoded.value===undefined?{missing:true}:{value:plain(encoded.value)};}catch{item.invalid=true;}
 nodeCases.push(item);
}
fs.writeFileSync(path.join(output,'node-fixtures.json'),JSON.stringify({values:nodeValues,cases:nodeCases},null,2)+'\n');
const ir={providerInstanceRoot,providerConfigRoots,providerConfigDefaults,persistedRoot,patchRoot,secretReference,secretFields,revision,effect:'4.1.2',sources,root,fields,projectOverrideKeys:Object.keys(C.ProjectSettingsOverrides.fields),defaults,encodedDefaults,nodes,transformSources};
fs.writeFileSync(path.join(output,'codec.json'),JSON.stringify(ir,null,2)+'\n');
fs.writeFileSync(path.join(output,'codec-fixtures.json'),JSON.stringify({revision,effect:'4.1.2',cases:codecCases},null,2)+'\n');
console.log(nodes.length+' closed AST nodes; '+Object.keys(transformSources).length+' transforms; '+codecCases.length+' codec cases');
