#!/usr/bin/env node
// Execute the complete native DeviceHubProxy router with synthetic auth, readiness and HTTP.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
if(!fs.existsSync('/run/.containerenv'))throw Error('Run native proxy fixtures inside private rootless Podman');
const [checkout,contracts,output,dependencies,pinnedRevision]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-device-native-proxy-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision=pinnedRevision??'32a0531748c44b25072d7495ce0b9bd7e9450a87';
if(!['32a0531748c44b25072d7495ce0b9bd7e9450a87','4cb6945dc60ed03f567dc5daafd083e52308e82b'].includes(revision))throw Error('An audited native proxy revision is required');
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
const native=await import(pathToFileURL(module('apps/server/src/device/DeviceHubProxy.ts')).href);
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const Layer=await import(pathToFileURL(require.resolve('effect/Layer')).href);
const {HttpClient,HttpClientResponse,HttpRouter}=await import(pathToFileURL(require.resolve('effect/http')).href);
const Auth=await import(pathToFileURL(module('apps/server/src/auth/EnvironmentAuth.ts')).href);
const DeviceService=await import(pathToFileURL(module('apps/server/src/device/DeviceService.ts')).href);
const {AuthOrchestrationReadScope:read,AuthOrchestrationOperateScope:operate}=await import(pathToFileURL(contracts+'/src/index.ts').href);
const paths=['/api/devices','/api/devices/ws','/api/devices/boot','/vendor/serve-sim/api','/vendor/serve-sim/api/screenshot','/vendor/serve-sim/api/event-log','/vendor/serve-sim/api/event-log/events','/vendor/serve-sim/helper/sim-1/stream.mjpeg','/vendor/serve-sim/helper/sim-1/stream.avcc','/vendor/serve-sim/helper/sim-1/config','/vendor/serve-sim/helper/sim-1/health','/vendor/serve-sim/helper/sim-1/ax','/vendor/serve-sim/helper/sim-1/foreground','/vendor/serve-sim/helper/sim-1/panel/1/stream.avcc','/vendor/serve-sim/helper/sim-1/panel/3/stream.avcc','/vendor/serve-sim/helper/sim-1/panel/2/stream.avcc','/vendor/serve-sim/helper/sim-1/webrtc/offer','/vendor/serve-sim/helper/sim-1/webrtc/close','/vendor/serve-sim/helper/ws','/vendor/serve-sim/appstate','/vendor/serve-sim/exec','/vendor/serve-emu/api','/vendor/serve-emu/api/devices','/vendor/serve-emu/api/screenshot','/vendor/serve-emu/api/stream-mode','/vendor/serve-emu/api/stream-settings','/vendor/serve-emu/api/accessibility','/vendor/serve-emu/api/fold','/vendor/serve-emu/health','/vendor/serve-emu/webrtc/offer','/vendor/serve-emu/webrtc/close','/vendor/serve-emu/ws','/vendor/serve-emu/exec','/unknown'];
const cases=[];
for(const scopes of [[],[read],[operate],[read,operate]]){
 let requests=[],readinessCalls=[];
 const http=HttpClient.make(request=>Effect.sync(()=>{requests.push({url:request.url,method:request.method,headers:Object.fromEntries(Object.entries(request.headers).filter(([key])=>!['b3','traceparent','tracestate'].includes(key)))});return HttpClientResponse.fromWeb(request,new Response('frame',{headers:{'set-cookie':'upstream=1','content-type':'application/octet-stream'}}));}));
 const {handler,dispose}=HttpRouter.toWebHandler(native.layer.pipe(Layer.provideMerge(Layer.succeed(Auth.EnvironmentAuth,{authenticateWebSocketUpgrade:()=>Effect.succeed({sessionId:'test',subject:'test',method:'bearer-access-token',scopes})})),Layer.provideMerge(Layer.succeed(DeviceService.DeviceService,{currentReadiness:host=>Effect.sync(()=>{readinessCalls.push(host??null);return {hostId:host??'local',hub:{origin:'http://hub.test'}};})})),Layer.provideMerge(Layer.succeed(HttpClient.HttpClient,http))),{disableLogger:true});
 for(const path of paths)for(const method of ['GET','HEAD','POST','PATCH','DELETE','OPTIONS']){
  requests=[];readinessCalls=[];
  const uri='/api/device-hub'+path+'?device=phone+%2B+1&wsTicket=secret&hostId=ssh-host&device=again';
  const headers={'authorization':'Bearer private-fixture','cookie':'session=secret','dpop':'private-proof','x-forwarded-proto':'https','origin':'https://client.test','content-type':'application/json','x-fixture':'kept'};
  const response=await handler(new Request('http://t3.test'+uri,{method,headers,...(['GET','HEAD'].includes(method)?{}:{body:'{}'})}));const text=await response.text();
  cases.push({input:{uri,method,scopes,headers},expected:{status:response.status,requests,readinessCalls,text,headers:Object.fromEntries(response.headers)}});
 }
 await dispose();
}
const websocketCases=[];
for(const scopes of [[],[read],[operate],[read,operate]]){
 const {handler,dispose}=HttpRouter.toWebHandler(native.layer.pipe(Layer.provideMerge(Layer.succeed(Auth.EnvironmentAuth,{authenticateWebSocketUpgrade:()=>Effect.succeed({sessionId:'test',subject:'test',method:'bearer-access-token',scopes})})),Layer.provideMerge(Layer.succeed(DeviceService.DeviceService,{currentReadiness:()=>Effect.succeed(null)})),Layer.provideMerge(Layer.succeed(HttpClient.HttpClient,HttpClient.make(()=>Effect.die('No unavailable hub transport may open'))))),{disableLogger:true});
 for(const socketPath of ['/api/devices/ws','/vendor/serve-sim/helper/ws','/vendor/serve-emu/ws']){
  const uri='/api/device-hub'+socketPath;
  const response=await handler(new Request('http://t3.test'+uri,{headers:{upgrade:'websocket',connection:'Upgrade'}}));
  websocketCases.push({input:{uri,method:'GET',scopes},expected:{status:response.status,text:await response.text()}});
 }
 await dispose();
}
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,cases,websocketCases},null,2)+'\n');console.log(cases.length+' native complete proxy route outcomes; '+websocketCases.length+' native WebSocket scope outcomes');
