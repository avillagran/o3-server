import * as Effect from 'effect/Effect';
import * as Layer from 'effect/Layer';
import * as Schema from 'effect/Schema';
import * as Redacted from 'effect/Redacted';
import * as FileSystem from 'effect/FileSystem';
import * as HttpRouter from 'effect/http/HttpRouter';
import * as HttpServer from 'effect/http/HttpServer';
import * as HttpApi from 'effect/http-api/HttpApi';
import * as HttpApiBuilder from 'effect/http-api/HttpApiBuilder';
import {EnvironmentHttpApi,EnvironmentVoiceHttpApi,EnvironmentAuthenticatedAuth,EnvironmentAuthenticatedPrincipal,AuthSessionId,CloudSpeechError,DEFAULT_SPEECH_TRANSCRIPTION_OPTIONS,DEFAULT_SERVER_SETTINGS} from 'file:///native/packages/contracts/src/index.ts';
import * as Speech from 'file:///native/apps/server/src/speech/SpeechService.ts';
import * as Cloud from 'file:///native/apps/server/src/speech/CloudSpeech.ts';
import {speechHttpApiLayer} from 'file:///native/apps/server/src/speech/http.ts';
import {ServerSettingsService} from '../speech-service-4314/settings.mjs';
import {TextGeneration} from './tokens.mjs';
import fs from 'node:fs';
const options=Buffer.from(JSON.stringify(DEFAULT_SPEECH_TRANSCRIPTION_OPTIONS));
const valid=Buffer.alloc(4+options.length+8);valid.writeUInt32LE(options.length);options.copy(valid,4);valid.writeFloatLE(0.5,4+options.length+4);
const fixtures=[
 {name:'cloud-typed-error-wire',path:'/api/voice/cloud/openai/transcribe',binaryHex:valid.toString('hex'),contentType:'application/octet-stream'},
 {name:'router-typed-error-wire',path:'/api/voice/cloud/openrouter/transcribe?model=fixture/model',binaryHex:valid.toString('hex'),contentType:'application/octet-stream'},
 {name:'keys',method:'GET',path:'/api/voice/cloud/keys'},
 {name:'models',method:'GET',path:'/api/voice/cloud/openrouter/models'},
 ...['openai','xai','openrouter','OpenAI','unknown'].map(provider=>({name:'key-'+provider,path:'/api/voice/cloud/'+provider+'/key',body:{key:'fixture-key'}})),
 ...[null,'',' fixture-key ','https://fixture.invalid','a\nb',{provider:'fixture-vault',reference:' fixture/key '},{provider:'fixture-vault',reference:'fixture/key',ignored:true},42,{},false].map((key,index)=>({name:'key-shape-'+index,path:'/api/voice/cloud/openai/key',body:{key}})),
 {name:'missing-key',path:'/api/voice/cloud/openai/key',body:{}},
 ...['openai','xai','openrouter','unknown'].map(provider=>({name:'transcribe-'+provider,path:'/api/voice/cloud/'+provider+'/transcribe',raw:'bad',contentType:'application/octet-stream'})),
 ...['fixture/model','openrouter/auto','fixture/model:nitro','fixture/model&model=other/model'].map((model,index)=>({name:'model-query-'+index,path:'/api/voice/cloud/openrouter/transcribe?model='+model,raw:'bad',contentType:'application/octet-stream'})),
 {name:'large-cloud-header',path:'/api/voice/cloud/openai/transcribe',raw:'bad',contentType:'application/octet-stream',contentLength:'19328005'},
 {name:'postprocess',path:'/api/voice/post-process',body:{options:{speechCustomWords:[],speechPostProcessingPrompt:{mode:'default',customInstructions:''}},transcript:'',draft:{text:'',selection:{start:0,end:0}}}},
];
const cases=[];
for(const scopes of [['orchestration:read'],['orchestration:operate'],['settings:write']]){
 const calls=[];
 const principal=Layer.succeed(EnvironmentAuthenticatedAuth,effect=>effect.pipe(Effect.provideService(EnvironmentAuthenticatedPrincipal,{sessionId:Schema.decodeUnknownSync(AuthSessionId)('cloud-http-fixture'),subject:'fixture',method:'bearer-access-token',scopes:new Set(scopes)})));
 const cloud={configured:Effect.sync(()=>{calls.push({method:'configured'});return{openai:false,xai:true,openrouter:false};}),models:Effect.sync(()=>{calls.push({method:'models'});return[{id:'fixture/model',name:'Fixture model'}];}),setKey:(provider,key)=>Effect.sync(()=>calls.push({method:'setKey',provider,key:Redacted.isRedacted(key)?Redacted.value(key):key})),transcribe:(provider,pcm,options,model)=>Effect.sync(()=>calls.push({method:'transcribe',provider,pcmHex:Buffer.from(pcm).toString('hex'),options,model:model??null})).pipe(Effect.andThen(Effect.fail(new CloudSpeechError({provider,reason:'rejected-key'}))))};
 const logicalFs=FileSystem.makeNoop({makeTempDirectoryScoped:()=>Effect.die('No filesystem effects are permitted in this HTTP admission fixture')});
 const dependencies=Layer.mergeAll(Layer.succeed(Cloud.CloudSpeech,cloud),Layer.succeed(ServerSettingsService,{getSettings:Effect.succeed(DEFAULT_SERVER_SETTINGS)}),Layer.succeed(TextGeneration,{}),Layer.succeed(FileSystem.FileSystem,logicalFs));
 const {handler,dispose}=HttpRouter.toWebHandler(HttpApiBuilder.layer(HttpApi.make('environment').add(EnvironmentVoiceHttpApi)).pipe(Layer.provide(speechHttpApiLayer),Layer.provide(principal),Layer.provide(Layer.succeed(Speech.SpeechService,{})),Layer.provide(dependencies),Layer.provide(HttpServer.layerServices)),{disableLogger:true});
 try{
  for(const input of fixtures){
   calls.length=0;const headers={'content-type':input.contentType??'application/json'};if(input.contentLength)headers['content-length']=input.contentLength;
   const response=await handler(new Request('http://fixture'+input.path,{method:input.method??'POST',headers,...(input.binaryHex!==undefined?{body:Buffer.from(input.binaryHex,'hex')}:input.raw!==undefined?{body:input.raw}:input.body!==undefined?{body:JSON.stringify(input.body)}:{})}));
   const text=await response.text();let body;try{body=JSON.parse(text);}catch{body=text;}
   if(body&&typeof body==='object'&&body.traceId)body.traceId='<native-trace>';
   cases.push({name:input.name,input,scopes,status:response.status,body,calls:[...calls]});
  }
 }finally{await dispose();}
}
// The actual typed error serializer, not a modeled public error DTO.
const cloudErrors=[];
for(const provider of ['openai','xai','openrouter'])for(const reason of ['missing-key','key-changed','unreadable-key','rejected-key','rate-limit','model-unavailable','unavailable']){
 const error=new CloudSpeechError({provider,reason});cloudErrors.push({provider,reason,encoded:Schema.encodeSync(CloudSpeechError)(error),message:error.message});
}
fs.writeFileSync(process.argv[2],JSON.stringify({nativeRevision:'83a3674ffd84ffb33de768210c806cb1ac53fd80',description:'Whole original HTTP route/auth-scope ordering and actual typed error schema; logical principal/domain boundaries, no auth crypto/provider or filesystem effects',cases,cloudErrors},null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({cases:cases.length,cloudErrors:cloudErrors.length}));
