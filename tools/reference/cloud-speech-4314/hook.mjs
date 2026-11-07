import {registerHooks,createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import fs from 'node:fs';
import crypto from 'node:crypto';
const require=createRequire('/deps/package.json');let resolving=false;
const loaded=new Map();
registerHooks({
 resolve(specifier,context,next){
  if(context.parentURL?.startsWith('file:///native/apps/server/')){
   if(specifier==='./secretProviderDefinitions.ts'&&context.parentURL.endsWith('/secrets/SecretResolver.ts'))return {url:new URL('./secret-drivers.mjs',import.meta.url).href,shortCircuit:true};
   if(specifier==='../serverSettings.ts')return {url:new URL('../speech-service-4314/settings.mjs',import.meta.url).href,shortCircuit:true};
   if(specifier==='./native.ts'&&context.parentURL.endsWith('/speech/SpeechService.ts'))return {url:new URL('../speech-service-4314/native.mjs',import.meta.url).href,shortCircuit:true};
   if(specifier==='../config.ts'&&context.parentURL.endsWith('/speech/SpeechModelStore.ts'))return {url:new URL('../speech-model-store-4314/config.mjs',import.meta.url).href,shortCircuit:true};
   if(specifier==='./EnvironmentAuth.ts'||specifier==='./SessionStore.ts'||specifier==='./dpop.ts')return {url:new URL('../speech-http-4314/tokens.mjs',import.meta.url).href,shortCircuit:true};
   if(specifier==='../textGeneration/TextGeneration.ts')return {url:new URL('./tokens.mjs',import.meta.url).href,shortCircuit:true};
  }
  if(specifier==='@t3tools/contracts')return {url:'file:///native/packages/contracts/src/index.ts',shortCircuit:true};
  for(const [alias,directory] of [['@t3tools/shared/','shared'],['@t3tools/secret-provider-core/','secret-provider-core']])if(specifier.startsWith(alias))return {url:'file:///native/packages/'+directory+'/src/'+specifier.slice(alias.length)+'.ts',shortCircuit:true};
  if(!resolving&&(specifier==='effect'||specifier.startsWith('effect/')||specifier.startsWith('@effect/platform-node/'))){resolving=true;try{return {url:pathToFileURL(require.resolve(specifier)).href,shortCircuit:true};}finally{resolving=false;}}
  return next(specifier,context);
 },
 load(url,context,next){
  if(url.startsWith('file:///native/')){
   const bytes=fs.readFileSync(new URL(url));loaded.set(url.slice('file:///native/'.length),{bytes:bytes.length,sha256:crypto.createHash('sha256').update(bytes).digest('hex')});
   fs.writeFileSync('/out/loaded-sources.json',JSON.stringify(Object.fromEntries(loaded),null,2)+'\n');
  }
  return next(url,context);
 }
});
const imported=createRequire('/native/apps/server/src/speech/CloudSpeech.ts').resolve('effect/Effect'),direct=require.resolve('effect/Effect');
if(imported!==direct)throw Error('Multiple Effect identities');
fs.writeFileSync('/out/canonical-effect.json',JSON.stringify({imported,direct})+'\n',{flag:'wx'});
