#!/usr/bin/env node
// Execute the complete native channel with injected input/configuration/write boundaries.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,dependencies,output]=process.argv.slice(2);
if(!output)throw Error('usage: generate-desktop-browser-fixtures.mjs CHECKOUT DEPENDENCIES OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',sources={};
const require=createRequire(dependencies+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const source=path=>{const value=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});sources[path]=crypto.createHash('sha256').update(value).digest('hex');return value;};
const moduleUrl=code=>'data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(code).replace(/from ["']((?:effect|@effect\/platform-node)\/[^"']+)["']/g,(_,name)=>'from '+JSON.stringify(pathToFileURL(require.resolve(name)).href))).toString('base64');
const effect=async name=>import(pathToFileURL(require.resolve('effect/'+name)).href);
const Effect=await effect('Effect'),Queue=await effect('Queue'),Schema=await effect('Schema'),Option=await effect('Option');
const base=source('packages/contracts/src/baseSchemas.ts');
const start=base.indexOf('// Hermes copies'),end=base.indexOf('export const DpopFailureReason');
if(start<0||end<start)throw Error('Pinned base schema boundaries missing');
const baseUrl=moduleUrl('import * as Effect from \"effect/Effect\";import * as Schema from "effect/Schema";import * as SchemaTransformation from "effect/SchemaTransformation";\n'+base.slice(start,end));
const contractUrl=moduleUrl(source('packages/contracts/src/desktopBrowser.ts').replace('"./baseSchemas.ts"',JSON.stringify(baseUrl)));
const contract=await import(contractUrl);
const configUrl=moduleUrl('import * as Context from "effect/Context";export class ServerConfig extends Context.Service()("probe/DesktopBrowserConfig"){}');
const configuration=await import(configUrl);
const streamUrl=moduleUrl('import * as Stream from "effect/Stream";export const fromReadable=()=>Stream.fromQueue(globalThis.browserProbeInput);');
const socketUrl=moduleUrl('export class Socket {destroy(){globalThis.browserProbeDestroyed++;}}');
const writerUrl=moduleUrl('import * as Effect from "effect/Effect";export const writeAllToFileDescriptor=(fd,payload)=>Effect.sync(()=>{globalThis.browserProbeWrites.push({fd,line:payload.toString()});});');
let program=source('apps/server/src/preview/DesktopBrowserChannel.ts');
for(const [from,to] of [['@t3tools/contracts',contractUrl],['../config.ts',configUrl],['@effect/platform-node/NodeStream',streamUrl],['node:net',socketUrl],['../resourceTelemetry/DesktopTelemetryReceiver.ts',writerUrl]])program=program.replace(JSON.stringify(from),JSON.stringify(to));
const native=await import(moduleUrl(program+'\nexport {make};\n'));
const eventDecode=Schema.decodeUnknownOption(contract.DesktopBrowserEvent),commandDecode=Schema.decodeUnknownOption(contract.DesktopBrowserCommand);
const schemaCases=[];
const values=[null,false,true,0,1,1.5,'',' \ufeff value \ufeff',{},[],Infinity];
const add=(kind,input)=>{const result=(kind==='event'?eventDecode:commandDecode)(input);schemaCases.push({kind,input,expected:Option.isSome(result)?result.value:null});};
for(const [kind,base] of [['event',{type:'attached',threadId:'thread',tabId:'tab'}],['event',{type:'detached',threadId:'thread',tabId:'tab'}],['event',{type:'shown',threadId:'thread',tabId:'tab',shown:false}],['event',{type:'cdp',threadId:'thread',tabId:'tab',message:'opaque'}],['command',{type:'cdp',threadId:'thread',tabId:'tab',message:'opaque'}],['command',{type:'release',threadId:'thread',tabId:'tab'}],['command',{type:'pointer',threadId:'thread',tabId:'tab',phase:'click',x:1.5,y:-2}],['command',{type:'turn',threadId:'thread',active:true}]]){
 add(kind,base);add(kind,{...base,future:'ignored'});
 for(const key of Object.keys(base)){const missing={...base};delete missing[key];add(kind,missing);for(const value of values){if(!Number.isFinite(value)&&typeof value==='number')continue;add(kind,{...base,[key]:value});}}
}
const key={threadId:'thread',tabId:'tab'},other={threadId:'other',tabId:'tab'};
const eventLists=[[],[{type:'attached',...key}],[{type:'shown',...key,shown:false}],[{type:'attached',...key},{type:'shown',...key,shown:false},{type:'detached',...key}],[{type:'attached',...key},{type:'shown',...key,shown:false},{type:'shown',...key,shown:true}],[{type:'attached',...key},{type:'attached',...other},{type:'detached',...key}],[{type:'attached',threadId:' \ufeff thread ',tabId:' tab \ufeff '}],[{type:'future',...key},{type:'shown',...key,shown:'invalid'},{type:'attached',...key}]];
const stateCases=[];
for(const config of [{},{desktopBrowserFd:41},{desktopBrowserControlFd:42},{desktopBrowserFd:41,desktopBrowserControlFd:42}])for(const events of eventLists){
 globalThis.browserProbeWrites=[];globalThis.browserProbeDestroyed=0;
 const result=await Effect.runPromise(Effect.gen(function*(){
  const input=yield*Queue.unbounded();globalThis.browserProbeInput=input;
  let channel;const snapshots=[];
  yield*Effect.scoped(Effect.gen(function*(){
   channel=yield*native.make.pipe(Effect.provideService(configuration.ServerConfig,config));
   const snapshot=()=>Effect.gen(function*(){return {attached:yield*channel.isAttached(key),shown:channel.isShown(key),otherAttached:yield*channel.isAttached(other),otherShown:channel.isShown(other)};});
   snapshots.push(yield*snapshot());
   if(channel.available)for(const [index,event]of events.entries()){
    yield*Queue.offer(input,new TextEncoder().encode(JSON.stringify(event)+'\n'));
    const sentinel={threadId:'sentinel',tabId:String(index)};yield*Queue.offer(input,new TextEncoder().encode(JSON.stringify({type:'attached',...sentinel})+'\n'));
    if(!(yield*channel.awaitAttached(sentinel,'1 second')))throw Error('Native input did not reach sentinel');
    snapshots.push(yield*snapshot());
   }
   yield*channel.turn('thread',true);yield*channel.pointer(key,{phase:'move',x:-1.5,y:2.5});yield*channel.turn('thread',false);
   return channel.available;
  }));
  return {available:channel.available,snapshots,afterClose:{attached:yield*channel.isAttached(key),shown:channel.isShown(key),otherShown:channel.isShown(other)}};
 }));
 stateCases.push({config,events,expected:{...result,writes:globalThis.browserProbeWrites,destroyed:globalThis.browserProbeDestroyed}});
}
const Stream=await effect('Stream'),Ndjson=await effect('encoding/Ndjson');
const encode=value=>[...new TextEncoder().encode(value)];
const attached=JSON.stringify({type:'attached',...key}),shown=JSON.stringify({type:'shown',...key,shown:false});
const cdp=JSON.stringify({type:'cdp',...key,message:'opaque 😀 café'});
const bom=[239,187,191],transportInputs=[[],[encode('\n \t\r\n')],[bom],[[...bom,...encode(attached+'\n')]],[encode('\n'),[...bom,...encode(attached)]],[encode(' \t')],[encode('\u00a0')],[encode(attached+'\r'+shown+'\r\n'+cdp+'\n')],[encode(attached)],[encode('{"type":')],[encode(attached+'\nbad\n'+shown)],[encode(attached+'\n'),[255,10],encode(shown)],[[239],[187],[191],encode(attached+'\n')]];
const bytes=encode(cdp+'\n');for(let index=0;index<=bytes.length;index++)transportInputs.push([bytes.slice(0,index),bytes.slice(index)]);
const transportCases=[];
for(const chunks of transportInputs){const values=[];const result=await Effect.runPromise(Stream.fromIterable(chunks.map(chunk=>Uint8Array.from(chunk))).pipe(Stream.pipeThroughChannel(Ndjson.decode({ignoreEmptyLines:true})),Stream.runForEach(value=>Effect.sync(()=>{values.push(value);})),Effect.result));transportCases.push({chunks,expected:{values,...(result._tag==='Failure'?{error:result.failure.reason,unterminated:result.failure.unterminated}:{})}});}
// Parent control direction uses text decoding + splitLines, then per-line optional schema decoding.
const controlDecode=Schema.decodeUnknownOption(Schema.fromJsonString(contract.DesktopBrowserCommand));
const turn=JSON.stringify({type:'turn',threadId:' \ufeff thread ',active:true});
const pointer=JSON.stringify({type:'pointer',...key,phase:'move',x:-1.5,y:2.5});
const controlInputs=[...transportInputs,[encode(turn)],[encode(turn+'\r'+pointer+'\r\n'+cdp+'\n')],[encode('bad\n'+turn+'\n'+pointer)],[encode('\n \t\r\n'+turn)],[encode('{"type":"cdp","threadId":"thread","tabId":"tab","message":"'),[255],encode('"}\n'+turn)],[encode(turn+'\n'),[255,10],encode(pointer)],[[...bom,...encode(turn+'\n')]]];
for(const line of [
 '{"type":"cdp","threadId":"thread","tabId":"tab","message":"opaque 1e400","future":1e400}',
 '{"type":"cdp","threadId":"thread","tabId":"tab","message":"opaque","future":{"nested":[-1e400,1e400]}}',
 '{"type":"pointer","threadId":"thread","tabId":"tab","phase":"move","x":1e400,"y":0}',
 '{"type":"cdp","threadId":"thread","tabId":"tab","message":"opaque","future":1e400.0}',
 '{"type":"cdp","threadId":"thread","tabId":"tab","message":"opaque","future":1e400e2}',
 '{"type":"pointer","threadId":"thread","tabId":"tab","phase":"move","x":1e-400,"y":-0}',
])controlInputs.push([encode(line+'\n'+turn+'\n')]);
const controlBytes=encode(cdp+'\r\n'+turn);for(let index=0;index<=controlBytes.length;index++)controlInputs.push([controlBytes.slice(0,index),controlBytes.slice(index)]);
const controlTransportCases=[];
for(const chunks of controlInputs){const lines=[],commands=[];const result=await Effect.runPromise(Stream.fromIterable(chunks.map(chunk=>Uint8Array.from(chunk))).pipe(Stream.decodeText(),Stream.splitLines,Stream.filter(line=>line.length>0),Stream.runForEach(line=>Effect.sync(()=>{lines.push(line);const decoded=controlDecode(line);if(Option.isSome(decoded))commands.push(decoded.value);})),Effect.result));controlTransportCases.push({chunks,expected:{lines,commands,...(result._tag==='Failure'?{error:String(result.failure)}:{})}});}
const endpointCases=[];
if(process.argv.includes('--endpoints')){
 const deadline=(promise,phase)=>new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('Native endpoint timed out: '+phase)),2000);promise.then(value=>{clearTimeout(timer);resolve(value);},error=>{clearTimeout(timer);reject(error);});});
 const closed=socket=>socket.readyState===WebSocket.CLOSED?Promise.resolve():deadline(new Promise(resolve=>socket.addEventListener('close',resolve,{once:true})),'close');
 const connect=async url=>{const socket=new WebSocket(url);try{await deadline(new Promise((resolve,reject)=>{socket.addEventListener('open',resolve,{once:true});socket.addEventListener('error',reject,{once:true});}),'connect');return socket;}catch(error){socket.close();throw error;}};
 const wait=async predicate=>{for(let n=0;n<500;n++){if(predicate())return;await new Promise(resolve=>setTimeout(resolve,2));}throw Error('Native endpoint observation timed out');};
 globalThis.browserProbeWrites=[];globalThis.browserProbeDestroyed=0;
 const expected=await Effect.runPromise(Effect.scoped(Effect.gen(function*(){
  const input=yield*Queue.unbounded();globalThis.browserProbeInput=input;
  const channel=yield*native.make.pipe(Effect.provideService(configuration.ServerConfig,{desktopBrowserFd:41,desktopBrowserControlFd:42}));
  yield*Queue.offer(input,new TextEncoder().encode(JSON.stringify({type:'attached',...key})+'\n'));
  if(!(yield*channel.awaitAttached(key,'1 second')))throw Error('Native endpoint attach timed out');
  const url=yield*channel.endpoint(key);
  return yield*Effect.promise(async()=>{
   const first=await connect(url);
   try{
   const incoming=new Promise(resolve=>first.addEventListener('message',event=>resolve(event.data),{once:true}));
   await Effect.runPromise(Queue.offer(input,new TextEncoder().encode(JSON.stringify({type:'cdp',...key,message:'native opaque 😀'})+'\n')));
   const received=await deadline(incoming,'incoming frame');
   first.send('client opaque 😀');
   await wait(()=>globalThis.browserProbeWrites.some(write=>JSON.parse(write.line).message==='client opaque 😀'));
   first.close();await closed(first);
   return {received,secretBytes:Buffer.from(new URL(url).pathname.slice(1),'base64url').length};
   }finally{first.close();}
  });
 })));
 endpointCases.push({scenario:'first-client-roundtrip',expected:{...expected,writes:globalThis.browserProbeWrites,destroyed:globalThis.browserProbeDestroyed}});
}
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,schemaCases,stateCases,transportCases,controlTransportCases,...(endpointCases.length?{endpointCases}:{})},null,2)+'\n');
console.log(`${schemaCases.length} native browser schema cases, ${stateCases.length} full-channel lifecycle traces, ${transportCases.length} NDJSON and ${controlTransportCases.length} parent-control byte-stream cases`);
