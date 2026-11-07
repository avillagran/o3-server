import {registerHooks,createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import fs from 'node:fs';
const require=createRequire('/deps/package.json');let resolving=false;
registerHooks({resolve(specifier,context,next){
 const token=new URL('./tokens.mjs',import.meta.url).href;
 if(context.parentURL?.startsWith('file:///native/apps/server/')){
  if(specifier.endsWith('/serverSettings.ts')||specifier==='../serverSettings.ts')return {url:new URL('../speech-service-4314/settings.mjs',import.meta.url).href,shortCircuit:true};
  if(specifier==='./native.ts'&&context.parentURL.endsWith('/speech/SpeechService.ts'))return {url:new URL('../speech-service-4314/native.mjs',import.meta.url).href,shortCircuit:true};
  if(specifier==='../config.ts'&&context.parentURL.endsWith('/speech/SpeechModelStore.ts'))return {url:new URL('../speech-model-store-4314/config.mjs',import.meta.url).href,shortCircuit:true};
  if(specifier==='./CloudSpeech.ts'||specifier.endsWith('/textGeneration/TextGeneration.ts')||specifier==='./EnvironmentAuth.ts'||specifier==='./SessionStore.ts'||specifier==='./dpop.ts')return {url:token,shortCircuit:true};
 }
 if(specifier==='@t3tools/contracts')return {url:'file:///native/packages/contracts/src/index.ts',shortCircuit:true};
 if(specifier.startsWith('@t3tools/shared/'))return {url:'file:///native/packages/shared/src/'+specifier.slice('@t3tools/shared/'.length)+'.ts',shortCircuit:true};
 if(!resolving&&(specifier==='effect'||specifier.startsWith('effect/'))){resolving=true;try{return {url:pathToFileURL(require.resolve(specifier)).href,shortCircuit:true};}finally{resolving=false;}}
 if(!resolving&&specifier.startsWith('@effect/platform-node/')){resolving=true;try{return {url:pathToFileURL(require.resolve(specifier)).href,shortCircuit:true};}finally{resolving=false;}}
 return next(specifier,context);
}});
const imported=createRequire('/native/apps/server/src/speech/http.ts').resolve('effect/Effect'),direct=require.resolve('effect/Effect');
if(imported!==direct)throw Error('Multiple Effect identities');
fs.writeFileSync('/out/canonical-effect.json',JSON.stringify({imported,direct})+'\n',{flag:'wx'});
