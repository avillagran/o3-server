// Synthetic protocol fixture only; this is not transcribe-cpp inference.
import fs from 'node:fs';
import path from 'node:path';
const events=path.join(process.env.HOME,'events.jsonl');
const record=value=>fs.appendFileSync(events,JSON.stringify({pid:process.pid,parent:process.ppid,cwd:process.cwd(),...value})+'\n');
process.send?.({type:'unrelated-synthetic-control-message'});
export const TranscribeModel={load:async(model,placement)=>{
 const mode=fs.readFileSync(model,'utf8');
 record({kind:'load',mode,backend:placement.backend,environment:Object.keys(process.env)});
 if(mode.startsWith('vad:'))return {
  vadInfo:{sampleRate:16000,frameSamples:512},dispose(){record({kind:'vad-dispose'});},
  createVadSession(){let samples=0,scored=0;const score=total=>{const firstFrame=scored;const count=Math.max(0,total-scored);scored=total;return {firstFrame,probs:new Float32Array(count).fill(1)};};return {
   async streamReset(){samples=scored=0;record({kind:'vad-reset'});},
   async streamFeed(pcm){samples+=pcm.length;record({kind:'vad-feed',length:pcm.length});return score(Math.floor(samples/512));},
   async streamFlush(){return score(Math.ceil(samples/512));},dispose(){}
  };}
 };

 if(mode==='fallback'&&placement.backend!=='cpu')throw Error('Synthetic automatic backend failure');
 return {backend:(mode==='gpu-inference'||mode==='gpu-stream')&&placement.backend!=='cpu'?'fixture-gpu':'cpu',capabilities:{supportsStreaming:mode==='stream'||mode==='gpu-stream'||mode==='stream-hang',languages:mode==='gpu-stream'?['en']:['en-US',17,'de-DE'],supportsTranslate:true,maxAudioMs:0},supports:feature=>feature==='long_form'||feature==='vocabulary',
 transcribe:async(pcm,options)=>{
  record({kind:'transcribe',mode,length:pcm.length,language:options.language,options});
  if(mode==='gpu-inference'&&placement.backend!=='cpu')throw Error('Synthetic GPU inference failed');
  if(mode==='fail-inference')throw Error('Synthetic native inference failed');
  if(mode.startsWith('text:')||mode==='gpu-inference')return {text:mode.startsWith('text:')?mode.slice(5):'um oh three'};
  if(mode==='hang'){fs.writeFileSync(path.join(process.env.HOME,'entered.staging'),String(process.pid));fs.renameSync(path.join(process.env.HOME,'entered.staging'),path.join(process.env.HOME,'entered'));while(true){}}
  if(mode==='exit')process.exit(47);
  if(mode==='disconnect'){process.disconnect();await new Promise(()=>{});}
  return {text:JSON.stringify({length:pcm.length,bytes:pcm.byteLength,first:pcm[0],last:pcm[pcm.length-1],options})};
 },
 createSession:()=>({dispose(){record({kind:'session-dispose'});},stream:async options=>{
  let step=0;const text={full:'',committed:'',tentative:''};
  return {text,feed:async pcm=>{
   if(mode==='stream-hang'){record({kind:'stream-feed-entered'});fs.writeFileSync(path.join(process.env.HOME,'entered.staging'),String(process.pid));fs.renameSync(path.join(process.env.HOME,'entered.staging'),path.join(process.env.HOME,'entered'));while(true){}}
   if(mode==='gpu-stream'){record({kind:'feed',backend:placement.backend,length:pcm.length});if(placement.backend!=='cpu'&&step===1)throw Error('Synthetic native stream feed failed');step++;text.full=text.committed='oh three '+step;text.tentative='um';return {committedChanged:true,tentativeChanged:true};}
   text.full=text.committed=options.language+':'+pcm[0];record({kind:'feed',language:options.language});return {committedChanged:true,tentativeChanged:false};},finalize:async()=>{if(mode==='gpu-stream')text.full='um oh three';},reset(){record({kind:'stream-reset'});}};
 }})};
}};

export const getAvailableBackends=()=>[{deviceType:'gpu',kind:'synthetic',deviceId:7,name:'Synthetic GPU'}];
