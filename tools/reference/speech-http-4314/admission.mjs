import * as Effect from 'effect/Effect';
import * as Layer from 'effect/Layer';
import * as Schema from 'effect/Schema';
import * as HttpRouter from 'effect/http/HttpRouter';
import * as HttpServer from 'effect/http/HttpServer';
import * as HttpApi from 'effect/http-api/HttpApi';
import * as HttpApiBuilder from 'effect/http-api/HttpApiBuilder';
import * as NodeServices from '@effect/platform-node/NodeServices';
import * as HostProcess from 'file:///native/packages/shared/src/HostProcess.ts';
import {AuthSessionId,AuthSettingsWriteScope,AuthOrchestrationOperateScope,EnvironmentAuthenticatedAuth,EnvironmentAuthenticatedPrincipal,EnvironmentVoiceHttpApi,DEFAULT_SERVER_SETTINGS} from 'file:///native/packages/contracts/src/index.ts';
import * as Speech from 'file:///native/apps/server/src/speech/SpeechService.ts';
import * as Store from 'file:///native/apps/server/src/speech/SpeechModelStore.ts';
import {speechHttpApiLayer} from 'file:///native/apps/server/src/speech/http.ts';
import {ServerSettingsService} from '../speech-service-4314/settings.mjs';
import {CloudSpeech,TextGeneration} from './tokens.mjs';
import {harness} from '../speech-model-store-4314/oracle.mjs';
import fs from 'node:fs';
const observed=await harness({responses:[{status:500,chunks:[]}]});
const storeError=observed.failureObject;
if(storeError?._tag!=='SpeechModelDownloadError')throw Error('Expected the actual private whole-ModelStore error instance');
const settings={getSettings:Effect.succeed(DEFAULT_SERVER_SETTINGS),updateSettings:()=>Effect.void};
const virtualStore={isReady:()=>Effect.succeed(false),download:()=>Effect.fail(storeError)};
let mapped;
await Effect.runPromise(Effect.scoped(Effect.gen(function*(){const speech=yield* Speech.SpeechService;const result=yield* Effect.result(speech.downloadModel(DEFAULT_SERVER_SETTINGS.speechModelId));if(result._tag!=='Failure')throw Error('Expected failure');mapped=result.failure;})).pipe(Effect.provide(Speech.layer),Effect.provideService(HostProcess.Platform,'linux'),Effect.provideService(HostProcess.Architecture,'x64'),Effect.provideService(HostProcess.LinuxLibc,'gnu'),Effect.provideService(ServerSettingsService,settings),Effect.provideService(Store.SpeechModelStore,virtualStore)));
const fixtures=[
 {name:'download-valid-reader',path:'/api/voice/models/download',body:{modelId:DEFAULT_SERVER_SETTINGS.speechModelId}},
 ...[{},null,[],7,{modelId:null},{modelId:7},{modelId:''},{modelId:'a'.repeat(161)},{modelId:'😀'.repeat(81)},{modelId:' '},{modelId:'😀'.repeat(80)},{modelId:DEFAULT_SERVER_SETTINGS.speechModelId,unknown:true}].map((body,index)=>({name:'download-shape-'+index,path:'/api/voice/models/download',body})),
 {name:'download-bad-json',path:'/api/voice/models/download',raw:'{'},
 {name:'download-no-content-type',path:'/api/voice/models/download',body:{modelId:DEFAULT_SERVER_SETTINGS.speechModelId},contentType:null},
 {name:'download-text-content-type',path:'/api/voice/models/download',body:{modelId:DEFAULT_SERVER_SETTINGS.speechModelId},contentType:'text/plain'},
 ...['','?benchmarks=1','?benchmarks=0','?benchmarks=1&benchmarks=1','?benchmarks=0&benchmarks=1'].map((query,index)=>({name:'models-query-'+index,method:'GET',path:'/api/voice/models'+query})),
 {name:'prepare',path:'/api/voice/models/prepare'},
 ...[{acceleration:'cpu'},{acceleration:'auto'},{acceleration:'gpu:a'},{acceleration:'gpu:'},{acceleration:'gpu:a\n'},{acceleration:null},{}].map((body,index)=>({name:'acceleration-'+index,path:'/api/voice/acceleration',body})),
 ...[{timeout:'min_15'},{timeout:'immediately'},{timeout:'never'},{timeout:null},{timeout:'15'},{}].map((body,index)=>({name:'unload-'+index,path:'/api/voice/model-unload-timeout',body})),
 {name:'transcribe-bad-options',path:'/api/voice/transcribe',raw:'bad',contentType:'application/octet-stream'},
 {name:'transcribe-oversized-header',path:'/api/voice/transcribe',raw:'bad',contentType:'application/octet-stream',contentLength:'19328005'},
];
const cases=[];
for(const scopes of [['orchestration:read'],[AuthOrchestrationOperateScope],[AuthSettingsWriteScope]]){
 const principal=Layer.succeed(EnvironmentAuthenticatedAuth,effect=>effect.pipe(Effect.provideService(EnvironmentAuthenticatedPrincipal,{sessionId:Schema.decodeUnknownSync(AuthSessionId)('speech-http-fixture'),subject:'fixture',method:'bearer-access-token',scopes:new Set(scopes)})));
 const calls=[];
 const failed=(method,input)=>Effect.sync(()=>calls.push({method,input})).pipe(Effect.andThen(Effect.fail(mapped)));
 const speech={downloadModel:id=>failed('downloadModel',id),models:benchmarks=>failed('models',benchmarks),prepareModel:failed('prepareModel',null),updateAcceleration:value=>failed('updateAcceleration',value),updateModelUnloadTimeout:value=>failed('updateModelUnloadTimeout',value),transcribe:()=>failed('transcribe','decoded')};
 const dependencies=Layer.mergeAll(Layer.succeed(CloudSpeech,{}),Layer.succeed(ServerSettingsService,settings),Layer.succeed(TextGeneration,{}));
 const {handler,dispose}=HttpRouter.toWebHandler(HttpApiBuilder.layer(HttpApi.make('environment').add(EnvironmentVoiceHttpApi)).pipe(Layer.provide(speechHttpApiLayer),Layer.provide(principal),Layer.provide(Layer.succeed(Speech.SpeechService,speech)),Layer.provide(dependencies),Layer.provide(HttpServer.layerServices),Layer.provide(NodeServices.layer)),{disableLogger:true});
 await Effect.runPromise(Effect.scoped(Effect.gen(function*(){
  yield* Effect.addFinalizer(()=>Effect.promise(dispose));const context=yield* Layer.build(NodeServices.layer);
  for(const fixture of fixtures){
   calls.length=0;const headers={};const contentType=fixture.contentType===undefined?'application/json':fixture.contentType;
   if(contentType!==null)headers['content-type']=contentType;if(fixture.contentLength!==undefined)headers['content-length']=fixture.contentLength;
   const body=fixture.raw??(fixture.body===undefined?undefined:JSON.stringify(fixture.body));
   const request=new Request('http://fixture'+fixture.path,{method:fixture.method??'POST',headers,...(body===undefined?{}:{body:contentType===null?new TextEncoder().encode(body):body})});
   const response=yield* Effect.promise(()=>handler(request,context));
   const text=yield* Effect.promise(()=>response.text());let value;try{value=JSON.parse(text);}catch{value=text;}
   const trace=value&&typeof value==='object'?value.traceId:undefined;
   if(trace!==undefined)value.traceId='<native-trace>';
   const responseHeaders=Object.fromEntries([...response.headers].map(([key,value])=>[key,trace!==undefined&&value===trace?'<native-trace>':value]));
   cases.push({name:fixture.name,input:fixture,actualRequestContentType:request.headers.get('content-type'),scopes,status:response.status,headers:responseHeaders,body:value,calls:[...calls]});
  }
 })));
}
fs.writeFileSync(process.argv[2],JSON.stringify({nativeRevision:'83a3674ffd84ffb33de768210c806cb1ac53fd80',scope:'whole original HTTP router input decoding, scope order and error serialization; only method dependencies return the already observed native Service error',cases},null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({wholeHttpRoute:true,cases:cases.length,network:false,cryptography:false}));
