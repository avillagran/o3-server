import * as Effect from 'effect/Effect';
import * as FileSystem from 'effect/FileSystem';
import * as Path from 'effect/Path';
import * as SecureRandom from 'effect/crypto/SecureRandom';
import * as Stream from 'effect/Stream';
import * as Sink from 'effect/Sink';
import * as Option from 'effect/Option';
import {HttpClient,HttpClientError,HttpClientResponse} from 'effect/http';
import {ServerConfig} from './config.mjs';
import * as Native from 'file:///native/apps/server/src/speech/SpeechModelStore.ts';
import crypto from 'node:crypto';
import nodepath from 'node:path';
import fs from 'node:fs';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
if(process.versions.node!=='26.13.0')throw Error('Pinned Node26.13.0 required');
if(JSON.parse(fs.readFileSync('/deps/node_modules/effect/package.json','utf8')).version!=='4.1.2')throw Error('Pinned Effect4.1.2 required');
const nativeRequire=createRequire('file:///native/apps/server/src/speech/SpeechModelStore.ts');
const ownRequire=createRequire(import.meta.url);
if(nativeRequire.resolve('effect/Schema')!==ownRequire.resolve('effect/Schema'))throw Error('Duplicate canonical Effect schema instance');

const data=Uint8Array.from({length:64},(_,index)=>(index*7)%256);
const sha=bytes=>crypto.createHash('sha256').update(bytes).digest('hex');
const model={id:'fixture/speech-model',revision:'a'.repeat(40),filename:'model.gguf',size:64,sha256:sha(data)};
const directory='/fixture/speech/models',final=directory+'/model.gguf';
export function harness(config){
 const files=new Map(),events=[],requests=[];let nextInode=100n,tick=1000,hashes=0,uuid=0,requestIndex=0,changed=false;
 const put=(path,bytes,info)=>{files.set(path,{bytes:Uint8Array.from(bytes),ino:info?.ino??nextInode++,mtime:info?.mtime??tick++,type:info?.type??'File'});};
 const err=(method,path)=>({_tag:'SystemError',reason:'NotFound',module:'FileSystem',method,pathOrDescriptor:path});
 if(config.initial==='valid')put(final,data);
 if(config.initial==='corrupt')put(final,new Uint8Array(64));
 if(config.initial==='short')put(final,data.subarray(0,20));
 if(config.orphan)put(final+'.f660de6c-e172-4146-8ffe-d3c32eb48cd4.part',data.subarray(0,20));
 put(final+'.notes.part',new TextEncoder().encode('unrelated'));
 const marker=path=>{const file=files.get(path);return [model.sha256,file.bytes.length,file.mtime,7n,file.ino].join(':');};
 if(config.marker&&files.has(final))put(final+'.verified',new TextEncoder().encode(config.marker==='valid'?marker(final):'untrusted'));
 const disk={
  readDirectory:()=>Effect.sync(()=>[...files.keys()].filter(path=>nodepath.dirname(path)===directory).map(path=>nodepath.basename(path))),
  stat:path=>Effect.suspend(()=>{const file=files.get(path);return file?Effect.succeed({type:file.type,size:BigInt(file.bytes.length),mtime:Option.some(new Date(file.mtime)),dev:7n,ino:Option.some(file.ino)}):Effect.fail(err('stat',path));}),
  readFileString:path=>Effect.suspend(()=>files.has(path)?Effect.succeed(new TextDecoder().decode(files.get(path).bytes)):Effect.fail(err('readFileString',path))),
  writeFileString:(path,text)=>config.markerWriteFails?Effect.fail(err('writeFileString',path)):Effect.sync(()=>{put(path,new TextEncoder().encode(text));events.push({kind:'marker',path,text});}),
  makeDirectory:()=>Effect.void,
  remove:(path,options)=>Effect.sync(()=>{events.push({kind:'remove',path,options});files.delete(path);}),
  rename:(from,to)=>Effect.suspend(()=>{const file=files.get(from);if(!file)return Effect.fail(err('rename',from));files.set(to,file);files.delete(from);events.push({kind:'rename',from,to});return Effect.void;}),
  stream:path=>Stream.unwrap(Effect.sync(()=>{
   hashes++;events.push({kind:'hash',path});const file=files.get(path);if(!file)return Stream.fail(err('stream',path));
   const bytes=Uint8Array.from(file.bytes);
   if(config.changeDuringHash&&!changed){file.mtime+=100;changed=true;}
   return Stream.succeed(bytes);
  })),
  sink:(path,options)=>Sink.unwrap(Effect.sync(()=>{
   if(options.flag==='w'||!files.has(path))put(path,new Uint8Array());
   return Sink.forEach(chunk=>Effect.sync(()=>{const file=files.get(path),bytes=new Uint8Array(file.bytes.length+chunk.length);bytes.set(file.bytes);bytes.set(chunk,file.bytes.length);file.bytes=bytes;file.mtime=tick++;}));
  })),
 };
 const client=HttpClient.make(request=>Effect.sync(()=>{
  const spec=config.responses[Math.min(requestIndex++,config.responses.length-1)];
  requests.push({url:request.url,range:request.headers.range??null});
  const transport=()=>new HttpClientError.HttpClientError({reason:new HttpClientError.TransportError({request,cause:new Error('synthetic transport reset')})});
  let body=Stream.empty;
  for(const item of spec.chunks??[]){const bytes=item==='rest'?data.subarray(20):item==='all'?data:item==='first'?data.subarray(0,20):Uint8Array.from(item);body=Stream.concat(body,Stream.succeed(bytes));}
  if(spec.fail)body=Stream.concat(body,Stream.fail(transport()));
  const response=HttpClientResponse.fromWeb(request,new Response(null,{status:spec.status??200}));
  Object.defineProperty(response,'headers',{value:spec.headers??{}});Object.defineProperty(response,'stream',{value:body});return response;
 }));
 const random={uuidV4:()=>Effect.succeed('00000000-0000-4000-8000-'+String(++uuid).padStart(12,'0'))};
 const program=Effect.gen(function*(){
  const store=yield* Native.SpeechModelStore.pipe(Effect.provide(Native.layer));
  const before=[yield* store.isReady(model),(yield* store.installedPath(model))??null,yield* store.isReady(model)];
  const outcome=yield* Effect.result(store.download(model,{onProgress:bytes=>events.push({kind:'progress',bytes}),onVerificationStart:()=>events.push({kind:'verify'})}));
  const after=[yield* store.isReady(model),(yield* store.installedPath(model))??null,yield* store.isReady(model)];
  const observed={before,outcome:outcome._tag==='Success'?{ok:true,path:outcome.success}:{ok:false,tag:outcome.failure._tag,reason:outcome.failure.reason??null,status:outcome.failure.status??null,message:outcome.failure.message},after,requests,events,hashes,files:[...files].sort(([a],[b])=>a.localeCompare(b)).map(([path,file])=>({path,bytes:file.bytes.length,sha256:sha(file.bytes),text:path.endsWith('.verified')?new TextDecoder().decode(file.bytes):null}))};
  Object.defineProperty(observed,'failureObject',{value:outcome._tag==='Failure'?outcome.failure:undefined});return observed;
 }).pipe(Effect.provideService(FileSystem.FileSystem,disk),Effect.provideService(Path.Path,nodepath.posix),Effect.provideService(SecureRandom.SecureRandom,random),Effect.provideService(ServerConfig,{stateDir:'/fixture'}),Effect.provideService(HttpClient.HttpClient,client));
 return Effect.runPromise(program);
}
const good={chunks:['all'],headers:{'content-length':'64'}};
const cases=[
 {name:'fresh',responses:[good]},
 {name:'valid-first-use',initial:'valid',responses:[good]},
 {name:'valid-marker-no-hash',initial:'valid',marker:'valid',responses:[good]},
 {name:'invalid-marker-rehash',initial:'valid',marker:'invalid',responses:[good]},
 {name:'corrupt-same-size',initial:'corrupt',responses:[good]},
 {name:'wrong-size',initial:'short',responses:[good]},
 {name:'marker-write-ignored',markerWriteFails:true,responses:[good]},
 {name:'identity-changes-while-hashing',initial:'valid',changeDuringHash:true,responses:[good]},
 {name:'native-orphan-removal-plan',orphan:true,responses:[good]},
 {name:'transport-resume',responses:[{chunks:['first'],fail:true},{status:206,chunks:['rest'],headers:{'content-range':'bytes 20-63/64','content-length':'44'}}]},
 {name:'range-ignored-restart',responses:[{chunks:['first'],fail:true},good]},
 {name:'three-short-attempts',responses:[{chunks:['first']}]},
 {name:'bad-sha256',responses:[{chunks:[new Array(64).fill(0)],headers:{'content-length':'64'}}]},
];
for(const status of [201,206,400,416,500])cases.push({name:'status-'+status,responses:[{status,chunks:['all']}]});
for(const value of ['0','63','65','',' 64 ','0x40','Infinity','NaN'])cases.push({name:'content-length-'+JSON.stringify(value),responses:[{chunks:['all'],headers:{'content-length':value}}]});
for(const range of ['bytes 20-63/64','bytes 19-63/64','bytes 20-62/64','bytes 20-63/65','bytes 20-63/*','bytes 20-63/64\n','bytes 020-063/064'])cases.push({name:'content-range-'+JSON.stringify(range),responses:[{chunks:['first'],fail:true},{status:206,chunks:['rest'],headers:{'content-range':range}}]});
for(const points of [[10],[13],[13,10],[0x2028],[0x2029]]) {
 const range='bytes 20-63/64'+String.fromCodePoint(...points);
 cases.push({name:'range-real-terminators-'+points.join('-'),rawCodepoints:[...range].map(c=>c.codePointAt(0)),responses:[{chunks:['first'],fail:true},{status:206,chunks:['rest'],headers:{'content-range':range}}]});
}
for(const [label,digits] of [['max-finite','f'.repeat(13)+'8'+'0'.repeat(242)],['below-overflow','f'.repeat(13)+'b'+'f'.repeat(242)],['overflow-tie','f'.repeat(13)+'c'+'0'.repeat(242)],['above-overflow','f'.repeat(256)]]) {
 const integer=BigInt('0x'+digits);
 for(const [radix,prefix] of [[16,'0x'],[8,'0o'],[2,'0b']]) {
  const length=prefix+integer.toString(radix);
  cases.push({name:'length-'+label+'-radix'+radix,numberFinite:Number.isFinite(Number(length)),responses:[{chunks:['all'],headers:{'content-length':length}}]});
 }
}
if(import.meta.url===pathToFileURL(process.argv[1]).href){
const results=[];for(const input of cases)results.push({input,expected:await harness(input)});
fs.writeFileSync(process.argv[2],JSON.stringify({nativeRevision:'83a3674ffd84ffb33de768210c806cb1ac53fd80',model,cases:results},null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({cases:results.length,actualWholeModule:true,filesystem:'in-memory operations only; no native real deletion'}));

}
