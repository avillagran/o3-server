#!/usr/bin/env node
// Execute the whole pinned preview manager; inject only clock and random services.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,dependencies,output]=process.argv.slice(2);
if(!output)throw Error('usage: generate-preview-manager-fixtures.mjs CHECKOUT DEPENDENCIES OUTPUT [URL_OUTPUT]');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',sources={};
const require=createRequire(dependencies+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const source=path=>{const text=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});sources[path]=crypto.createHash('sha256').update(text).digest('hex');return text;};
const moduleUrl=code=>'data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(code).replace(/from ["'](effect(?:\/[^"']+)?)['"]/g,(_,name)=>'from '+JSON.stringify(pathToFileURL(require.resolve(name)).href))).toString('base64');
const effect=async name=>import(pathToFileURL(require.resolve('effect/'+name)).href);
const Effect=await effect('Effect'),PubSub=await effect('PubSub'),SecureRandom=await effect('crypto/SecureRandom'),Schema=await effect('Schema'),Option=await effect('Option');
const base=moduleUrl(source('packages/contracts/src/baseSchemas.ts'));
const profile=moduleUrl(source('packages/contracts/src/browserProfile.ts').replace('"./baseSchemas.ts"',JSON.stringify(base)));
const contract=moduleUrl(source('packages/contracts/src/preview.ts').replace('"./baseSchemas.ts"',JSON.stringify(base)).replace('"./browserProfile.ts"',JSON.stringify(profile)));
const classification=moduleUrl(source('packages/shared/src/hostClassification.ts'));
const shared=moduleUrl(source('packages/shared/src/preview.ts').replace('"./hostClassification.ts"',JSON.stringify(classification)));
const clock=moduleUrl('import * as Effect from "effect/Effect";import * as Native from "effect/DateTime";export const now=Effect.sync(()=>Native.makeUnsafe(globalThis.previewProbeTime));export const formatIso=Native.formatIso;');
const program=source('apps/server/src/preview/Manager.ts').replaceAll('"@t3tools/contracts"',JSON.stringify(contract)).replace('"@t3tools/shared/preview"',JSON.stringify(shared)).replace('"effect/DateTime"',JSON.stringify(clock));
const native=await import(moduleUrl(program)),helpers=await import(shared);
const contracts=await import(contract);
const acceptsOpen=Schema.decodeUnknownOption(contracts.PreviewOpenInput);
const cleanError=error=>Object.fromEntries([['_tag',error._tag],['message',error.message],...Object.entries(error).filter(([key])=>!['_tag','stack','cause','message','name'].includes(key))]);
const inputs=new Set(['',' ','\ufeff','localhost:5173','LOCALHOST/x','127.0.0.1:3000','0.0.0.0/x','[::1]:8080/x','[::]/x','example.com','https://example.com/x?q=a#b','http://[','http://[😀','https://example.com/😀','http://','https://a:bad','https://user:pass@example.com','😀'.repeat(1025)]);
for(const scheme of ['about','blob','chrome','data','file','ftp','javascript','mailto','sms','ssh','tel','view-source','ws','wss','custom','http','https'])for(const suffix of [':value',':VALUE','://example.com/x'])inputs.add(scheme+suffix);
for(const host of ['example.com','localhost','localhost.','127.1','127.0.0.1','0.0.0.0','[::1]','[::]','münich.example','example.com:80'])for(const prefix of ['',' \ufeff ','\u0085'])inputs.add(prefix+host+'\n');
const urlCases=[...inputs].map(input=>{const openSchemaAccepted=Option.isSome(acceptsOpen({threadId:'thread',url:input}));try{return{input,openSchemaAccepted,expected:{url:helpers.normalizePreviewUrl(input)}};}catch(error){return{input,openSchemaAccepted,expected:{error:cleanError(error)}};}});
const traces=[];
let sequence=0;
const serverEpoch='00000000-0000-4000-8000-000000000001';
const opsFor=input=>[
 {method:'open',input,as:'a'},
 {method:'reportStatus',input:{threadId:'thread',tabId:'$a',navStatus:{_tag:'Success',url:'https://example.com/',title:'Title'},canGoBack:true,canGoForward:false}},
 {method:'reportStatus',input:{threadId:'thread',tabId:'$a',serverControlled:true,navStatus:{_tag:'LoadFailed',url:'https://example.com/',title:'Failed',code:-7,description:'Synthetic failure',download:{id:'download',fileName:'fixture.pdf'}},canGoBack:true,canGoForward:false}},
 {method:'markHeadless',input:{threadId:'thread',tabId:'$a'}},
 {method:'markHeadless',input:{threadId:'thread',tabId:'$a'}},
 {method:'requestReveal',input:{threadId:'thread',tabId:'$a',force:false}},
 {method:'resize',input:{threadId:'thread',tabId:'$a',viewport:{_tag:'fixed',width:800,height:600}}},
 {method:'adjust',input:{threadId:'thread',tabId:'$a',colorScheme:'dark',zoomFactor:1.25,hardReload:true,clear:'cache'}},
 {method:'adjust',input:{threadId:'thread',tabId:'$a'}},
 {method:'close',input:{threadId:'other'}},
 {method:'close',input:{threadId:'thread',tabId:'missing'}},
 {method:'close',input:{threadId:'thread',tabId:'$a'}},
 {method:'resize',input:{threadId:'thread',tabId:'$a',viewport:{_tag:'fill'}}},
];
const scenarios=[];
for(const runtime of [undefined,'server'])for(const profileId of [undefined,'default','incognito'])for(const url of [undefined,'',' ','localhost:5173','example.com','javascript:alert(1)','http://['])scenarios.push(opsFor({threadId:'thread',...(runtime?{runtime,automationOwner:'agent'}:{}),...(profileId?{profileId}:{}),...(url!==undefined?{url}:{}),reveal:false}));
scenarios.push([...Array.from({length:40},(_,i)=>({method:'open',input:{threadId:i%3===0?'other':'thread',runtime:'server'},as:'tab'+i})),{method:'close',input:{threadId:'thread'}},{method:'close',input:{threadId:'other'}}]);
for(const operations of scenarios){
 const initialSequence=sequence;
 const expected=await Effect.runPromise(Effect.scoped(Effect.gen(function*(){
  let uuid=0;const random={uuidV4:()=>Effect.sync(()=>uuid++===0?serverEpoch:`00000000-0000-4000-8000-${String(uuid).padStart(12,'0')}`)};
  const manager=yield*native.make.pipe(Effect.provideService(SecureRandom.SecureRandom,random));
  const subscription=yield*manager.subscribeEvents;
  const aliases={},steps=[];
  for(const [index,operation]of operations.entries()){
   globalThis.previewProbeTime=Date.UTC(2026,10,6,0,0,index);
   const input=JSON.parse(JSON.stringify(operation.input),(key,value)=>typeof value==='string'&&value.startsWith('$')?(aliases[value.slice(1)]??'missing'):value);
   const before=[];
   if(operation.method==='open'){sequence++;input.beforePublish=snapshot=>before.push(snapshot);}
   const result=yield*manager[operation.method](input).pipe(Effect.result);
   if(result._tag==='Success'&&operation.as)aliases[operation.as]=result.success.tabId;
   const events=yield*PubSub.takeUpTo(subscription,Number.MAX_SAFE_INTEGER);
   const lists={thread:yield*manager.list({threadId:'thread'}),other:yield*manager.list({threadId:'other'})};
   steps.push({at:new Date(globalThis.previewProbeTime).toISOString(),input:operation,expected:{...(result._tag==='Success'?{result:result.success??null}:{error:cleanError(result.failure)}),events,before,lists}});
  }
  return steps;
 })));
 traces.push({serverEpoch,initialSequence,steps:expected});
}
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources,urlCases,traces},null,2)+'\n');
if(process.argv[5])fs.writeFileSync(process.argv[5],JSON.stringify({revision,effect:'4.1.2',sources,urlCases},null,2)+'\n');
console.log(`${urlCases.length} actual URL helper cases, ${traces.length} complete native preview-manager traces`);
