export const state={events:[],profile:{},replyText:'',autoGpu:false,failGpu:false,failAll:false};
export async function listNativeSpeechGpuDevices(){state.events.push({kind:'discover'});return [];}
export async function loadNativeSpeechModel(path,signal,moduleUrl,acceleration){
 state.events.push({kind:'load',path,acceleration});
 let revision=0;
 const backend=state.autoGpu&&acceleration!=='cpu'?'fixture-gpu':'CPU';
 return {backend,languages:['en'],supportsStreaming:false,supportsLongForm:false,supportsTranslation:false,supportsVocabulary:false,...state.profile,
  begin:async(options,filterSilence,vad)=>{if(state.streamMode){revision=0;state.events.push({kind:'begin',backend,options,filterSilence,vad:vad??null});}},feed:async pcm=>{if(!state.streamMode)return {revision:1,text:{committed:'',tentative:''}};state.events.push({kind:'feed',backend,length:pcm.length});if(state.failGpu&&backend!=='CPU'&&revision===1)throw Error('Synthetic native stream feed failed');revision++;return {revision,text:{committed:'oh three '+revision,tentative:'um'}};},finish:async()=>{if(state.streamMode)state.events.push({kind:'finish',backend});return state.streamMode?'um oh three':'';},reset:async()=>{if(state.streamMode)state.events.push({kind:'reset',backend});},transcribe:async(pcm,options,filterSilence,vad)=>{state.events.push({kind:'transcribe',options,filterSilence,vad:vad??null,...(state.autoGpu?{backend}:{})});if(state.failAll||(state.failGpu&&backend!=='CPU'))throw Error('Synthetic native inference failed');return {text:state.replyText};},dispose:async()=>{state.events.push({kind:'dispose'});}};
}
