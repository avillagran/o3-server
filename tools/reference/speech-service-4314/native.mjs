export const state={events:[],profile:{},replyText:'',autoGpu:false,failGpu:false,failAll:false};
export async function listNativeSpeechGpuDevices(){state.events.push({kind:'discover'});return [];}
export async function loadNativeSpeechModel(path,signal,moduleUrl,acceleration){
 state.events.push({kind:'load',path,acceleration});
 const backend=state.autoGpu&&acceleration!=='cpu'?'fixture-gpu':'CPU';
 return {backend,languages:['en'],supportsStreaming:false,supportsLongForm:false,supportsTranslation:false,supportsVocabulary:false,...state.profile,
  begin:async()=>{},feed:async()=>({revision:1,text:{committed:'',tentative:''}}),finish:async()=>'',reset:async()=>{},transcribe:async(pcm,options,filterSilence,vad)=>{state.events.push({kind:'transcribe',options,filterSilence,vad:vad??null,...(state.autoGpu?{backend}:{})});if(state.failAll||(state.failGpu&&backend!=='CPU'))throw Error('Synthetic native inference failed');return {text:state.replyText};},dispose:async()=>{state.events.push({kind:'dispose'});}};
}
