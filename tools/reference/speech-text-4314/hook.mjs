import {registerHooks,createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
import fs from 'node:fs';
const require=createRequire('/deps/package.json');let resolving=false;
registerHooks({resolve(specifier,context,next){
 if(specifier==='@t3tools/contracts')return {url:'file:///native/packages/contracts/src/index.ts',shortCircuit:true};
 if(specifier.startsWith('@t3tools/shared/'))return {url:'file:///native/packages/shared/src/'+specifier.slice('@t3tools/shared/'.length)+'.ts',shortCircuit:true};
 if(!resolving&&(specifier==='effect'||specifier.startsWith('effect/'))){resolving=true;try{return {url:pathToFileURL(require.resolve(specifier)).href,shortCircuit:true};}finally{resolving=false;}}
 return next(specifier,context);
}});
const imported=createRequire('/native/packages/shared/src/speech.ts').resolve('effect/Effect'),direct=require.resolve('effect/Effect');
if(imported!==direct)throw Error('Multiple Effect identities');
fs.writeFileSync('/out/canonical-effect.json',JSON.stringify({imported,direct})+'\n',{flag:'wx'});
