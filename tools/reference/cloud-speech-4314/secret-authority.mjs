import * as Effect from 'effect/Effect';
import * as Layer from 'effect/Layer';
import * as Schema from 'effect/Schema';
import * as Fiber from 'effect/Fiber';
import * as Deferred from 'effect/Deferred';
import * as FetchHttpClient from 'effect/http/FetchHttpClient';
import {DEFAULT_SERVER_SETTINGS,DEFAULT_SPEECH_TRANSCRIPTION_OPTIONS,ServerSettings} from 'file:///native/packages/contracts/src/index.ts';
import * as Source from 'file:///native/apps/server/src/speech/CloudSpeech.ts';
import * as ResolverLayer from 'file:///native/apps/server/src/secrets/SecretResolver.ts';
import {ServerSettingsService} from '../speech-service-4314/settings.mjs';
import {reset} from './secret-drivers.mjs';
import fs from 'node:fs';
const cases=[];
for(const name of ['changed-before-lookup','changed-during-lookup','key-changed-during-lookup']){
 const reference={provider:'vault',reference:'op://Fixture/Voice/key'};
 const provider=name==='changed-before-lookup'?'openrouter':'openai';
 let settings={...DEFAULT_SERVER_SETTINGS,cloudSpeechCredentials:{openai:'',xai:'',openrouter:'',[provider]:reference},secretProviders:{vault:{kind:'one_password',label:'Fixture Vault',config:{account:'old-account'}}}};
 if(!Schema.is(ServerSettings)(settings))throw Error('Fixture must be a valid actual effective ServerSettings value');
 const state={events:[],entered:false,gate:name==='changed-before-lookup'?undefined:Deferred.makeUnsafe(),byAccount:{'old-account':'fixture-old-key','new-account':'fixture-new-key'}};reset(state);
 const settingsService={getSettings:Effect.sync(()=>{state.events.push({kind:'settings-read',account:settings.secretProviders.vault.config.account});return settings;})};
 const uploads=[];
 const upstream=async(url,init)=>{
  if(String(url).includes('/models?')){settings={...settings,secretProviders:{vault:{...settings.secretProviders.vault,config:{account:'new-account'}}}};return Response.json({data:[{id:'fixture/transcribe'}]});}
  uploads.push({url:String(url),bearer:new Headers(init.headers).get('authorization')});return Response.json({text:'fixture transcript'});
 };
 const layer=Source.layer.pipe(Layer.provide(FetchHttpClient.layer),Layer.provide(ResolverLayer.layer),Layer.provide(Layer.succeed(ServerSettingsService,settingsService)));
 const outcome=await Effect.runPromise(Effect.scoped(Effect.gen(function*(){
  const service=yield* Source.CloudSpeech;
  const run=service.transcribe(provider,new Uint8Array(new Float32Array([0,0.5]).buffer),DEFAULT_SPEECH_TRANSCRIPTION_OPTIONS,provider==='openrouter'?'fixture/transcribe':undefined).pipe(Effect.result);
  if(!state.gate)return yield* run;
  const fiber=yield* Effect.forkChild(run);
  while(!state.entered)yield* Effect.yieldNow;
  settings=name==='key-changed-during-lookup'?{...settings,cloudSpeechCredentials:{...settings.cloudSpeechCredentials,[provider]:'fixture-replacement-key'}}:{...settings,secretProviders:{vault:{...settings.secretProviders.vault,config:{account:'new-account'}}}};
  yield* Deferred.succeed(state.gate,undefined);return yield* Fiber.join(fiber);
 })).pipe(Effect.provide(layer),Effect.provideService(FetchHttpClient.Fetch,upstream)));
 cases.push({name,events:state.events,uploads,result:outcome._tag==='Success'?{ok:true,text:outcome.success}:{ok:false,tag:outcome.failure._tag,reason:outcome.failure.reason},finalAccount:settings.secretProviders.vault.config.account});
}
fs.writeFileSync(process.argv[2],JSON.stringify({nativeRevision:'83a3674ffd84ffb33de768210c806cb1ac53fd80',description:'Whole actual CloudSpeech AND application SecretResolver layer/core, actual OnePassword definition, synthetic driver read and Fetch only; schema-validated snapshots; no CLI, filesystem or provider network',cases},null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({cases:cases.length}));
