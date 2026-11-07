#!/usr/bin/env node
// Execute the complete pinned native DeviceService with controlled hosts and real Effect concurrency.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output,dependencies]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-device-native-state-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87';
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const directory=path.dirname(output);fs.mkdirSync(directory,{recursive:true});
const sources={},loaded=new Map();
function module(file){
 if(loaded.has(file))return loaded.get(file);
 const target=path.join(directory,crypto.createHash('sha256').update(file).digest('hex')+(file.endsWith('.json')?'.json':'.mjs'));
 loaded.set(file,target);
 const source=execFileSync('git',['-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'});
 sources[file]=crypto.createHash('sha256').update(source).digest('hex');
 if(file.endsWith('.json')){fs.writeFileSync(target,source);return target;}
 const code=stripTypeScriptTypes(source).replace(/from\s+(["'])([^"']+)\1/g,(all,quote,specifier)=>{
  let destination;
  if(specifier==='@t3tools/contracts')destination=contracts+'/src/index.ts';
  else if(specifier.startsWith('effect/'))destination=require.resolve(specifier);
  else if(specifier.startsWith('./')||specifier.startsWith('../'))destination=module(path.posix.normalize(path.posix.join(path.posix.dirname(file),specifier)));
  else if(specifier.startsWith('@t3tools/')){
   const [,pkg,entry]=specifier.match(/^@t3tools\/([^/]+)(?:\/(.+))?$/)??[];
   if(!pkg)throw Error('Unhandled package '+specifier);
   destination=module(`packages/${pkg}/src/${entry??'index'}.ts`);
  }else if(specifier==='diff') {
   if(!dependencies)throw Error('Pinned diff dependency directory required');
   const folder=path.join(dependencies,'node_modules/diff');const metadata=JSON.parse(fs.readFileSync(path.join(folder,'package.json'),'utf8'));if(metadata.version!=='8.0.3')throw Error('diff8.0.3 required');destination=path.join(folder,metadata.module??metadata.main);
  }else return all;
  return 'from '+JSON.stringify(pathToFileURL(destination).href);
 });
 fs.writeFileSync(target,code);return target;
}
const native=await import(pathToFileURL(module('apps/server/src/device/DeviceService.ts')).href);
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const Digest=await import(pathToFileURL(require.resolve('effect/crypto/Digest')).href);
const TestClock=await import(pathToFileURL(require.resolve('effect/testing/TestClock')).href);
const Deferred=await import(pathToFileURL(require.resolve('effect/Deferred')).href);
const Fiber=await import(pathToFileURL(require.resolve('effect/Fiber')).href);
const PubSub=await import(pathToFileURL(require.resolve('effect/PubSub')).href);
const Settings=await import(pathToFileURL(module('apps/server/src/serverSettings.ts')).href);
const contractsModule=await import(pathToFileURL(contracts+'/src/index.ts').href);
const copy=value=>JSON.parse(JSON.stringify(value));
const ios={id:'sim-old',name:'Old iPhone',version:'iOS 26',platform:'ios',booted:true,physical:false};
const android={id:'emulator-5554',name:'Old Pixel',version:'Android 16',platform:'android',booted:true,physical:false};
async function refreshCase(input){
 let settings={...contractsModule.DEFAULT_SERVER_SETTINGS,enableDeviceSupport:true,enableAgentDeviceAccess:false};
 let listing=copy(input.initial);let gate=null;const id=input.hostId;const summary={id,kind:id==='local'?'local':'ssh',label:'Fixture host',platforms:[{platform:'ios',available:true},{platform:'android',available:true}],hubInstalled:true,agentDeviceInstalled:false};
 const ready={nodePath:'/fixture/node',hub:{origin:'http://device.test'},devices:{list:Effect.suspend(()=>{const captured=copy(listing);if(gate?.next){gate.next=false;return Effect.gen(function*(){yield* Deferred.succeed(gate.entered,undefined);yield* Deferred.await(gate.release);return captured;});}return Effect.succeed(captured);}),boot:device=>Effect.succeed(device.id),attach:()=>Effect.void,shutdown:()=>Effect.void,screenshot:()=>Effect.succeed(new Uint8Array()),endsDevices:input.endsDevices},run:()=>Effect.succeed({code:0,stdout:' V....D libx264',stderr:''}),helpers:{serveSimAxSettings:null,serveSimCli:null}};
 const host={id,label:'Fixture host',summary:Effect.succeed(summary),platformAvailability:platform=>Effect.succeed({platform,available:true}),ensureReady:()=>Effect.succeed(ready),current:Effect.succeed(ready),stopAgent:Effect.void,stop:Effect.void};
 const hosts=new Map([[id,host]]);
 const settingService={getSettings:Effect.sync(()=>settings),updateSettings:patch=>Effect.sync(()=>settings={...settings,...patch})};
 const program=Effect.gen(function*(){
  const service=yield* native.makeWithHosts(hosts);
  yield* service.list;
  if(input.seedSessions)for(const device of input.initial.devices)yield* service.open({hostId:id,deviceId:device.id,platform:device.platform,threadId:'thread-'+device.id,boot:false});
  const before=copy(yield* service.state);listing=copy(input.next);let current=before;let after;
  if(input.race){
   gate={next:true,entered:yield* Deferred.make(),release:yield* Deferred.make()};const pending=yield* Effect.forkChild(service.list);yield* Deferred.await(gate.entered);
   if(input.race==='disabled')settings={...settings,enableDeviceSupport:false};
   if(input.race==='host-replaced'){hosts.set(id,{...host,summary:Effect.succeed({...summary,label:'Replacement host'})});yield* service.refreshHosts;}
   if(input.race==='new-session'){const device={...ios,id:'sim-concurrent'};listing={devices:[...input.initial.devices,device]};yield* service.open({hostId:id,deviceId:device.id,platform:device.platform,threadId:'thread-new',boot:false});}
   if(input.race==='reopened-session'){const device=input.initial.devices[0];yield* service.close({threadId:'thread-'+device.id,hostId:id,deviceId:device.id});yield* TestClock.adjust(1);listing=copy(input.initial);yield* service.open({hostId:id,deviceId:device.id,platform:device.platform,threadId:'thread-'+device.id,boot:false});}
   current=copy(yield* service.state);yield* Deferred.succeed(gate.release,undefined);after=copy(yield* Fiber.join(pending));
  }else{after=copy(yield* service.list);}
  const transitions=[];
  if(!input.race)for(const status of [{status:'starting'},{status:'ready'}]){const current=copy(yield* service.state);const state=copy(yield* service.setHostStatus(id,status));transitions.push({current,status,expected:state});}
  return {scenario:input.race??'refresh',input:{current,hostId:id,listing:input.next,endsDevices:input.endsDevices,sessionsBefore:before.sessions,enabled:input.race!=='disabled',hostUnchanged:input.race!=='host-replaced'},expected:after,transitions};
 }).pipe(Effect.provideService(Settings.ServerSettingsService,settingService),Effect.provide(Digest.layerWebCrypto),Effect.provide(TestClock.layer()),Effect.scoped);
 return await Effect.runPromise(program);
}
const cases=[];
for(const hostId of ['local','fixture-ssh'])for(const endsDevices of [false,true])for(const incompletePlatforms of [[],['ios'],['android'],['ios','android']])for(const androidDiscoveryIncomplete of [false,true])for(const detail of [undefined,'Discovery warning']){
 const input={hostId,endsDevices,seedSessions:true,initial:{devices:[ios,android]},next:{devices:[{...android,id:'emulator-5560',name:'New Pixel'},{...ios,id:'sim-new',name:'New iPhone'}],incompletePlatforms,androidDiscoveryIncomplete,...(detail===undefined?{}:{detail})}};
 cases.push(await refreshCase(input));
}
for(const hostId of ['local','fixture-ssh'])for(const endsDevices of [false,true])for(const race of ['disabled','host-replaced','new-session','reopened-session'])cases.push(await refreshCase({hostId,endsDevices,race,seedSessions:true,initial:{devices:[ios,android]},next:{devices:[]}}));
async function bootCase(input){
 let settings={...contractsModule.DEFAULT_SERVER_SETTINGS,enableDeviceSupport:true,enableAgentDeviceAccess:false};let listing={devices:[input.initialDevice]};const calls=[];const id=input.hostId;
 const summary={id,kind:id==='local'?'local':'ssh',label:'Fixture host',platforms:[{platform:'ios',available:true},{platform:'android',available:true}],hubInstalled:true,agentDeviceInstalled:false};
 const ready={nodePath:'/fixture/node',hub:{origin:'http://device.test'},devices:{list:Effect.sync(()=>copy(listing)),boot:device=>Effect.suspend(()=>{calls.push({type:'boot',id:device.id});if(input.bootFailure)return Effect.fail(new contractsModule.DeviceBootError({hostId:id,deviceId:device.id,reason:'launch_failed',cause:{fixture:true}}));listing=copy(input.afterBoot);return Effect.succeed(input.returnedId);}),attach:device=>Effect.sync(()=>calls.push({type:'attach',id:device.id})),shutdown:()=>Effect.void,screenshot:()=>Effect.succeed(new Uint8Array())},run:(command,args)=>Effect.sync(()=>{calls.push({type:'run',command,args});return {code:0,stdout:' V....D libx264',stderr:''};}),helpers:{serveSimAxSettings:null,serveSimCli:null}};
 const host={id,label:'Fixture host',summary:Effect.succeed(summary),platformAvailability:platform=>Effect.succeed({platform,available:true}),ensureReady:()=>Effect.succeed(ready),current:Effect.succeed(ready),stopAgent:Effect.void,stop:Effect.void};
 const program=Effect.gen(function*(){const service=yield* native.makeWithHosts(new Map([[id,host]]));yield* service.list;const before=copy(yield* service.state);const updates=yield* service.subscribe;const opened=yield* Effect.result(service.open({hostId:id,deviceId:input.initialDevice.id,platform:input.initialDevice.platform,threadId:'thread-open',...(input.boot===undefined?{}:{boot:input.boot})}));const after=copy(yield* service.state);const events=copy(yield* PubSub.takeAll(updates));const result=opened._tag==='Success'?{value:opened.success}:{error:Object.fromEntries(['_tag','hostId','deviceId','reason','message'].filter(key=>opened.failure[key]!==undefined).map(key=>[key,opened.failure[key]]))};return {input,before,after,events,calls,result};}).pipe(Effect.provideService(Settings.ServerSettingsService,{getSettings:Effect.sync(()=>settings)}),Effect.provide(Digest.layerWebCrypto),Effect.provide(TestClock.layer()),Effect.scoped);
 return await Effect.runPromise(program);
}
const boot=[];
for(const hostId of ['local','fixture-ssh'])for(const initial of [ios,android])for(const sameId of [false,true])for(const scenario of ['returned-present','original-present','missing','platform-incomplete','other-platform-incomplete','both-incomplete','boot-failure']){
 const initialDevice={...initial,booted:false};const returnedId=sameId?initial.id:initial.platform==='android'?'emulator-5560':'sim-new';const incompletePlatforms=scenario==='platform-incomplete'?[initial.platform]:scenario==='other-platform-incomplete'?[initial.platform==='ios'?'android':'ios']:scenario==='both-incomplete'?['ios','android']:[];
 const afterBoot={devices:scenario==='missing'?[]:[scenario==='original-present'?initialDevice:{...initialDevice,id:returnedId,booted:true}],incompletePlatforms,...(incompletePlatforms.length?{detail:'Discovery was incomplete after boot'}:{})};boot.push(await bootCase({hostId,initialDevice,returnedId,afterBoot,bootFailure:scenario==='boot-failure',scenario}));
}
for(const hostId of ['local','fixture-ssh'])for(const initial of [ios,android])for(const alreadyBooted of [false,true]){const initialDevice={...initial,booted:alreadyBooted};boot.push(await bootCase({hostId,initialDevice,returnedId:initial.id,afterBoot:{devices:[initialDevice]},boot:alreadyBooted?undefined:false,scenario:alreadyBooted?'already-booted':'no-boot'}));}
const lock=JSON.parse(fs.readFileSync(path.join(dependencies,'package-lock.json'),'utf8'));
const fixture={revision,effect:'4.1.2',externalDependencies:{diff:{version:'8.0.3',integrity:lock.packages['node_modules/diff'].integrity}},sources,cases,boot};
const serialized=Object.entries(fixture).map(([key,value])=>'  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))).join(',\n');fs.writeFileSync(output,'{\n'+serialized+'\n}\n');console.log(`${cases.length} actual native service refresh scenarios, ${cases.reduce((n,c)=>n+c.transitions.length,0)} status restoration transitions, ${boot.length} boot-confirmation scenarios`);
