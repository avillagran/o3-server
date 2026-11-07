import * as Effect from 'effect/Effect';
import * as Deferred from 'effect/Deferred';
import * as Fiber from 'effect/Fiber';
import * as Exit from 'effect/Exit';
import * as TestClock from 'effect/testing/TestClock';
import * as HostProcess from 'file:///native/packages/shared/src/HostProcess.ts';
import * as Source from 'file:///native/apps/server/src/speech/SpeechService.ts';
import * as Store from 'file:///native/apps/server/src/speech/SpeechModelStore.ts';
import {SPEECH_MODELS,DEFAULT_SPEECH_MODEL_ID} from 'file:///native/apps/server/src/speech/model.ts';
import {ServerSettingsService} from './settings.mjs';
import {state as native} from './native.mjs';
import fs from 'node:fs';
if(process.versions.node!=='26.13.0')throw Error('Exact Node26.13 required');
const tiny=SPEECH_MODELS.find(model=>model.id==='handy-computer/moonshine-tiny-gguf');
const normalize=result=>result._tag==='Success'?{ok:true,value:result.success??null}:{ok:false,tag:result.failure._tag,operation:result.failure.operation??null,modelId:result.failure.modelId??null,message:result.failure.message};
async function scenario(name,run){
 native.events=[];const events=native.events;
 let settings={speechModelId:tiny.id,speechAcceleration:'cpu',speechModelUnloadTimeout:'min_2',speechLanguage:'auto',speechRemoveFillerWords:false,speechCustomWords:[]};const ready=new Set([tiny.id]);const controlled=new Set(),pending=new Map();
 const serviceSettings={getSettings:Effect.sync(()=>({...settings})),updateSettings:patch=>Effect.sync(()=>{events.push({kind:'settings',patch});settings={...settings,...patch};})};
 const store={isReady:model=>Effect.sync(()=>ready.has(model.id)),installedPath:model=>Effect.sync(()=>ready.has(model.id)?'/fixture/models/'+model.filename:undefined),download:(model,progress)=>Effect.gen(function*(){
 events.push({kind:'download',id:model.id});progress?.onProgress?.(0);
 if(controlled.has(model.id)){const gate=Deferred.makeUnsafe();pending.set(model.id,{gate,progress});yield* Deferred.await(gate);pending.delete(model.id);}
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
fs.writeFileSync(process.argv[2],JSON.stringify({nativeRevision:'83a3674ffd84ffb33de768210c806cb1ac53fd80',cases},null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({cases:cases.length,wholeService:true,paidOrNativeInference:false}));
