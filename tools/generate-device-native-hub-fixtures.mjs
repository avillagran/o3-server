#!/usr/bin/env node
// Execute the complete pinned native device hub operations with an in-memory HTTP transport.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-device-native-hub-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87';
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
const native=await import(pathToFileURL(module('apps/server/src/device/DeviceHubOperations.ts')).href);
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const TestClock=await import(pathToFileURL(require.resolve('effect/testing/TestClock')).href);
const Fiber=await import(pathToFileURL(require.resolve('effect/Fiber')).href);
const HttpClient=await import(pathToFileURL(require.resolve('effect/http/HttpClient')).href);
const HttpClientResponse=await import(pathToFileURL(require.resolve('effect/http/HttpClientResponse')).href);
const ios={id:'sim-1',name:'iPhone',version:'iOS 26',platform:'ios',booted:false,physical:false};
const android={id:'emulator-5554',name:'Pixel_API_36',version:'Android 16',platform:'android',booted:true,physical:false};
function projection(error){const out={};for(const key of ['_tag','hostId','deviceId','operation','reason','diagnostics'])if(error[key]!==undefined)out[key]=error[key];out.message=error.message;if(error._tag==='DeviceBootError')out.cause=error.cause;return out;}
async function execute(input){
 const requests=[],runs=[];let position=0;
 const options={hostId:'local',origin:'http://device.test',http:HttpClient.make(request=>{
  const body=request.body._tag==='Uint8Array'?JSON.parse(new TextDecoder().decode(request.body.body)):null;
  requests.push({method:request.method,url:request.url,body});
  if(input.timeout)return Effect.never;
  if(input.transportFailure)return Effect.fail(new Error('Synthetic device transport unavailable'));
  const response=input.responses[position++]??input.responses.at(-1);
  return Effect.succeed(HttpClientResponse.fromWeb(request,new Response(response.binary?new Uint8Array(response.binary):JSON.stringify(response.body),{status:response.status??200})));
 }),run:(command,args)=>{runs.push({command,args});return Effect.succeed(input.avds??{code:0,stdout:'',stderr:''});},platformAvailable:platform=>Effect.succeed(input.available?.[platform]??false)};
 const operations=native.makeDeviceHubOperations(options);
 const action=input.operation==='list'?operations.list:input.operation==='boot'?operations.boot(input.device):input.operation==='attach'?operations.attach(input.device):input.operation==='screenshot'?operations.screenshot(input.device):operations.shutdown(input.device);
 const program=input.timeout?Effect.gen(function*(){const fiber=yield* Effect.forkChild(Effect.result(action));yield* TestClock.adjust(input.operation==='list'?'40 seconds':'3 minutes');return yield* Fiber.join(fiber);}).pipe(Effect.provide(TestClock.layer())):Effect.result(action);
 const result=await Effect.runPromise(program);
 return {requests,runs,...(result._tag==='Success'?{value:input.operation==='screenshot'?Array.from(result.success):result.success??null}:{error:projection(result.failure)})};
}
const warningSets=[[],[{message:'[android-utils] Android SDK unavailable',error:'private Android diagnostic'}],[{message:'[apple-utils] Failed to run `xcrun simctl list devices --json`',error:'Error: simctl unavailable\n    at hidden frame\nprivate tail'}],[{message:'[apple-utils] Failed to parse JSON output: malformed'}],[{message:'[apple-utils] Failed to list devices: busy'}],[{message:'[apple-utils] `xcrun simctl list devices --json` did not respond within 30000ms'}],[{message:'[apple-utils] Failed to read CoreSimulator plist at /tmp/CoreSimulator/Devices/old/device.plist:',error:'Error: ENOENT: missing'}],[{message:'[apple-utils] Failed to read CoreSimulator plist at /tmp/CoreSimulator/Devices/old/device.plist:',error:"Error: unavailable\n    code: 'ENOENT'"}],[{message:'[apple-utils] Failed to read CoreSimulator plist at /tmp/CoreSimulator/Devices/old/device.plist:',error:'Error: EACCES: permission denied'}],[{message:'[apple-utils] Failed to read CoreSimulator plist at /tmp/unrelated/device.plist:',error:'Error: ENOENT: missing'}],[{message:'Hub warning',error:'  first detail\r\n  second detail\r\n    at frame\r\nignored '}],[{message:'x'.repeat(2200),error:'z'.repeat(2400)}],[{message:'Warning',error:'\ufeff\t \r\n'}],[{message:'[android-utils] Missing'},{message:'[apple-utils] Failed to list devices: busy',error:'failed'}]];
const list=[];
for(const iosAvailable of [false,true])for(const androidAvailable of [false,true])for(const simulators of [[],[ios]])for(const emulators of [[],[android]])for(const errors of warningSets)for(const avds of [{code:0,stdout:'Pixel_API_36\n Tablet \r\nTablet\n',stderr:''},{code:1,stdout:'Ignored_AVD',stderr:'synthetic failure'}]){
 const input={operation:'list',available:{ios:iosAvailable,android:androidAvailable},avds,responses:[{body:{simulators,emulators,errors}}]};list.push({input,expected:await execute(input)});
}
for(const body of [{},{simulators:[],emulators:[],errors:null},{simulators:[{...ios,booted:'false'}],emulators:[]},{simulators:[],emulators:[],errors:[{message:'error',error:null}]}]){const input={operation:'list',responses:[{body}]};list.push({input,expected:await execute(input)});}
for(const status of [400,500]){const input={operation:'list',responses:[{status,body:{simulators:[],emulators:[]}}]};list.push({input,expected:await execute(input)});}
for(const ios of [false,true])for(const android of [false,true]){const input={operation:'list',timeout:true,available:{ios,android},responses:[]};list.push({input,expected:await execute(input)});}
const boot=[];
const diagnostics=['failed','Insufficient disk space','Not enough disk','No space left on device','time out waiting','timed out waiting','timeout','TIMEOUT','insufficient\ndisk','x'.repeat(2200),'   '];
for(const device of [ios,{...android,booted:false}])for(const diagnostic of diagnostics)for(const nested of [false,true]){
 const body=nested?{ok:false,errors:[{message:'Launch command failed',error:diagnostic}]}:{ok:false,error:diagnostic};const input={operation:'boot',device,responses:[{body}]};boot.push({input,expected:await execute(input)});
}
for(const device of [ios,{...android,booted:false}])for(const body of [{ok:true},{ok:true,id:'replacement'},{ok:true,serial:'emulator-5560',id:'replacement'}])for(const attached of [{ok:true},{ok:false,error:'Ignored attach envelope failure'}]){const input={operation:'boot',device,responses:[{body},{body:attached}]};boot.push({input,expected:await execute(input)});}
for(const device of [ios,android])for(const operation of ['attach','shutdown'])for(const ok of [true,false]){const input={operation,device,responses:[{body:{ok}},{body:{simulators:[{...ios,booted:false}],emulators:[]}}]};boot.push({input,expected:await execute(input)});}
for(const device of [ios,android]){const input={operation:'boot',device,timeout:true,responses:[]};boot.push({input,expected:await execute(input)});}
const screenshots=[];
for(const platform of ['ios','android'])for(const id of ['phone','device/a b?é'])for(const status of [200,400,500]){const input={operation:'screenshot',device:{...ios,platform,id},responses:[{status,binary:[137,80,78,71,0,255]}]};screenshots.push({input,expected:await execute(input)});}
for(const platform of ['ios','android']){const input={operation:'screenshot',device:{...ios,platform},transportFailure:true,responses:[]};screenshots.push({input,expected:await execute(input)});}
const fixture={revision,effect:'4.1.2',sources,list,boot,screenshots};
const serialized=Object.entries(fixture).map(([key,value])=>'  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))).join(',\n');fs.writeFileSync(output,'{\n'+serialized+'\n}\n');console.log(`${list.length} native hub listings, ${boot.length} boot/attach/shutdown outcomes, ${screenshots.length} screenshot outcomes`);
