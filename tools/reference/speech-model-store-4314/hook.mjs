import {registerHooks,createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
const require=createRequire('/deps/package.json');
let resolving=false;
registerHooks({resolve(specifier,context,next){
 if(specifier==='../config.ts'&&context.parentURL?.endsWith('/speech/SpeechModelStore.ts'))return {url:new URL('./config.mjs',import.meta.url).href,shortCircuit:true};
 if(!resolving&&(specifier==='effect'||specifier.startsWith('effect/'))){resolving=true;try{return {url:pathToFileURL(require.resolve(specifier)).href,shortCircuit:true};}finally{resolving=false;}}
 return next(specifier,context);
}});
