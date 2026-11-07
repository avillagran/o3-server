// Private owned transport for the complete pinned native speech factory.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {pathToFileURL} from 'node:url';

const MAX_RECORD=32*1024*1024; // Including LF; buffered PCM can occupy19,200,000 raw bytes.
const MAX_PCM=19_200_000;
const directory=process.argv[2];
const expectedSpec=process.argv[3];
if(!path.isAbsolute(directory??''))throw Error('Invalid owned runtime directory');
function readOwned(file,maximum){
 const fd=fs.openSync(file,fs.constants.O_RDONLY|fs.constants.O_NOFOLLOW|fs.constants.O_NONBLOCK);
 try{
  const before=fs.fstatSync(fd);
  if(!before.isFile()||before.size>maximum)throw Error('Owned resource exceeds its bound');
  const buffer=Buffer.allocUnsafe(maximum+1);let length=0;
  while(length<=maximum){const count=fs.readSync(fd,buffer,length,maximum+1-length,null);if(!count)break;length+=count;}
  const after=fs.fstatSync(fd);
  if(length>maximum||['dev','ino','size','mtimeMs','ctimeMs'].some(key=>before[key]!==after[key]))throw Error('Changed owned resource');
  return buffer.subarray(0,length);
 }finally{fs.closeSync(fd);}
}
const bytes=readOwned(path.join(directory,'spec.json'),128*1024);
if(!/^[a-f0-9]{64}$/.test(expectedSpec??'')||crypto.createHash('sha256').update(bytes).digest('hex')!==expectedSpec)throw Error('Invalid owned speech specification');
const spec=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(bytes));
if(process.versions.node!=='26.13.0'||spec.version!==1||!/^[A-Za-z0-9_-]{16,128}$/.test(spec.owner??''))throw Error('Unsupported owned speech runtime');
const nativeBytes=readOwned(path.join(directory,'native-worker.mjs'),1024*1024);
const manifest=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(readOwned(path.join(directory,'native-manifest.json'),64*1024)));
const nativeHash=crypto.createHash('sha256').update(nativeBytes).digest('hex');
if(nativeHash!==spec.nativeSha256||nativeHash!==manifest.files['native-worker.mjs'].sha256)throw Error('Changed native speech resource');
// Explicit engine URLs make the native ambient package fallback unreachable.
// Import checked bytes directly, rather than reopening a mutable pathname.
const native=await import('data:text/javascript;base64,'+nativeBytes.toString('base64'));
const textManifestBytes=readOwned(path.join(directory,'text-manifest.json'),64*1024);
if(crypto.createHash('sha256').update(textManifestBytes).digest('hex')!==spec.textManifestSha256)throw Error('Changed speech text manifest');
const textManifest=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(textManifestBytes));
async function textModule(name){
 const bytes=readOwned(path.join(directory,'text-'+name),256*1024),expected=textManifest.files[name];
 if(!expected||bytes.length!==expected.bytes||crypto.createHash('sha256').update(bytes).digest('hex')!==expected.sha256)throw Error('Changed speech text resource');
 return await import('data:text/javascript;base64,'+bytes.toString('base64'));
}
const [dictionary,filler,language,catalog,prompt]=await Promise.all(['custom-words.mjs','filler-words.mjs','native-language.mjs','model.mjs','transcription-post-processing.mjs'].map(textModule));
function serviceContext(modelId,options,streaming=false){
 const definition=catalog.getSpeechModel(modelId);if(!definition||!options||Array.isArray(options)||typeof options!=='object')throw Error('Invalid owned service options');
 const intent=catalog.effectiveSpeechLanguage(definition,options.speechLanguage);
 const translate=options.speechTranslateToEnglish&&(streaming?(!model.supportsStreaming&&model.supportsTranslation):definition.supportsTranslation)&&intent!=='en';
 const words=dictionary.normalizeSpeechCustomWords(options.speechCustomWords),terms=words.map(word=>word.term);
 const vocabulary=model.supportsVocabulary&&!translate?terms:[];
 const hint=language.nativeSpeechLanguage(model.languages,intent);
 const prepared={timestamps:'none',...(hint?{language:hint}:{}),...(translate?{task:'translate',targetLanguage:'en'}:{}),...(vocabulary.length?{vocabulary}: {})};
 const correct=dictionary.makeSpeechTextCorrector(vocabulary.length?[]:terms,words);
 return {prepared,correct,finalize(text){const corrected=correct(text);return (options.speechRemoveFillerWords?filler.removeSpeechFillerWords(corrected,translate?'en':intent==='auto'?undefined:intent,options.speechCorrectionWord):corrected).trim();}};
}
const controller=new AbortController();
let model,loading,closing,streamContext;
let vad=spec.vad;
const leases=[];
async function seal(file){
 if(!file||!path.isAbsolute(file.path)||!/^[a-f0-9]{64}$/.test(file.sha256)||!/^[1-9][0-9]*$/.test(file.bytes))throw Error('Invalid sealed speech input');
 const handle=await fs.promises.open(file.path,fs.constants.O_RDONLY|fs.constants.O_NOFOLLOW|fs.constants.O_NONBLOCK);
 leases.push(handle);
 const before=await handle.stat({bigint:true});
 if(!before.isFile()||before.size.toString()!==file.bytes)throw Error('Changed sealed speech input');
 const hash=crypto.createHash('sha256');
 const stream=fs.createReadStream(file.path,{fd:handle.fd,autoClose:false,highWaterMark:64*1024,signal:controller.signal});
 for await(const block of stream)hash.update(block);
 const after=await handle.stat({bigint:true});
 if(hash.digest('hex')!==file.sha256||['dev','ino','size','mtimeNs','ctimeNs'].some(key=>before[key]!==after[key]))throw Error('Changed sealed speech input');
 return pathToFileURL(file.path).href;
}
function pcm(value){
 if(typeof value!=='string'||value.length>MAX_PCM/3*4)throw Error('Invalid owned PCM frame');
 const bytes=Buffer.from(value,'base64');
 if(bytes.length===0||bytes.length>MAX_PCM||bytes.length%4!==0||bytes.toString('base64')!==value)throw Error('Invalid owned PCM frame');
 const values=new Float32Array(Uint8Array.from(bytes).buffer);
 for(const sample of values)if(!Number.isFinite(sample)||sample < -1||sample > 1)throw Error('Invalid owned PCM sample');
 return values;
}
async function send(value){
 const text=JSON.stringify(value)+'\n';
 if(Buffer.byteLength(text)>MAX_RECORD)throw Error('Owned speech response exceeds32MiB');
 await new Promise((resolve,reject)=>process.stdout.write(text,error=>error?reject(error):resolve()));
}
async function stop(code=0){
 if(closing)return closing;
 closing=(async()=>{
  controller.abort();
  if(loading)await loading.catch(()=>undefined);
  if(model)await model.dispose().catch(()=>undefined);
  for(const handle of leases)await handle.close().catch(()=>undefined);
  process.exit(code);
 })();
 return closing;
}
const fatal=()=>{void stop(70);};
process.on('SIGTERM',()=>{void stop();});
process.on('SIGINT',()=>{void stop();});
process.stdin.on('end',()=>{void stop();});
process.stdin.on('error',fatal);
process.stdout.on('error',fatal);
async function dispatch(frame){
 if(!frame||Array.isArray(frame)||frame.owner!==spec.owner||!Number.isSafeInteger(frame.id)||frame.id<1||typeof frame.kind!=='string')throw Error('Invalid owned speech command');
 const id=frame.id;
 try{
  let result=null;
  if(frame.kind==='text-ready'&&spec.mode==='text'){result=true;
  }else if(frame.kind==='cloud-correct'){
   if(typeof frame.text!=='string'||typeof frame.language!=='string'||!frame.options||!Array.isArray(frame.options.speechCustomWords))throw Error('Invalid owned cloud text');
   const corrected=dictionary.applySpeechAliases(frame.text,frame.options.speechCustomWords);
   result=(frame.options.speechRemoveFillerWords?filler.removeSpeechFillerWords(corrected,frame.language,frame.options.speechCorrectionWord):corrected).trim();
  }else if(frame.kind==='postprocess-prompt'){
   if(typeof frame.instructions!=='string'||typeof frame.transcript!=='string'||typeof frame.correction!=='string'||typeof frame.draft?.text!=='string'||typeof frame.draft?.selection?.start!=='number'||typeof frame.draft?.selection?.end!=='number')throw Error('Invalid owned cleanup prompt');
   const text=prompt.buildTranscriptionPostProcessingPrompt(frame.instructions,frame.transcript,frame.draft,frame.correction);
   result=Array.from({length:text.length},(_,index)=>text.charCodeAt(index));
  }else if(frame.kind==='postprocess-correct'){
   if(typeof frame.text!=='string'||typeof frame.original!=='string'||!Array.isArray(frame.words))throw Error('Invalid owned postprocessing text');
   const text=frame.text.trim();result=text.length>0?dictionary.applySpeechAliases(text,frame.words):frame.original;
  }else if(spec.mode==='text')throw Error('Text resource has no native engine authority');
  else if(frame.kind==='load'){
   if(model||loading||!spec.model)throw Error('Invalid owned model load');
   loading=(async()=>{
    const engine=await seal(spec.engine);
    await seal(spec.model);
    if(spec.vad)await seal(spec.vad);
    return await native.loadNativeSpeechModel(spec.model.path,controller.signal,engine,spec.acceleration);
   })();
   model=await loading;
   result={backend:model.backend,languages:model.languages,supportsStreaming:model.supportsStreaming,supportsLongForm:model.supportsLongForm,supportsTranslation:model.supportsTranslation,supportsVocabulary:model.supportsVocabulary};
  }else if(frame.kind==='vad'){
   if(!model||vad)throw Error('Invalid owned VAD binding');
   if(path.dirname(frame.vad?.path??'')!==directory||!/^vad-[A-Za-z0-9_-]+$/.test(path.basename(frame.vad.path)))throw Error('Invalid owned VAD path');
   if(!/^[1-9][0-9]*$/.test(frame.vad.bytes)||BigInt(frame.vad.bytes)>2097152n)throw Error('Invalid owned VAD size');
   await seal(frame.vad);vad=frame.vad;
  }else if(frame.kind==='discover'){
   if(model||loading)throw Error('Invalid owned device discovery');
   const engine=await seal(spec.engine);
   result=await native.listNativeSpeechGpuDevices(engine);
  }else{
   if(!model)throw Error('Owned speech model is not loaded');
   if(frame.kind==='service-transcribe'){const context=serviceContext(frame.modelId,frame.options);const raw=await model.transcribe(pcm(frame.pcm),context.prepared,frame.options.speechSilenceFiltering===true,vad?.path);result=context.finalize(raw.text);}
   else if(frame.kind==='transcribe')result=await model.transcribe(pcm(frame.pcm),frame.options,frame.filterSilence===true,vad?.path);
   else if(frame.kind==='service-begin'){const context=serviceContext(frame.modelId,frame.options,true);await model.begin(context.prepared,frame.options.speechSilenceFiltering===true,vad?.path);streamContext=context;}
   else if(frame.kind==='begin'){await model.begin(frame.options,frame.filterSilence===true,vad?.path);streamContext=undefined;}
   else if(frame.kind==='feed'){result=await model.feed(pcm(frame.pcm));if(streamContext&&result?.text)result={...result,text:{committed:streamContext.correct(result.text.committed),tentative:streamContext.correct(result.text.tentative)}};}
   else if(frame.kind==='finish'){const raw=await model.finish();result=streamContext?streamContext.finalize(raw):raw;streamContext=undefined;}
   else if(frame.kind==='reset'){await model.reset();streamContext=undefined;}
   else throw Error('Unknown owned speech command');
  }
  if(!closing)await send({type:'reply',owner:spec.owner,id,ok:true,result});
 }catch(error){
  if(!closing)await send({type:'reply',owner:spec.owner,id,ok:false,error:typeof error?.message==='string'?error.message:'Native speech failed.'});
 }
}
let fragments=[],length=0;
const decoder=new TextDecoder('utf-8',{fatal:true});
process.stdin.on('data',chunk=>{
 if(closing)return;
 try{
  let start=0;
  for(let index=0;index<chunk.length;index++)if(chunk[index]===10){
   const part=chunk.subarray(start,index+1);
   if(length+part.length>MAX_RECORD)throw Error('Owned speech input exceeds32MiB');
   fragments.push(part);length+=part.length;
   const record=Buffer.concat(fragments,length);fragments=[];length=0;start=index+1;
   const frame=JSON.parse(decoder.decode(record.subarray(0,record.length-1)));
   void dispatch(frame).catch(fatal);
  }
  if(start<chunk.length){
   const part=chunk.subarray(start);
   if(length+part.length>MAX_RECORD)throw Error('Owned speech input exceeds32MiB');
   fragments.push(part);length+=part.length;
  }
 }catch{fatal();}
});
await send({type:'hello',owner:spec.owner,node:process.versions.node,pid:process.pid});
