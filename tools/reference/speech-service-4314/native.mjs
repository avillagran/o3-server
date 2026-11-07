export const state={events:[]};
export async function listNativeSpeechGpuDevices(){state.events.push({kind:'discover'});return [];}
export async function loadNativeSpeechModel(path,signal,moduleUrl,acceleration){
 state.events.push({kind:'load',path,acceleration});
 return {backend:'CPU',languages:['en'],supportsStreaming:false,supportsLongForm:false,supportsTranslation:false,supportsVocabulary:false,
  begin:async()=>{},feed:async()=>({revision:1,text:{committed:'',tentative:''}}),finish:async()=>'',reset:async()=>{},transcribe:async(pcm,options,filterSilence,vad)=>{state.events.push({kind:'transcribe',options,filterSilence,vad:vad??null});return {text:''};},dispose:async()=>{state.events.push({kind:'dispose'});}};
}
