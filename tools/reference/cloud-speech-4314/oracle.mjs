import * as Effect from 'effect/Effect';
import * as Layer from 'effect/Layer';
import * as Schema from 'effect/Schema';
import * as Redacted from 'effect/Redacted';
import * as FetchHttpClient from 'effect/http/FetchHttpClient';
import * as SecretResolver from 'file:///native/packages/secret-provider-core/src/server/SecretResolver.ts';
import * as Source from 'file:///native/apps/server/src/speech/CloudSpeech.ts';
import {DEFAULT_SERVER_SETTINGS,DEFAULT_SPEECH_TRANSCRIPTION_OPTIONS,SpeechTranscriptionOptions} from 'file:///native/packages/contracts/src/index.ts';
import {ServerSettingsService} from '../speech-service-4314/settings.mjs';
import fs from 'node:fs';
import {cloudSpeechLanguages} from 'file:///native/packages/shared/src/speech.ts';
if(process.versions.node!=='26.13.0')throw Error('Exact Node26.13 required');
const clone=value=>structuredClone(value);
const results=[];
async function scenario(name,config,run){
 const events=[],requests=[];let credentials=clone(config.credentials??{}),resolved=config.resolved,catalogFetches=0;
 const settings={getSettings:Effect.sync(()=>{events.push({kind:'settings-read'});if(config.settingsFailure)throw Error('Synthetic settings defect');return {...DEFAULT_SERVER_SETTINGS,cloudSpeechCredentials:clone(credentials)};}),updateSettings:patch=>Effect.sync(()=>{events.push({kind:'settings-write',patch});credentials={...credentials,...patch.cloudSpeechCredentials};})};
 const resolver={invalidate:references=>Effect.sync(()=>events.push({kind:'invalidate',references})),label:()=>Effect.succeed('Fixture Vault'),read:references=>Effect.sync(()=>{events.push({kind:'secret-read',references});if(config.afterSecretRead)credentials=clone(config.afterSecretRead);return references.map(()=>resolved);})};
 const upstream=async(input,init)=>{
  const url=String(input);const request={url,method:init?.method,redirect:init?.redirect,headers:Object.fromEntries(new Headers(init?.headers))};
  if(init?.body instanceof FormData){request.form=[];for(const [key,value]of init.body){request.form.push([key,typeof value==='string'?value:{name:value.name,type:value.type,bytes:value.size,hex:Buffer.from(await value.arrayBuffer()).toString('hex')}]);}}
  else if(init?.body!==undefined){request.json=JSON.parse(await new Response(init.body).text());}
  requests.push(request);
  if(url.includes('/models?')){catalogFetches++;if(config.afterCatalog)credentials=clone(config.afterCatalog);return config.catalogStatus?new Response('synthetic upstream diagnostic',{status:config.catalogStatus}):Response.json(config.catalog??{data:[{id:'fixture/transcribe',name:' Fixture ASR '},{id:'other/model'},{id:'openrouter/auto'},{id:'bad id'},{id:'fixture/blank',name:'   '}]});}
  if(config.rejectFetch)throw Error('Synthetic provider transport failure');
  if(config.status)return new Response('synthetic upstream diagnostic',{status:config.status});
  return Response.json(Object.hasOwn(config,'reply')?config.reply:{text:'  um, oh three  '});
 };
 const state={events,requests,commands:[],credentials:()=>clone(credentials),setCredentials:value=>{credentials=clone(value);},setResolved:value=>{resolved=value;},catalogFetches:()=>catalogFetches};
 const layer=Source.layer.pipe(Layer.provide(FetchHttpClient.layer),Layer.provide(Layer.succeed(ServerSettingsService,settings)),Layer.provide(Layer.succeed(SecretResolver.SecretResolver,resolver)));
 await Effect.runPromise(Effect.scoped(Effect.gen(function*(){const service=yield* Source.CloudSpeech;const record=(label,effect)=>effect.pipe(Effect.result,Effect.map(outcome=>{const result=outcome._tag==='Success'?{ok:true,value:outcome.success??null}:{ok:false,tag:outcome.failure._tag,provider:outcome.failure.provider??null,reason:outcome.failure.reason??null,message:outcome.failure.message};state.observations.push({label,...result});}));state.observations=[];yield* run({...service,transcribe:(provider,pcm,options,model)=>{state.commands.push({provider,pcmHex:Buffer.from(pcm).toString("hex"),options,model:model??null});return service.transcribe(provider,pcm,options,model);}},state,record);})).pipe(Effect.provide(layer),Effect.provideService(FetchHttpClient.Fetch,upstream)));
 results.push({name,input:config,observations:state.observations,commands:state.commands,events,requests,credentials:state.credentials(),catalogFetches});
}
const pcm=new Uint8Array(new Float32Array([0,0.5,-0.5,1,-1,1/32767]).buffer);
const options=partial=>Schema.decodeUnknownSync(SpeechTranscriptionOptions)({...DEFAULT_SPEECH_TRANSCRIPTION_OPTIONS,speechLanguage:'en',speechSilenceFiltering:false,...partial});
for(const provider of ['openai','xai','openrouter']){
 const model=provider==='openrouter'?'fixture/transcribe':undefined;
 await scenario(provider+'-configured-multipart-or-json',{credentials:{[provider]:'fixture-'+provider}},(service,state,record)=>Effect.gen(function*(){yield* record('configured',service.configured);yield* record('transcribe',service.transcribe(provider,pcm,options({speechCustomWords:[{term:'T3 Code',aliases:['oh three']},{term:'<skip>',aliases:[]},{term:'line\nterm',aliases:[]}],speechRemoveFillerWords:true}),model));}));
 for(const status of [401,402,403,404,429,500])await scenario(provider+'-status-'+status,{credentials:{[provider]:'fixture-'+provider},status},(service,state,record)=>record('transcribe',service.transcribe(provider,pcm,options({}),model)));
 for(const reply of [{text:7},{other:'no text'},null])await scenario(provider+'-bad-result-'+JSON.stringify(reply),{credentials:{[provider]:'fixture-'+provider},reply},(service,state,record)=>record('transcribe',service.transcribe(provider,pcm,options({}),model)));
 await scenario(provider+'-missing-key',{},(service,state,record)=>record('transcribe',service.transcribe(provider,new Uint8Array(3),options({}),model)));
 await scenario(provider+'-invalid-pcm',{credentials:{[provider]:'fixture-key'}},(service,state,record)=>record('transcribe',service.transcribe(provider,new Uint8Array(3),options({}),model)));
 await scenario(provider+'-transport-failure',{credentials:{[provider]:'fixture-key'},rejectFetch:true},(service,state,record)=>record('transcribe',service.transcribe(provider,pcm,options({}),model)));
 for(const value of [undefined,'',' ',' https://fixture.invalid','https://fixture.invalid','fixture-key']){
  const reference={provider:'fixture-vault',reference:'fixture/voice/key'};
  await scenario(provider+'-reference-'+JSON.stringify(value),{credentials:{[provider]:reference},resolved:value},(service,state,record)=>record('transcribe',service.transcribe(provider,pcm,options({}),model)));
 }
 await scenario(provider+'-key-changed-during-secret-read',{credentials:{[provider]:{provider:'fixture-vault',reference:'fixture/voice/key'}},resolved:'fixture-resolved',afterSecretRead:{[provider]:'fixture-replacement'}},(service,state,record)=>record('transcribe',service.transcribe(provider,pcm,options({}),model)));
 await scenario(provider+'-key-removed-during-secret-read',{credentials:{[provider]:{provider:'fixture-vault',reference:'fixture/voice/key'}},resolved:'fixture-resolved',afterSecretRead:{}},(service,state,record)=>record('transcribe',service.transcribe(provider,pcm,options({}),model)));
}
await scenario('set-key-and-vault-relock',{},(service,state,record)=>Effect.gen(function*(){
 yield* record('empty',service.configured);yield* record('save',service.setKey('openai',Redacted.make('fixture-chosen')));yield* record('configured',service.configured);yield* record('clear',service.setKey('openai',null));yield* record('cleared',service.configured);
 const reference={provider:'fixture-vault',reference:'fixture/voice/key'};yield* record('set-reference',service.setKey('openai',reference));state.setResolved('fixture-unlocked');yield* record('unlocked',service.transcribe('openai',pcm,options({})));state.setResolved(undefined);yield* record('locked-again',service.transcribe('openai',pcm,options({})));
}));
await scenario('router-catalog-cache-success',{credentials:{openrouter:'fixture-key'}},(service,state,record)=>Effect.gen(function*(){yield* record('first',service.models);yield* record('second',service.models);yield* record('transcribe',service.transcribe('openrouter',pcm,options({}),'fixture/transcribe'));}));
await scenario('router-catalog-failure-not-cached',{credentials:{openrouter:'fixture-key'},catalogStatus:503},(service,state,record)=>Effect.gen(function*(){yield* record('first',service.models);yield* record('second',service.models);}));
for(const model of [undefined,'openrouter/auto','fixture/transcribe:nitro','fixture/missing'])await scenario('router-model-'+JSON.stringify(model),{credentials:{openrouter:'fixture-key'}},(service,state,record)=>record('transcribe',service.transcribe('openrouter',pcm,options({}),model)));
for(const afterCatalog of [{},{openrouter:'fixture-new'}])await scenario('router-key-changed-after-catalog-'+JSON.stringify(afterCatalog),{credentials:{openrouter:'fixture-old'},afterCatalog},(service,state,record)=>record('transcribe',service.transcribe('openrouter',pcm,options({}),'fixture/transcribe')));
fs.writeFileSync(process.argv[2],JSON.stringify({nativeRevision:'83a3674ffd84ffb33de768210c806cb1ac53fd80',node:process.versions.node,cloudSpeechLanguages,description:'Whole original CloudSpeech layer, actual FetchHttpClient/Schema/SettingsSecret helper; fixture settings/resolver/fetch only; no provider network',pcmHex:Buffer.from(pcm).toString('hex'),cases:results},null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({cases:results.length,observations:results.reduce((n,row)=>n+row.observations.length,0)}));
