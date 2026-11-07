import * as Effect from 'effect/Effect';
import * as Schema from 'effect/Schema';
import {SpeechTranscriptionOptions} from 'file:///native/packages/contracts/src/speech.ts';
import * as Deferred from 'effect/Deferred';
import * as Fiber from 'effect/Fiber';
import * as Exit from 'effect/Exit';
import * as TestClock from 'effect/testing/TestClock';
import * as HostProcess from 'file:///native/packages/shared/src/HostProcess.ts';
import * as Source from 'file:///native/apps/server/src/speech/SpeechService.ts';
import * as Store from 'file:///native/apps/server/src/speech/SpeechModelStore.ts';
import {SPEECH_MODELS,DEFAULT_SPEECH_MODEL_ID} from 'file:///native/apps/server/src/speech/model.ts';
import {SILERO_VAD} from 'file:///native/apps/server/src/speech/silero.ts';
import {ServerSettingsService} from './settings.mjs';
import {state as native} from './native.mjs';
import fs from 'node:fs';
if(process.versions.node!=='26.13.0')throw Error('Exact Node26.13 required');
const tiny=SPEECH_MODELS.find(model=>model.id==='handy-computer/moonshine-tiny-gguf');
const normalize=result=>result._tag==='Success'?{ok:true,value:result.success??null}:{ok:false,tag:result.failure._tag,operation:result.failure.operation??null,modelId:result.failure.modelId??null,message:result.failure.message};
async function scenario(name,run){
 native.events=[];native.profile={};native.replyText='';native.streamMode=false;native.autoGpu=native.failGpu=native.failAll=false;const events=native.events;
 let settings={speechModelId:tiny.id,speechAcceleration:'cpu',speechModelUnloadTimeout:'min_2',speechLanguage:'auto',speechRemoveFillerWords:false,speechCustomWords:[]};const ready=new Set([tiny.id]);const controlled=new Set(),pending=new Map();
 const serviceSettings={getSettings:Effect.sync(()=>({...settings})),updateSettings:patch=>Effect.sync(()=>{events.push({kind:'settings',patch});settings={...settings,...patch};})};
 const store={isReady:model=>Effect.sync(()=>ready.has(model.id)),installedPath:model=>Effect.sync(()=>ready.has(model.id)?'/fixture/models/'+model.filename:undefined),download:(model,progress)=>Effect.gen(function*(){
 events.push({kind:'download',id:model.id});progress?.onProgress?.(0);
 if(controlled.has(model.id)){const gate=Deferred.makeUnsafe();pending.set(model.id,{gate,progress});yield* Deferred.await(gate).pipe(Effect.ensuring(Effect.sync(()=>{if(pending.get(model.id)?.gate===gate)pending.delete(model.id);})));}
 progress?.onProgress?.(model.size);progress?.onVerificationStart?.();ready.add(model.id);return '/fixture/models/'+model.filename;
}).pipe(Effect.onExit(exit=>Effect.sync(()=>{if(!Exit.isSuccess(exit))events.push({kind:'download-interrupted',id:model.id});}))),remove:model=>Effect.sync(()=>{events.push({kind:'remove',id:model.id});ready.delete(model.id);})};
 const observations=[];
 const program=Effect.scoped(Effect.gen(function*(){
  const service=yield* Source.SpeechService;
  yield* run(service,{events,ready,controlled,pending,settings:()=>settings,observations,record:(label,result)=>observations.push({label,...normalize(result)})});
 })).pipe(Effect.provide(Source.layer),Effect.provideService(HostProcess.Platform,'linux'),Effect.provideService(HostProcess.Architecture,'x64'),Effect.provideService(HostProcess.LinuxLibc,'gnu'),Effect.provideService(ServerSettingsService,serviceSettings),Effect.provideService(Store.SpeechModelStore,store),Effect.provide(TestClock.layer()));
 await Effect.runPromise(program);return {name,observations,events,settings,ready:[...ready]};
}
const cases=[];
cases.push(await scenario('select-uninstalled-and-missing', (service,h)=>Effect.gen(function*(){
 h.record('status',yield* Effect.result(service.status));
 h.record('missing',yield* Effect.result(service.selectModel('missing-model')));
 h.record('select-default-uninstalled',yield* Effect.result(service.selectModel(DEFAULT_SPEECH_MODEL_ID)));
})));
cases.push(await scenario('prepare-unload-absolute-clock',(service,h)=>Effect.gen(function*(){
 h.record('prepare',yield* Effect.result(service.prepareModel));
 h.observations.push({label:'loaded',events:[...h.events]});
 yield* TestClock.adjust(2*60_000);h.observations.push({label:'two-minutes',events:[...h.events]});
 yield* TestClock.adjust(3*60_000);h.observations.push({label:'five-minutes',events:[...h.events]});
})));
cases.push(await scenario('download-dedup-busy-and-cancel',(service,h)=>Effect.gen(function*(){
 const target=DEFAULT_SPEECH_MODEL_ID;h.controlled.add(target);
 const first=yield* service.downloadModel(target).pipe(Effect.forkChild);
 while(!h.pending.has(target))yield* TestClock.adjust(0);
 const second=yield* service.downloadModel(target).pipe(Effect.forkChild);yield* TestClock.adjust(0);
 h.record('selection-busy',yield* Effect.result(service.selectModel(tiny.id)));
 h.record('other-download-busy',yield* Effect.result(service.downloadModel(tiny.id)));
 h.record('wrong-cancel-noop',yield* Effect.result(service.cancelDownload('other-id')));
 h.record('cancel',yield* Effect.result(service.cancelDownload(target)));
 h.record('first-caller',yield* Effect.result(Fiber.join(first)));
 h.record('second-caller',yield* Effect.result(Fiber.join(second)));
})));
cases.push(await scenario('removal-selects-first-installed-catalog-entry',(service,h)=>Effect.gen(function*(){
 h.ready.add(DEFAULT_SPEECH_MODEL_ID);
 h.record('prepare',yield* Effect.result(service.prepareModel));
 h.record('remove-selected',yield* Effect.result(service.removeModel(tiny.id)));
})));
cases.push(await scenario('selection-disposes-before-settings-write',(service,h)=>Effect.gen(function*(){
 h.record('prepare',yield* Effect.result(service.prepareModel));
 h.record('select',yield* Effect.result(service.selectModel(DEFAULT_SPEECH_MODEL_ID)));
})));
cases.push(await scenario('never-timeout-keeps-model-until-scope-close',(service,h)=>Effect.gen(function*(){
 h.record('prepare',yield* Effect.result(service.prepareModel));
 h.record('never',yield* Effect.result(service.updateModelUnloadTimeout('never')));
 yield* TestClock.adjust('2 hours');h.observations.push({label:'two-hours',events:[...h.events]});
})));
const options={speechLanguage:'auto',speechTranslateToEnglish:false,speechRemoveFillerWords:false,speechCustomWords:[],speechSilenceFiltering:false};
cases.push(await scenario('recording-silero-timeout-backoff-and-installation-reset',(service,h)=>Effect.gen(function*(){
 yield* service.updateModelUnloadTimeout('never');h.controlled.add(SILERO_VAD.id);
 const first=yield* service.transcribe(new Uint8Array(new Float32Array([0]).buffer),options).pipe(Effect.forkChild);
 while(!h.pending.has(SILERO_VAD.id))yield* TestClock.adjust(0);
 yield* TestClock.adjust(15_000);h.record('first-recording',yield* Effect.result(Fiber.join(first)));
 h.record('backoff-recording',yield* Effect.result(service.transcribe(new Uint8Array(new Float32Array([0]).buffer),options)));
 h.controlled.delete(SILERO_VAD.id);
 h.record('installation-bypasses-backoff',yield* Effect.result(service.downloadModel(DEFAULT_SPEECH_MODEL_ID)));
 h.record('recording-after-installation',yield* Effect.result(service.transcribe(new Uint8Array(new Float32Array([0]).buffer),options)));
})));
cases.push(await scenario('installation-silero-timeout-is-optional',(service,h)=>Effect.gen(function*(){
 h.controlled.add(SILERO_VAD.id);
 const installation=yield* service.downloadModel(tiny.id).pipe(Effect.forkChild);
 while(!h.pending.has(SILERO_VAD.id))yield* TestClock.adjust(0);
 yield* TestClock.adjust(120_000);h.record('installation-without-silero',yield* Effect.result(Fiber.join(installation)));
})));
const audioOptions=partial=>Schema.decodeUnknownSync(SpeechTranscriptionOptions)({...options,...partial});
const profile={supportsLongForm:true,supportsVocabulary:true,languages:['en-US','de-DE']};
const utterance=new Uint8Array(new Float32Array([0.25]).buffer);
for(const translate of [false,true])cases.push(await scenario('buffered-native-text-translation-'+translate,(service,h)=>Effect.gen(function*(){
 native.profile=profile;native.replyText='um oh three';
 const definition=translate?SPEECH_MODELS.find(model=>model.supportsTranslation&&model.languages.includes('fr')):tiny;
 if(!definition)throw Error('Missing native multilingual fixture');h.settings().speechModelId=definition.id;h.ready.add(definition.id);
 const input=audioOptions({speechLanguage:translate?'fr':'en',speechTranslateToEnglish:translate,speechRemoveFillerWords:true,speechCustomWords:[{term:'O3',aliases:['oh three']}]});
 h.observations.push({label:'input',modelId:definition.id,options:input});
 h.record('transcript',yield* Effect.result(service.transcribe(utterance,input)));
})));
cases.push(await scenario('buffered-automatic-cpu-retry-and-setting-memo-fence',(service,h)=>Effect.gen(function*(){
 native.profile=profile;native.replyText='um oh three';native.autoGpu=native.failGpu=true;h.settings().speechAcceleration='auto';
 const input=audioOptions({speechRemoveFillerWords:true,speechCustomWords:[{term:'O3',aliases:['oh three']}]});
 h.observations.push({label:'input',modelId:tiny.id,options:input});
 h.record('gpu-failure-recovers-once',yield* Effect.result(service.transcribe(utterance,input)));
 h.record('next-call-uses-cpu-memo',yield* Effect.result(service.transcribe(utterance,input)));
 h.settings().speechAcceleration='cpu';h.record('explicit-cpu',yield* Effect.result(service.transcribe(utterance,input)));
 h.settings().speechAcceleration='auto';h.record('changed-setting-cleared-memo',yield* Effect.result(service.transcribe(utterance,input)));
})));
cases.push(await scenario('buffered-explicit-cpu-failure-is-not-retried',(service,h)=>Effect.gen(function*(){
 native.profile=profile;native.failAll=true;
 h.record('failure',yield* Effect.result(service.transcribe(utterance,audioOptions({}))));
})));
cases.push(await scenario('buffered-invalid-audio-and-missing-model',(service,h)=>Effect.gen(function*(){
 for(const [name,bytes] of [['empty',new Uint8Array()],['unaligned',new Uint8Array(3)],['nan',new Uint8Array(new Float32Array([NaN]).buffer)]])h.record(name,yield* Effect.result(service.transcribe(bytes,audioOptions({}))));
 h.ready.delete(tiny.id);h.record('missing',yield* Effect.result(service.transcribe(utterance,audioOptions({}))));
})));
cases.push(await scenario('stream-native-cpu-replay-revision-and-reservation',(service,h)=>Effect.gen(function*(){
 native.streamMode=true;native.autoGpu=native.failGpu=true;native.profile={supportsStreaming:true,supportsLongForm:true,supportsVocabulary:true,languages:['en']};h.settings().speechAcceleration='auto';
 const input=audioOptions({speechLanguage:'en',speechRemoveFillerWords:true,speechCustomWords:[{term:'O3',aliases:['oh three']}]});
 yield* Effect.scoped(Effect.gen(function*(){
  const stream=yield* service.startStream(input);
  h.record('prepare-busy',yield* Effect.result(service.prepareModel));
  h.record('first',yield* Effect.result(stream.feed(utterance)));
  h.record('recovered',yield* Effect.result(stream.feed(utterance)));
  h.record('after-recovery',yield* Effect.result(stream.feed(utterance)));
  h.record('finished',yield* Effect.result(stream.finish));
  h.record('feed-after-finish',yield* Effect.result(stream.feed(utterance)));
  h.record('finished-but-still-reserved',yield* Effect.result(service.prepareModel));
 }));
 h.record('released',yield* Effect.result(service.prepareModel));
})));
cases.push(await scenario('stream-unfinished-reset-retains-model',(service,h)=>Effect.gen(function*(){
 native.streamMode=true;native.profile={supportsStreaming:true,supportsLongForm:true,languages:['en']};
 yield* Effect.scoped(Effect.gen(function*(){
  const stream=yield* service.startStream(audioOptions({}));
  for(const [name,bytes] of [['empty',new Uint8Array()],['unaligned',new Uint8Array(3)],['chunk-limit',new Uint8Array(64004)]])h.record(name,yield* Effect.result(stream.feed(bytes)));
  h.record('valid-after-invalid',yield* Effect.result(stream.feed(utterance)));
 }));
 h.record('cached-after-reset',yield* Effect.result(service.prepareModel));
})));
cases.push(await scenario('cached-model-still-requires-current-installed-receipt',(service,h)=>Effect.gen(function*(){
 native.profile={supportsStreaming:true,supportsLongForm:true};
 h.record('loaded',yield* Effect.result(service.prepareModel));
 h.ready.delete(tiny.id);
 h.record('missing-even-when-cached',yield* Effect.result(service.prepareModel));
 h.record('buffered-missing-even-when-cached',yield* Effect.result(service.transcribe(utterance,audioOptions({}))));
 h.ready.add(tiny.id);
 h.record('restored-reuses-cache',yield* Effect.result(service.prepareModel));
})));
fs.writeFileSync(process.argv[2],JSON.stringify({nativeRevision:'83a3674ffd84ffb33de768210c806cb1ac53fd80',silero:SILERO_VAD,cases},null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({cases:cases.length,wholeService:true,paidOrNativeInference:false}));
