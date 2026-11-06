#!/usr/bin/env node
// Execute native LocalDeviceHost helpers with an entirely synthetic filesystem.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output,dependencies]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-device-native-platform-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87';
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const directory=path.dirname(output);fs.mkdirSync(directory,{recursive:true});
const sources={},loaded=new Map();
function module(file){
 if(loaded.has(file))return loaded.get(file);
 const target=path.join(directory,crypto.createHash('sha256').update(file).digest('hex')+(file.endsWith('.json')?'.json':'.mjs'));
 loaded.set(file,target);
 const source=execFileSync('git',['-c','safe.directory='+checkout,'-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'});
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
const native=await import(pathToFileURL(module('apps/server/src/device/LocalDeviceHost.ts')).href);
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const FileSystem=await import(pathToFileURL(require.resolve('effect/FileSystem')).href);
const Path=await import(pathToFileURL(require.resolve('effect/Path')).href);
const Host=await import(pathToFileURL(module('packages/shared/src/HostProcess.ts')).href);
const cases=[];
for(const platform of ['linux','darwin','win32'])for(const mode of ['explicit','root','standard','path','none'])for(let mask=0;mask<32;mask++){
 const env={HOME:'/fixture/home',PATH:'/fixture/bin'};
 if(mode==='explicit')env.ANDROID_HOME='  /fixture/sdk  ';
 if(mode==='root'){env.ANDROID_HOME='  ';env.ANDROID_SDK_ROOT=' /fixture/sdk ';}
 const root=mode==='standard'?'/fixture/home/Library/Android/sdk':'/fixture/sdk';
 const win=platform==='win32';const adb=win?'adb.exe':'adb';const avd=win?'avdmanager.bat':'avdmanager';
 const names=[`platform-tools/${adb}`,`emulator/${win?'emulator.exe':'emulator'}`,`cmdline-tools/latest/bin/${avd}`,`tools/bin/${avd}`];
 const files=mode==='none'?[]:names.filter((_,index)=>mask&(1<<index)).map(name=>root+'/'+name);
 const realpaths={};if(mode==='path'&&(mask&1)){files.push('/fixture/bin/'+adb);realpaths['/fixture/bin/'+adb]=root+'/platform-tools/'+adb;}
 // The fifth bit adds a later candidate, proving native first-found ordering.
 if(mask&16)files.push('/fixture/home/Android/Sdk/platform-tools/'+adb);
 const filesystem=FileSystem.makeNoop({exists:file=>Effect.succeed(files.includes(file)),realPath:file=>Effect.succeed(realpaths[file]??file)});
 const program=Effect.gen(function*(){
   const sdk=yield* native.__testing.androidSdk;
   const reason=yield* native.__testing.platformReason('android');
   const path=yield* Path.Path;
   const environment=native.__testing.deviceHostEnvironment(env,sdk.root,platform,path);
   return {sdk,reason,environment};
 }).pipe(Effect.provideService(FileSystem.FileSystem,filesystem),Effect.provideService(Host.Platform,platform),Effect.provideService(Host.Environment,env),Effect.provide(Path.layer));
 const expected=await Effect.runPromise(program);
 cases.push({input:{platform,environment:env,files,realpaths},expected});
}
const Runner=await import(pathToFileURL(module('apps/server/src/processRunner.ts')).href);
const Option=await import(pathToFileURL(require.resolve('effect/Option')).href);
const hubCases=[];
for(const platform of ['linux','darwin','win32'])for(const android of [undefined,'','/fixture/sdk'])for(const runtime of ['missing','valid','wrong-owner','file','defined-empty','defined'])for(let camera=0;camera<8;camera++){
 const environment={FORCE_COLOR:'1',NO_COLOR:'0',EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS:'-existing value'};if(android!==undefined)environment.ANDROID_HOME=android;
 if(runtime==='defined-empty')environment.XDG_RUNTIME_DIR='';if(runtime==='defined')environment.XDG_RUNTIME_DIR='/fixture/runtime';
 const uid=camera===7?undefined:camera===6?0:1000;
 const outputs=[{code:0,stdout:'emulated',stderr:''},{code:0,stdout:'emulated imagefile',stderr:''},{code:0,stdout:'emulatedCamera',stderr:''},{code:1,stdout:'emulated',stderr:''},{code:0,stdout:'λemulatedλ',stderr:''},{code:0,stdout:'',stderr:'emulated'}];
 const cameras=[outputs[camera%outputs.length],outputs[(camera+1)%outputs.length]];
 const filesystem=FileSystem.makeNoop({stat:()=>runtime==='missing'?Effect.fail(new Error('missing')):Effect.succeed({type:runtime==='file'?'File':'Directory',uid:Option.some(runtime==='wrong-owner'?9999:uid)})});
 const run={run:input=>Effect.succeed(cameras[input.args[0].endsWith('front')?0:1])};
 const expected=await Effect.runPromise(native.__testing.hubEnvironment(environment).pipe(Effect.provideService(FileSystem.FileSystem,filesystem),Effect.provideService(Runner.ProcessRunner,run),Effect.provideService(Host.Platform,platform),Effect.provideService(Host.UserId,uid),Effect.provide(Path.layer)));
 hubCases.push({input:{platform,environment,uid:uid??null,validRuntimeDirectory:runtime!=='missing'&&runtime!=='wrong-owner'&&runtime!=='file',cameras},expected});
}
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,cases,hubCases},null,2)+'\n');
console.log(cases.length+' native local platform cases + '+hubCases.length+' native hub environment cases');
