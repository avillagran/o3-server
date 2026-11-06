#!/usr/bin/env node
// Execute the pinned resolver and 1Password algorithms against private synthetic boundaries.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output,override]=process.argv.slice(2);
if(!output)throw Error('usage: generate-secret-resolver-fixtures.mjs CHECKOUT CONTRACTS OUTPUT_DIRECTORY [REVISION]');
const revision=override??'6577c6b6185167c0e502fa01f9e8e8f2dee6ff25';
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
fs.mkdirSync(output,{recursive:true});
const runtime=path.join(output,'runtime');fs.mkdirSync(runtime,{recursive:true});
const sources={};
function source(file){const text=execFileSync('git',['-C',checkout,'show',revision+':'+file],{encoding:'utf8'});sources[file]=crypto.createHash('sha256').update(text).digest('hex');return text;}
const checkedContracts=new Set();
function checkContract(file){if(checkedContracts.has(file))return;checkedContracts.add(file);const relative=path.relative(checkout,file);const original=source(relative);if(fs.readFileSync(file,'utf8')!==original)throw Error('Contract source mismatch: '+relative);for(const match of original.matchAll(/\bfrom\s+["'](\.[^"']+)["']/g))checkContract(path.resolve(path.dirname(file),match[1]));}
checkContract(contracts+'/src/index.ts');
for(const file of ['packages/shared/src/shell.ts','packages/provider-core/src/server/snapshotProbe.ts','packages/secret-provider-core/src/server/SecretResolver.test.ts','packages/secret-provider-onepassword/src/server/driver.test.ts','packages/secret-provider-onepassword/src/shared/definition.test.ts'])source(file);
const visited=new Map();
const substrate=pathToFileURL(path.join(runtime,'substrate.mjs')).href;
function resolve(specifier,from){
 if(specifier==='@t3tools/contracts')return pathToFileURL(contracts+'/src/index.ts').href;
 if(['@t3tools/provider-core/server/snapshotProbe','@t3tools/shared/shell'].includes(specifier))return substrate;
 if(specifier.startsWith('@t3tools/')){const[pkg,...parts]=specifier.slice(9).split('/');const dir=path.join(checkout,'packages',pkg);const manifest=JSON.parse(fs.readFileSync(dir+'/package.json','utf8'));let file=manifest.exports[parts.length?'./'+parts.join('/'):'.'];if(typeof file==='object')file=file.import??file.default;if(typeof file!=='string')throw Error('Unknown package export '+specifier);return write(path.resolve(dir,file));}
 if(specifier.startsWith('.'))return write(path.resolve(path.dirname(from),specifier));
 if(specifier.startsWith('node:'))return specifier;
 return pathToFileURL(require.resolve(specifier)).href;
}
function write(file){if(visited.has(file))return visited.get(file);const target=path.join(runtime,'native-'+visited.size+'.mjs');const url=pathToFileURL(target).href;visited.set(file,url);let text=stripTypeScriptTypes(source(path.relative(checkout,file)),{mode:'strip'});text=text.replace(/\b(from\s*|import\s*)(["'])([^"']+)\2/g,(_,prefix,q,specifier)=>prefix+q+resolve(specifier,file)+q);fs.writeFileSync(target,text);return url;}
const effectUrl=pathToFileURL(require.resolve('effect/Effect')).href;
fs.writeFileSync(path.join(runtime,'substrate.mjs'),`import * as Effect from ${JSON.stringify(effectUrl)};\nexport const state={calls:[],templates:new Map(),answer:()=>({code:0,stdout:'',stderr:''})};\nexport const resolveSpawnCommand=(command,args)=>Effect.succeed({command,args,shell:false});\nexport const spawnAndCollect=(_binary,command)=>Effect.suspend(()=>{state.calls.push([...command.args]);const result=state.answer(command.args);return result.hang?Effect.never:Effect.succeed(result);});\n`);
const Effect=await import(effectUrl);
const Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const Clock=await import(pathToFileURL(require.resolve('effect/Clock')).href);
const FileSystem=await import(pathToFileURL(require.resolve('effect/FileSystem')).href);
const SecureRandom=await import(pathToFileURL(require.resolve('effect/crypto/SecureRandom')).href);
const C=await import(pathToFileURL(contracts+'/src/index.ts').href);
const Resolver=await import(write(path.join(checkout,'packages/secret-provider-core/src/server/SecretResolver.ts')));
const {driver}=await import(write(path.join(checkout,'packages/secret-provider-onepassword/src/server/driver.ts')));
const {definition,OnePasswordSecretProviderConfig}=await import(write(path.join(checkout,'packages/secret-provider-onepassword/src/shared/definition.ts')));
const {defineSecretProviderDriver}=await import(write(path.join(checkout,'packages/secret-provider-core/src/server/driver.ts')));
const {state}=await import(substrate);
const referenceInputs=[null,{},true,'op://a/b/c',{provider:' personal ',reference:' op://vault/item/field '},{provider:'1Password',reference:'op://v/i/f'},...['','bad id','-bad','a'.repeat(65),'\uFEFFok\uFEFF'].map(provider=>({provider,reference:'op://v/i/f'})),...['','  ','a\nb','\u0085','x'.repeat(1024),'x'.repeat(1025)].map(reference=>({provider:'p',reference}))];
const referenceCases=referenceInputs.map(input=>{try{return {input,decoded:Schema.decodeUnknownSync(C.SecretReference)(input)}}catch{return {input,invalid:true}}});
const accounts=['my.1password.com','work','1','-bad','a b',' a','a ','','a'.repeat(253),'a'.repeat(254),'a_b.c-d','\n','a\n',null,7];
const accountCases=accounts.map(account=>{const input={account,extra:'discarded'};try{return {input,decoded:Schema.decodeUnknownSync(OnePasswordSecretProviderConfig)(input)}}catch{return {input,invalid:true}}});
const references=['op://vault/item/field','op://vault/item/section/field','op://Private/My API/key','op://a/b/c?attribute=password','op://a/b/c?attr-name=VALUE_1','op://a/b/c?attr=bad value','op://a/b','op://a/b/c/d/e','op:// a/b/c','op://a/b /c','op://a/b/c\n','op://a/{x}/c','op://a/b/c?Attr=ok','op://a/b/c?x=a&y=b','OP://a/b/c',...['','a','a'.repeat(340),'a'.repeat(341)].map(v=>`op://${v}/${v}/${v}`)];
const syntaxCases=references.map(reference=>({reference,valid:definition.isReference(reference)}));
const coreCases=[];
const ref=(reference,provider='personal')=>({provider,reference});
const provider=(account='a',kind='fake',label=account)=>({kind,label,config:{account}});
async function coreCase(label,providers,events){
 let current=providers;let responses={};const calls=[];
 const drivers=['fake','other'].map(kind=>defineSecretProviderDriver({kind,definition:{kind,label:kind,configSchema:Schema.Struct({account:Schema.String}),isReference:r=>r.startsWith('fake://')},read:({account},references)=>Effect.sync(()=>{calls.push({kind,account,references});return references.map(reference=>responses[account+'|'+reference]);})}));
 const resolver=await Effect.runPromise(Resolver.make({drivers,secretProviders:Effect.sync(()=>current)}));
 const item={label,providers,events:[]};
 for(const event of events){
  const op={...event};
  if('responses'in event)responses=event.responses;
  if('providers'in event)current=event.providers;
  if(event.read)op.result=(await Effect.runPromise(resolver.read(event.read))).map(value=>value??null);
  if(event.invalidate!==undefined)await Effect.runPromise(resolver.invalidate(event.invalidate===null?undefined:event.invalidate));
  if(event.label!==undefined)op.result=await Effect.runPromise(resolver.label(event.label));
  op.calls=structuredClone(calls);item.events.push(op);
 }
 coreCases.push(item);
}
await coreCase('success and failure cache',{personal:provider()},[
 {responses:{'a|fake://one':' first ','a|fake://two':''},read:[ref('fake://one'),ref('fake://two'),ref('fake://missing'),ref('fake://one')]},
 {responses:{'a|fake://one':'changed','a|fake://missing':'now'},read:[ref('fake://one'),ref('fake://missing')]},
 {invalidate:[ref('fake://missing')]},{read:[ref('fake://one'),ref('fake://missing')]},{invalidate:[]},{read:[ref('fake://one')]},{invalidate:null},{read:[ref('fake://one')]},
]);
await coreCase('providers config labels invalid plans',{personal:provider(),work:provider('b'),broken:{kind:'fake',label:'Broken',config:{}},unknown:provider('a','unknown')},[
 {responses:{'a|fake://one':'a','b|fake://one':'b'},read:[ref('fake://one','work'),ref('fake://two'),ref('fake://one'),ref('fake://one','work'),ref('bad'),ref('fake://one','broken'),ref('fake://one','unknown'),ref('fake://one','gone')]},
 {providers:{personal:{...provider(),config:{account:'a',extra:'ignored'}},work:provider('b')},read:[ref('fake://one')]},
 {providers:{personal:provider('b','fake','  Work label  ')},read:[ref('fake://one')]},{label:'personal'},{label:'missing'},
 {providers:{personal:provider('b','other',' ')},read:[ref('fake://one')]},{label:'personal'},
 {providers:{},read:[ref('fake://one')]},{providers:{personal:provider('a')},read:[ref('fake://one')]},
]);
for(const refs of [[ref('fake://one')],[ref('fake://one'),ref('fake://one')],[ref('fake://one','work'),ref('fake://two'),ref('fake://one')],[]])for(const failed of [false,true])await coreCase('ordered group:'+coreCases.length,{personal:provider(),work:provider('b')},[{responses:failed?{}:{'a|fake://one':'a1','a|fake://two':'a2','b|fake://one':'b1'},read:refs},{read:refs},{invalidate:refs.slice(0,1)},{read:refs}]);
const driverCases=[];
async function driverCase(label,config,refs,answers){
 state.calls=[];state.templates=new Map();let templateIndex=0;
 const fakeFs=FileSystem.makeNoop({makeTempFileScoped:()=>Effect.succeed('/tmp/template-'+templateIndex++),writeFileString:(file,text)=>Effect.sync(()=>state.templates.set(file,text))});
 state.answer=args=>{const stage=args[1];const result=stage==='inject'?answers.inject:answers.read?.[args.at(-1)];if(result?.substitute){const template=state.templates.get(args[args.indexOf('-i')+1]);return {code:result.code??0,stdout:template.replace(/\{\{ (.*?) \}\}/g,(_,reference)=>result.substitute[reference]??'')+(result.trailing??''),stderr:''};}return {stdout:'',stderr:'',code:0,...result};};
 const random=SecureRandom.make(size=>new Uint8Array(size));
 let effect=driver.read(config,refs).pipe(Effect.provideService(FileSystem.FileSystem,fakeFs),Effect.provideService(SecureRandom.SecureRandom,random));
 if(answers.virtualTimeout){const live=await Effect.runPromise(Clock.Clock);effect=effect.pipe(Effect.provideService(Clock.Clock,new Proxy(live,{get:(target,key)=>key==='sleep'?()=>Effect.void:typeof target[key]==='function'?target[key].bind(target):target[key]})));}
 const values=await Effect.runPromise(effect);
 driverCases.push({label,config,references:refs,answers,result:values.map(v=>v??null),calls:state.calls.map(args=>args.map(arg=>arg.startsWith('/tmp/template-')?'TEMPLATE':arg)),templates:[...state.templates.values()].map(value=>value.replace(/__t3-secret-[a-f0-9-]+__/g,'SEPARATOR'))});
}
const one='op://Private/One/key',two='op://Private/Two/key';
for(const stdout of ['literal',' leading\n',' ','','\u0085','\uFEFF'])await driverCase('single:'+driverCases.length,{account:'my.1password.com'},[one],{read:{[one]:{stdout}}});
await driverCase('single failure',{account:'my.1password.com'},[one],{read:{[one]:{stdout:'ignored',code:2}}});
await driverCase('invalid account',{account:'-bad'},[one],{read:{[one]:{stdout:'ignored'}}});
await driverCase('empty references',{account:'work'},[],{});
for(const inject of [{substitute:{[one]:'a',[two]:' b\n'}},{substitute:{[one]:'a',[two]:''},trailing:'\n'},{substitute:{[one]:'a',[two]:'b'},code:1},{stdout:'broken'},{substitute:{[one]:'a',[two]:'b'},trailing:'unexpected'}])await driverCase('batch:'+driverCases.length,{account:'work'},[one,two],{inject,read:{[one]:{stdout:'first'},[two]:{code:1}}});
await driverCase('batch virtual timeout',{account:'work'},[one,two],{virtualTimeout:true,inject:{hang:true},read:{[one]:{stdout:'must not read'}}});
const fixture={revision,effect:'4.1.2',sources,referenceCases,accountCases,syntaxCases,coreCases,driverCases};
fs.writeFileSync(path.join(output,'secret-resolver.json'),JSON.stringify(fixture,null,2)+'\n');
console.log(`${referenceCases.length} references; ${accountCases.length} accounts; ${syntaxCases.length} native syntax cases; ${coreCases.length} resolver sequences; ${driverCases.length} native driver traces`);
