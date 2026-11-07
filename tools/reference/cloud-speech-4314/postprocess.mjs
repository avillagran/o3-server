import * as Effect from 'effect/Effect';
import * as Layer from 'effect/Layer';
import * as FileSystem from 'effect/FileSystem';
import * as Schema from 'effect/Schema';
import {DEFAULT_SERVER_SETTINGS,DEFAULT_SPEECH_TRANSCRIPTION_OPTIONS,EnvironmentSpeechPostProcessingRequest} from 'file:///native/packages/contracts/src/index.ts';
import * as Source from 'file:///native/apps/server/src/speech/postProcessing.ts';
import {buildTranscriptionPostProcessingPrompt} from 'file:///native/apps/server/src/textGeneration/TranscriptionPostProcessing.ts';
import {ServerSettingsService} from '../speech-service-4314/settings.mjs';
import {TextGeneration} from './tokens.mjs';
import fs from 'node:fs';
const units=value=>Array.from({length:value.length},(_,index)=>value.charCodeAt(index));
const cases=[];
const defaults={transcript:'  oh three, please  ',draft:{text:'a draft',selection:{start:2,end:4}},options:{...DEFAULT_SPEECH_TRANSCRIPTION_OPTIONS,speechPostProcessingPrompt:{mode:"default",customInstructions:""},speechCustomWords:[{term:'T3 Code',aliases:['oh three']}],speechCorrectionWord:'correction'}};
const inputs=[
 {name:'blank-no-settings-or-provider',transcript:' \ufeff\u2028\t'},
 {name:'default-prompt'},
 {name:'custom-prompt',options:{speechPostProcessingPrompt:{mode:'custom',customInstructions:'  Keep the exact names.  '}}},
 {name:'empty-custom-falls-back',options:{speechPostProcessingPrompt:{mode:'custom',customInstructions:' \ufeff '}}},
 {name:'generated-empty-falls-back',generated:' \ufeff\n'},
 {name:'provider-fails',fail:true},
 {name:'private-model',settings:{speechPostProcessingModelSelection:{instanceId:'claudeAgent',model:'fixture-model'}}},
 {name:'disabled-model-falls-back',settings:{speechPostProcessingModelSelection:{instanceId:'claudeAgent',model:'fixture-model'},providerInstances:{claudeAgent:{driver:'claudeAgent',enabled:false,config:{},environment:[]}}}},
 ...[[-1,-1],[99,100],[2,1],[1,2],[2,3],[0,4]].map(([start,end],index)=>({name:'utf16-draft-'+index,draft:{text:'a😀b',selection:{start,end}}})),
];
for(const spec of inputs){
 const events=[];const input=Schema.decodeUnknownSync(EnvironmentSpeechPostProcessingRequest)({...defaults,...spec,draft:spec.draft??defaults.draft,options:{...defaults.options,...spec.options}});
 const settings={getSettings:Effect.sync(()=>{events.push({kind:'settings-read'});return {...DEFAULT_SERVER_SETTINGS,...spec.settings};})};
 const fileSystem=FileSystem.makeNoop({makeTempDirectoryScoped:options=>Effect.acquireRelease(Effect.sync(()=>{events.push({kind:'cwd-open',options});return '/fixture/private-cleanup-cwd';}),directory=>Effect.sync(()=>events.push({kind:'cwd-release',directory})))});
 const generation={generateTranscriptionPostProcessing:request=>Effect.gen(function*(){events.push({kind:'generation',cwd:request.cwd,promptUnits:units(request.prompt),modelSelection:request.modelSelection});if(spec.fail)return yield* Effect.fail(Error('Synthetic private provider failure'));return {transcription:spec.generated??'  oh three is ready  '};})};
 const result=await Effect.runPromise(Effect.scoped(Effect.gen(function*(){const service=yield* Source.SpeechPostProcessing;return yield* Effect.result(service.process(input));})).pipe(Effect.provide(Source.layer),Effect.provideService(ServerSettingsService,settings),Effect.provideService(FileSystem.FileSystem,fileSystem),Effect.provideService(TextGeneration,generation)));
 const outcome=result._tag==='Success'?{ok:true,value:result.success}:{ok:false,tag:result.failure._tag,operation:result.failure.operation,message:result.failure.message};
 cases.push({name:spec.name,input:{...input,options:input.options},settings:spec.settings??{},generated:spec.generated??'  oh three is ready  ',fails:spec.fail??false,events,outcome});
}
const promptCases=[];
for(const draft of [{text:'a😀b',selection:{start:1,end:2}},{text:'a😀b',selection:{start:2,end:3}},{text:'a😀b',selection:{start:-3,end:90}},{text:'abc',selection:{start:2,end:1}}])for(const correction of ['',' "cancel"\n','\ufeffundo\ufeff'])promptCases.push({draft,correction,resultUnits:units(buildTranscriptionPostProcessingPrompt('fixture instructions','fixture transcript',draft,correction))});
fs.writeFileSync(process.argv[2],JSON.stringify({nativeRevision:'83a3674ffd84ffb33de768210c806cb1ac53fd80',description:'Whole original scoped postProcessing layer and prompt builder; explicit logical FS/TextGeneration dependencies, no provider or filesystem effects',cases,promptCases},null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({cases:cases.length,promptCases:promptCases.length}));
