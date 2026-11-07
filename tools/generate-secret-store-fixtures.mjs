#!/usr/bin/env node
// Execute native methods with an in-memory filesystem and deterministic random service only.
import fs from'node:fs';import path from'node:path';import crypto from'node:crypto';import{execFileSync}from'node:child_process';import{createRequire}from'node:module';import{pathToFileURL}from'node:url';import{loadSecretStoreNative}from'./secret-store-native.mjs';
const[checkout,contracts,output]=process.argv.slice(2);if(!output)throw Error('usage: generate-secret-store-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',require=createRequire(contracts+'/package.json');if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');fs.mkdirSync(output,{recursive:true});const sources={};
function source(file){const value=execFileSync('git',['-C',checkout,'show',revision+':'+file],{encoding:'utf8'});sources[file]=crypto.createHash('sha256').update(value).digest('hex');return value;}
const checked=new Set();function check(file){if(checked.has(file))return;checked.add(file);const value=source(path.relative(checkout,file));if(fs.readFileSync(file,'utf8')!==value)throw Error('Contract source mismatch '+file);for(const m of value.matchAll(/\bfrom\s+["'](\.[^"']+)["']/g))check(path.resolve(path.dirname(file),m[1]));}check(contracts+'/src/index.ts');
const load=async name=>import(pathToFileURL(require.resolve('effect/'+name)).href);const Effect=await load('Effect'),Option=await load('Option'),FileSystem=await load('FileSystem'),SecureRandom=await load('crypto/SecureRandom'),Path=await load('Path'),PlatformError=await load('PlatformError');const native=await loadSecretStoreNative(checkout,contracts,path.join(output,'runtime'),source);
const directory='/private-fixture/secrets';
function model(input={}){
 const files=new Map(Object.entries(input.files??{}).map(([name,value])=>[name,{bytes:[...value.bytes],mode:value.mode??384}])),dirs=new Map(),trace=[],counts={};let uuid=0,random=0;
 const failure=(method,target,reason='PermissionDenied')=>PlatformError.systemError({_tag:reason,module:'FileSystem',method,pathOrDescriptor:target,description:'Synthetic filesystem failure.'});
 const op=(method,parameters,work)=>Effect.suspend(()=>{trace.push({method,...parameters});counts[method]=(counts[method]??0)+1;const fault=[...(input.fails??[]),...(input.fail?[input.fail]:[])].find(f=>f.method===method&&(f.occurrence??1)===counts[method]);const fail=Boolean(fault);if(method==='readFile'&&input.disappearAfterRace&&counts.readFile===2)files.delete(parameters.path);if(fail&&!fault.after)return Effect.fail(failure(method,parameters.path??parameters.to,fault.reason));let value;try{value=work();}catch(error){return Effect.fail(error);}return fail?Effect.fail(failure(method,parameters.path??parameters.to,fault.reason)):Effect.succeed(value);});
 const fileSystem=FileSystem.makeNoop({
  makeDirectory:(p,options)=>op('makeDirectory',{path:p,options},()=>{dirs.set(p,493);}),
  chmod:(p,mode)=>op('chmod',{path:p,mode},()=>{if(dirs.has(p))dirs.set(p,mode);else if(files.has(p))files.get(p).mode=mode;else throw failure('chmod',p,'NotFound');}),
  readFile:p=>op('readFile',{path:p},()=>{if(!files.has(p))throw failure('readFile',p,'NotFound');return new Uint8Array(files.get(p).bytes);}),
  writeFile:(p,bytes)=>op('writeFile',{path:p,value:Array.from(bytes)},()=>{files.set(p,{bytes:Array.from(bytes),mode:420});}),
  rename:(from,to)=>op('rename',{from,to},()=>{if(!files.has(from))throw failure('rename',from,'NotFound');files.set(to,files.get(from));files.delete(from);}),
  link:(from,to)=>op('link',{from,to},()=>{if(input.raceWinner)files.set(to,{bytes:input.raceWinner,mode:384});if(files.has(to))throw failure('link',to,'AlreadyExists');if(!files.has(from))throw failure('link',from,'NotFound');files.set(to,files.get(from));}),
  remove:(p,options)=>op('remove',{path:p,...(options?{options}:{})},()=>{if(options?.recursive)throw Error('Recursive deletion is forbidden in the oracle.');if(!files.has(p))throw failure('remove',p,'NotFound');files.delete(p);}),
  open:(p,options)=>op('open',{path:p,options},()=>{if(files.has(p))throw failure('open',p,'AlreadyExists');files.set(p,{bytes:[],mode:options.mode});return{writeAll:bytes=>op('writeAll',{path:p,value:Array.from(bytes)},()=>{files.get(p).bytes=Array.from(bytes);}),sync:op('sync',{path:p},()=>{})};}).pipe(Effect.tap(()=>Effect.addFinalizer(()=>op('close',{path:p},()=>{})))),
 });
 const randomService={uuidV4:()=>Effect.suspend(()=>{trace.push({method:'uuidV4'});if(input.fail?.method==='uuidV4')return Effect.fail(new Error('Synthetic UUID failure'));return Effect.succeed('00000000-0000-4000-8000-'+String(++uuid).padStart(12,'0'));}),randomBytes:size=>Effect.suspend(()=>{trace.push({method:'randomBytes',size});if(input.fail?.method==='randomBytes')return Effect.fail(new Error('Synthetic random failure'));return Effect.succeed(new Uint8Array(size).fill(++random));})};
 const run=program=>Effect.runPromise(Effect.scoped(program).pipe(Effect.provideService(FileSystem.FileSystem,fileSystem),Effect.provideService(SecureRandom.SecureRandom,randomService),Effect.provideService(native.config.ServerConfig,{secretsDir:directory}),Effect.provide(Path.layer)));
 const snapshot=()=>({files:Object.fromEntries([...files].sort(([a],[b])=>a.localeCompare(b))),directories:Object.fromEntries(dirs)});return{run,trace,snapshot};
}
function resultValue(result){if(result._tag==='Success'){const v=result.success;if(Option.isOption(v))return{value:Option.isSome(v)?Array.from(v.value):null};return{value:v instanceof Uint8Array?Array.from(v):v??null};}const e=result.failure;return{error:{tag:e._tag,message:e.message,...(e.resource?{resource:e.resource}:{}),...(e.operation?{operation:e.operation}:{}),...(e.providerInstanceId?{providerInstanceId:e.providerInstanceId}:{}),...(e.environmentVariable?{environmentVariable:e.environmentVariable}:{}),...(e.cause?{causeTag:e.cause._tag,causeReason:e.cause.reason?._tag}:{})}};}
const cases=[];
async function add(label,operation,input={}){const state=model(input);const result=await state.run(Effect.result(Effect.gen(function*(){const store=yield*native.store.make;switch(operation){case'get':return yield*store.get('slot');case'set':return yield*store.set('slot',new Uint8Array([10,20,255]));case'create':return yield*store.create('slot',new Uint8Array([10,20,255]));case'random':return yield*store.getOrCreateRandom('slot',4);case'remove':return yield*store.remove('slot');default:throw Error(operation);}})));cases.push({label,operation,input,result:resultValue(result),trace:state.trace,state:state.snapshot()});}
for(const operation of['get','set','create','random','remove'])for(const existing of[false,true])await add(operation+':'+existing,operation,{files:existing?{[directory+'/slot.bin']:{bytes:[7,8],mode:384}}:{}});
for(const[operation,fail]of[['set',{method:'uuidV4'}],['create',{method:'uuidV4'}],['random',{method:'randomBytes'}],['get',{method:'readFile'}],['set',{method:'writeFile'}],['set',{method:'chmod',occurrence:2}],['set',{method:'rename'}],['set',{method:'chmod',occurrence:3}],['create',{method:'writeAll'}],['create',{method:'sync'}],['create',{method:'link'}],['remove',{method:'remove'}],['get',{method:'chmod',occurrence:1}],['get',{method:'makeDirectory'}]])await add('failure:'+cases.length,operation,{files:operation==='random'?{}:{[directory+'/slot.bin']:{bytes:[7,8],mode:384}},fail});

await add('random:concurrent-winner','random',{raceWinner:[88,89,90]});
await add('random:concurrent-disappeared','random',{raceWinner:[88],disappearAfterRace:true});
await add('create:existing-temporary-path','create',{files:{[directory+'/slot.bin.00000000-0000-4000-8000-000000000001.tmp']:{bytes:[66],mode:384}}});
const seedPath=new URL('../tests/fixtures/settings-secrets-4288.json',import.meta.url);const seedText=fs.readFileSync(seedPath,'utf8'),seed=JSON.parse(seedText);if(seed.revision!==revision)throw Error('Secret mapping fixture pin changed');
const empty={providerInstances:{},sourceControlHosts:{},usageLimitSources:{},easDeviceHosts:[],simlockDeviceHosts:[],secretProviders:{}};
const normalize=value=>JSON.parse(JSON.stringify(value,(_k,v)=>v instanceof Uint8Array?Array.from(v):v));
const transactionCases=[];
for(const row of seed.cases){
 const current=row.input;
 const nextVariants=[['retain',current],['remove',empty],['empty',native.settings.secrets.mapSettingsSecrets(current,()=>({value:'',redacted:false}))],['echo',native.settings.secrets.mapSettingsSecrets(current,()=>({value:'',redacted:true}))],['replace',native.settings.secrets.mapSettingsSecrets(current,()=>({value:'replacement-\u2603',redacted:false}))]];
 for(const[operation,next]of nextVariants){
  const state=model();
  const result=await state.run(Effect.gen(function*(){const store=yield*native.store.make;const methods=native.methods.transactionMethods(store,'/private-fixture/settings.json',native.settings.secrets,native.definitions);const plan=yield*methods.plan(current,next);return{settings:plan.settings,changes:plan.changes,redactedModern:methods.redact(plan.settings,{secretReferences:true}),redactedLegacy:methods.redact(plan.settings,{secretReferences:false})};}));
  transactionCases.push({label:row.label+':'+operation,current,next,...normalize(result)});
 }
}
const applyCases=[];
const changes=[{kind:'write',secretName:'first',value:[10,20],providerInstanceId:'synthetic',environmentVariable:'TOKEN'},{kind:'remove',secretName:'second',operation:'remove-stale-secret'},{kind:'write',secretName:'third',value:[30]}];
for(const variant of ['success','rollback','missing','read-failure','write-failure','after-mutation','second-remove-failure','rollback-failure']){
 const input={files:variant==='missing'?{}:{[directory+'/first.bin']:{bytes:[1],mode:384},[directory+'/second.bin']:{bytes:[2],mode:384}}};
 if(variant==='read-failure')input.fail={method:'readFile',occurrence:2};
 if(variant==='write-failure')input.fail={method:'writeFile',occurrence:2};
 if(variant==='after-mutation')input.fail={method:'chmod',occurrence:3};
 if(variant==='second-remove-failure')input.fail={method:'remove',occurrence:1};
 if(variant==='rollback-failure')input.fails=[{method:'chmod',occurrence:3,after:true},{method:'writeFile',occurrence:2}];
 const state=model(input);const result=await state.run(Effect.result(Effect.gen(function*(){const store=yield*native.store.make;const methods=native.methods.transactionMethods(store,'/private-fixture/settings.json',native.settings.secrets,native.definitions);const rollback=yield*methods.apply(changes.map(c=>({...c,...(c.value?{value:new Uint8Array(c.value)}:{})})));const applied=state.snapshot();if(variant==='rollback')yield*rollback;return{applied};})));
 applyCases.push({label:variant,input,changes,result:normalize(resultValue(result)),trace:state.trace,state:state.snapshot()});
}
const materializeCases=[],materializeErrors=[];
for(const row of seed.cases){
 const settings=native.settings.secrets.mapSettingsSecrets(row.input,secret=>({value:secret.sensitive?'':secret.value,redacted:secret.sensitive}));
 for(const variant of['missing','stored','malformed-utf8']){
  const files={};native.settings.secrets.mapSettingsSecrets(settings,secret=>{if(variant!=='missing'&&secret.sensitive&&secret.redacted)files[directory+'/'+secret.secretName+'.bin']={bytes:variant==='stored'?Array.from(new TextEncoder().encode('private-\u2603')):[239,187,191,65,255,66],mode:384};return secret;});
  const state=model({files});const result=await state.run(Effect.gen(function*(){const store=yield*native.store.make;return yield*native.methods.transactionMethods(store,'/private-fixture/settings.json',native.settings.secrets,native.definitions).materialize(settings);}));
  materializeCases.push({label:row.label+':'+variant,settings,files,result:normalize(result),trace:state.trace});
  if(variant==='stored'&&Object.keys(files).length){const failed=model({files,fail:{method:'readFile'}});const error=await failed.run(Effect.result(Effect.gen(function*(){const store=yield*native.store.make;return yield*native.methods.transactionMethods(store,'/private-fixture/settings.json',native.settings.secrets,native.definitions).materialize(settings);})));materializeErrors.push({label:row.label,settings,files,result:normalize(resultValue(error))});}
 }
}
fs.writeFileSync(path.join(output,'fixtures.json'),JSON.stringify({revision,effect:'4.1.2',sources,directory,knownKinds:native.knownKinds,seedSha256:crypto.createHash('sha256').update(seedText).digest('hex'),cases,transactionCases,applyCases,materializeCases,materializeErrors},null,2)+'\n');console.log(JSON.stringify({store:cases.length,planning:transactionCases.length,apply:applyCases.length,materialize:materializeCases.length,materializeErrors:materializeErrors.length}));
