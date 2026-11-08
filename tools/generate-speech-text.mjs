// Preserve complete native helpers; no replacement text-normalization model.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [source,output]=process.argv.slice(2);
const pin='83a3674ffd84ffb33de768210c806cb1ac53fd80';
const intake=JSON.parse(fs.readFileSync(path.join(source,'SOURCE.json')));
if(process.versions.node!=='26.13.0'||intake.revision!==pin)throw Error('Exact Node26.13 and native4314 required');
const sha=data=>crypto.createHash('sha256').update(data).digest('hex');
function read(name){const bytes=fs.readFileSync(path.join(source,name));if(sha(bytes)!==intake.files[name])throw Error('Changed source '+name);return bytes.toString();}
const license=read('LICENSE');
const files={},functions={};
const mappings=[['customWords.ts','custom-words.mjs'],['fillerWords.ts','filler-words.mjs'],['nativeLanguage.ts','native-language.mjs'],['model.ts','model.mjs'],['../textGeneration/TranscriptionPostProcessing.ts','transcription-post-processing.mjs']];
for(const [original,target] of mappings){
 const full=path.posix.normalize('apps/server/src/speech/'+original);
 let erased=stripTypeScriptTypes(read(full),{mode:'strip'});
 if(original==='model.ts'){
  const needle='import catalog from "./catalog.json" with { type: "json" };';
  if(erased.split(needle).length!==2)throw Error('Expected whole catalog dependency');
  erased=erased.replace(needle,'const catalog = '+read('apps/server/src/speech/catalog.json')+';');
  const shared=stripTypeScriptTypes(read('packages/shared/src/speech.ts'),{mode:'strip'});
  const start=shared.indexOf('export function effectiveSpeechLanguage('),end=shared.indexOf('\n/**',start);
  if(start<0||end<0)throw Error('Expected complete effective-language function');
  const binding='export { effectiveSpeechLanguage } from "@t3tools/shared/speech";';
  if(erased.split(binding).length!==2)throw Error('Expected effective-language dependency');
  erased=erased.replace(binding,shared.slice(start,end).trim());
 }
 const bytes='/*\n'+license+'\nSource: T3 Code nightly4314 '+pin+'\nComplete original module after type erasure; model catalog dependency embedded.\n*/\n'+erased;
 const destination=path.join(output,target);fs.writeFileSync(destination,bytes,{flag:'wx'});
 const loaded=await import(pathToFileURL(destination).href);
 const native=await import(pathToFileURL(path.join('/native',full)).href);
 for(const [name,value] of Object.entries(native)){
  if(typeof value==='function'){
   if(loaded[name]?.toString()!==value.toString())throw Error('Changed complete native function '+name);
   functions[original+':'+name]=sha(value.toString());
  }else if(JSON.stringify(loaded[name])!==JSON.stringify(value))throw Error('Changed native data export '+name);
 }
 files[target]={bytes:Buffer.byteLength(bytes),sha256:sha(bytes)};
}
fs.writeFileSync(path.join(output,'manifest.json'),JSON.stringify({nativeRevision:pin,node:'26.13.0',sources:intake.files,files,functions},null,2)+'\n',{flag:'wx'});
console.log(JSON.stringify({files,functions:Object.keys(functions).length}));
