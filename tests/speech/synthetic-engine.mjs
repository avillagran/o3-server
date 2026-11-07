// Synthetic protocol fixture only; this is not transcribe-cpp inference.
import fs from 'node:fs';
import path from 'node:path';
const events=path.join(process.env.HOME,'events.jsonl');
const record=value=>fs.appendFileSync(events,JSON.stringify({pid:process.pid,...value})+'\n');
process.send?.({type:'unrelated-synthetic-control-message'});
export const TranscribeModel={load:async(model,placement)=>{
 const mode=fs.readFileSync(model,'utf8');
 record({kind:'load',mode,backend:placement.backend,environment:Object.keys(process.env)});
 if(mode==='fallback'&&placement.backend!=='cpu')throw Error('Synthetic automatic backend failure');
 return {backend:'cpu',capabilities:{supportsStreaming:mode==='stream',languages:['en-US',17,'de-DE'],supportsTranslate:true,maxAudioMs:0},supports:feature=>feature==='long_form'||feature==='vocabulary',
 transcribe:async(pcm,options)=>{
  record({kind:'transcribe',mode,length:pcm.length,language:options.language});
  if(mode==='hang'){fs.writeFileSync(path.join(process.env.HOME,'entered'),String(process.pid));while(true){}}
  if(mode==='exit')process.exit(47);
  if(mode==='disconnect'){process.disconnect();await new Promise(()=>{});}
  return {text:JSON.stringify({length:pcm.length,bytes:pcm.byteLength,first:pcm[0],last:pcm[pcm.length-1],options})};
 },
 createSession:()=>({dispose(){record({kind:'session-dispose'});},stream:async options=>{
  const text={full:'',committed:'',tentative:''};
  return {text,feed:async pcm=>{text.full=text.committed=options.language+':'+pcm[0];record({kind:'feed',language:options.language});return {committedChanged:true,tentativeChanged:false};},finalize:async()=>{},reset(){}};
 }})};
}};
