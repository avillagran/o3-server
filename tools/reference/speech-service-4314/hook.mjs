import {registerHooks,createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import fs from 'node:fs';
import crypto from 'node:crypto';
const require=createRequire('/deps/package.json');let resolving=false;
registerHooks({resolve(specifier,context,next){
 if(specifier==='../serverSettings.ts'&&context.parentURL?.endsWith('/speech/SpeechService.ts'))return {url:new URL('./settings.mjs',import.meta.url).href,shortCircuit:true};
 if(specifier==='./native.ts'&&context.parentURL?.endsWith('/speech/SpeechService.ts'))return {url:new URL('./native.mjs',import.meta.url).href,shortCircuit:true};
 if(specifier==='../config.ts'&&context.parentURL?.endsWith('/speech/SpeechModelStore.ts'))return {url:new URL('./config.mjs',import.meta.url).href,shortCircuit:true};
 if(specifier==='@t3tools/contracts')return {url:'file:///native/packages/contracts/src/index.ts',shortCircuit:true};
 if(specifier.startsWith('@t3tools/shared/'))return {url:'file:///native/packages/shared/src/'+specifier.slice('@t3tools/shared/'.length)+'.ts',shortCircuit:true};
 if(!resolving&&(specifier==='effect'||specifier.startsWith('effect/'))){resolving=true;try{return {url:pathToFileURL(require.resolve(specifier)).href,shortCircuit:true};}finally{resolving=false;}}
 return next(specifier,context);
}});

const imported=createRequire('/native/apps/server/src/speech/SpeechService.ts').resolve('effect/Effect');
const direct=require.resolve('effect/Effect');
if(imported!==direct)throw Error('Multiple Effect identities');
fs.writeFileSync('/out/canonical-effect.json',JSON.stringify({imported,direct,sha256:crypto.createHash('sha256').update(fs.readFileSync(direct)).digest('hex')})+'\n',{flag:'wx'});
