#!/usr/bin/env node
// Execute native read-only tool inventories against an entirely in-memory filesystem.
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
const PlatformError=await import(pathToFileURL(require.resolve('effect/PlatformError')).href);
async function scenario(input){
 const locations=new Map(),directories=new Map();
 for(const [tool,pkg] of [['hub','expo-device-hub'],['agent','agent-device']]){
  const base=`/fixture/profile/tools/${pkg}`;directories.set(base,{tool,rows:input.entries?.[tool]??[]});
  for(const row of input.entries?.[tool]??[]){
   const root=`${base}/${row.version}`;
   locations.set(`${root}/.install-complete`,{tool,row,sentinel:true});
   locations.set(`${root}/node_modules/${pkg}/${tool==='hub'?'dist/server/cli.mjs':'bin/agent-device.mjs'}`,{tool,row});
  }
 }
 const failure=(tag,method)=>Effect.fail(PlatformError.systemError({_tag:tag,module:'FileSystem',method,description:'Private inventory fixture'}));
 const filesystem=FileSystem.makeNoop({
  readDirectory:directory=>{
   const value=directories.get(directory);if(!value)throw Error('Unexpected directory '+directory);
   return input.directoryErrors?.[value.tool]?failure(input.directoryErrors[value.tool],'readDirectory'):Effect.succeed(value.rows.map(row=>row.version));
  },
  readFileString:file=>{
   const value=locations.get(file);if(!value?.sentinel)throw Error('Unexpected sentinel '+file);
   const row=value.row;if(row.missing)return failure('NotFound','readFileString');if(row.sentinelError)return failure(row.sentinelError,'readFileString');
   return Effect.succeed(row.sentinel??row.version);
  },
  exists:file=>{
   const value=locations.get(file);if(!value||value.sentinel)throw Error('Unexpected entry '+file);
   return value.row.entryError?failure(value.row.entryError,'exists'):Effect.succeed(value.row.entry!==false);
  },
 });
 const value=await Effect.runPromise(native.deviceToolVersions('/fixture/profile',input.running??{}).pipe(Effect.provideService(FileSystem.FileSystem,filesystem),Effect.provide(Path.layer)));
 return value===undefined?{unavailable:true}:{value};
}
const cases=[];
const versions=['0.19.1','0.21.31','1.0.0','01.02.03','1.2.3-rc.1','1.2.3-.','1.2.3-..','1.2.3-','v1.2.3','1.2','1.2.3+meta','1.2.3\n','../escape','.staging-private','1.2.3_foo',''];
for(const tool of ['hub','agent'])for(const version of versions)for(const variant of ['exact','space','bom','wrong','empty','missing','no-entry','sentinel-error','entry-error']){
 const row={version,...(variant==='space'?{sentinel:` \t${version}\r\n`}:variant==='bom'?{sentinel:`\uFEFF${version}\u00A0`}:variant==='wrong'?{sentinel:'99.99.99'}:variant==='empty'?{sentinel:''}:variant==='missing'?{missing:true}:variant==='no-entry'?{entry:false}:variant==='sentinel-error'?{sentinelError:'PermissionDenied'}:variant==='entry-error'?{entryError:'PermissionDenied'}:{})};
 const input={entries:{[tool]:[row]},running:{hub:'0.18.0',agent:null}};cases.push({input,expected:await scenario(input)});
}
for(const tool of ['hub','agent'])for(const error of ['NotFound','PermissionDenied']){
 const input={entries:{hub:[{version:'0.19.1'}],agent:[{version:'0.21.31'}]},directoryErrors:{[tool]:error}};cases.push({input,expected:await scenario(input)});
}
for(const running of [{},{hub:'0.18.0',agent:'0.20.0'},{hub:null,agent:null}]){
 const input={entries:{hub:['2.0.0','10.0.0','1.0.0-rc.2','1.0.0-rc.10','01.0.0'].map(version=>({version})),agent:['0.21.31','0.19.1'].map(version=>({version}))},running};cases.push({input,expected:await scenario(input)});
}

const Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const contractsModule=await import(pathToFileURL(contracts+'/src/index.ts').href);
function pinnedSource(file){const source=execFileSync('git',['-c','safe.directory='+checkout,'-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'});sources[file]=crypto.createHash('sha256').update(source).digest('hex');return source;}
const authSource=pinnedSource('apps/server/src/auth/RpcAuthorization.ts');
const wsSource=pinnedSource('apps/server/src/ws.ts');
const requiredSource=authSource.match(/export const requiredScopeForDeviceList =[^;]+;/)?.[0];if(!requiredSource)throw Error('Native device authorization helper changed');
if(!authSource.includes('[WS_METHODS.deviceList]: AuthOrchestrationReadScope'))throw Error('Native base device scope changed');
const required=new Function('AuthOrchestrationOperateScope','AuthOrchestrationReadScope',stripTypeScriptTypes(requiredSource.replace('export ',''))+';return requiredScopeForDeviceList;')(contractsModule.AuthOrchestrationOperateScope,contractsModule.AuthOrchestrationReadScope);
const marker='[WS_METHODS.deviceList]:';const start=wsSource.indexOf(marker)+marker.length,end=wsSource.indexOf('[WS_METHODS.deviceOpen]',start);if(start<marker.length||end<0)throw Error('Native device handler changed');
const expression=wsSource.slice(start,end).trim().replace(/,$/,'');
const handler=new Function('deviceService','authorizeEffect','requiredScopeForDeviceList',`return (${expression});`)({inspect:{action:'inspect'},list:{action:'list'},updateTool:tool=>({action:'update',tool}),retryHost:host=>({action:'retry',host})},(scope,effect)=>({...effect,extraScope:scope}),required);
const authCases=[];
for(const input of [{},{inspectOnly:true},{retryHostId:'mini'},{inspectOnly:true,retryHostId:'mini'},{updateTool:'hub'},{updateTool:'agent',inspectOnly:true},{updateTool:'hub',retryHostId:'mini'},{updateTool:'agent',retryHostId:'mini',inspectOnly:true},{updateTool:null,inspectOnly:true},{updateTool:false,inspectOnly:true},{updateTool:'invalid'},{retryHostId:null},{retryHostId:''},{inspectOnly:'true'},{inspectOnly:true,retryHostId:null},{retryHostId:' mini '},{retryHostId:' '.repeat(3)},{retryHostId:'x'.repeat(129)},{retryHostId:'😀'.repeat(64)},{retryHostId:'😀'.repeat(65)},{updateTool:'hub',retryHostId:null},null,[]]){
 let decoded;try{decoded=await Effect.runPromise(Schema.decodeUnknownEffect(contractsModule.DeviceListInput)(input));}catch{authCases.push({input,invalid:true});continue;}
 authCases.push({input,decoded,baseScope:contractsModule.AuthOrchestrationReadScope,expected:handler(decoded)});
}

fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,cases,authCases},null,2)+'\n');console.log(cases.length+' native inventory outcomes, '+authCases.length+' native decoded authorization/dispatch rows; no filesystem mutation or process service');
