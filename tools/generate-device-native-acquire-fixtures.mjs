#!/usr/bin/env node
// Execute the native toolchain against in-memory filesystem and process services.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output,dependencies]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-device-native-ssh-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
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
  else if((specifier.startsWith('@effect/')||specifier.startsWith('@noble/')))destination=createRequire(dependencies+'/package.json').resolve(specifier);
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
const native=await import(pathToFileURL(module('apps/server/src/device/DeviceToolchain.ts')).href);
const processModule=await import(pathToFileURL(module('apps/server/src/processRunner.ts')).href);
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const Duration=await import(pathToFileURL(require.resolve('effect/Duration')).href);
const FileSystem=await import(pathToFileURL(require.resolve('effect/FileSystem')).href);
const Path=await import(pathToFileURL(require.resolve('effect/Path')).href);
async function scenario(input){
 const operations=[],commands=[],files=new Map();
 const name=input.tool==='hub'?'expo-device-hub':'agent-device';
 const version=input.tool==='hub'?native.DEVICE_HUB_VERSION:native.AGENT_DEVICE_VERSION;
 const final=`/fixture/profile/tools/${name}/${version}`;
 const entry=input.tool==='hub'?'dist/server/cli.mjs':'bin/agent-device.mjs';
 if(input.installed){files.set(`${final}/.install-complete`,input.sentinel??version+'\n');files.set(`${final}/node_modules/${name}/${entry}`,'fixture');}
 const filesystem=FileSystem.makeNoop({
  exists:file=>Effect.succeed(files.has(file)),
  readFileString:file=>files.has(file)?Effect.succeed(files.get(file)):Effect.fail(new Error('private absent')),
  remove:(file,options)=>Effect.sync(()=>{operations.push({method:'remove',file,options});}),
  makeDirectory:(file,options)=>Effect.sync(()=>{operations.push({method:'mkdir',file,options});}),
  makeTempDirectory:options=>Effect.sync(()=>{operations.push({method:'temp',options});return final+'/../.staging-private';}),
  writeFileString:(file,value)=>Effect.sync(()=>{operations.push({method:'write',file,value});files.set(file,value);}),
  rename:(from,to)=>Effect.sync(()=>{operations.push({method:'rename',from,to});}),
 });
 const runner={run:args=>Effect.sync(()=>{
  commands.push({...args,timeoutMilliseconds:Duration.toMillis(args.timeout),timeout:undefined});
  if(input.entry!==false)files.set(`${final}/../.staging-private/node_modules/${name}/${entry}`,'fixture');
  return {stdout:'',stderr:'private fixture only',code:input.code??0,timedOut:false,stdoutTruncated:false,stderrTruncated:false,stdoutInvalidUtf8:false,stderrInvalidUtf8:false};
 })};
 const action=input.tool==='hub'?native.ensureDeviceHub:native.ensureAgentDevice;
 const result=await Effect.runPromise(action('/fixture/profile').pipe(Effect.result,Effect.provideService(FileSystem.FileSystem,filesystem),Effect.provideService(processModule.ProcessRunner,runner),Effect.provide(Path.layer)));
 return {commands,operations,...(result._tag==='Success'?{paths:result.success}:{error:{_tag:result.failure._tag,tool:result.failure.tool,step:result.failure.step,message:result.failure.message,...(result.failure.exitCode===undefined?{}:{exitCode:result.failure.exitCode})}})};
}
const cases=[];
for(const tool of ['hub','agent'])for(const installed of [false,true])for(const code of [0,1,7])for(const entry of [false,true]){
 const input={tool,installed,code,entry};cases.push({input,expected:await scenario(input)});
}
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,cases},null,2)+'\n');
console.log(cases.length+' native toolchain outcomes; all filesystem and runner operations in memory');
