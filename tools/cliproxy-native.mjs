// Execute the complete pinned hub API with real schemas and controlled HTTP/Digest services.
import fs from 'node:fs';
import path from 'node:path';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
export async function loadCliproxyNative(checkout,contracts,output,source){
 const require=createRequire(contracts+'/package.json');const visited=new Map();fs.mkdirSync(output,{recursive:true});
 function resolve(specifier,from){
  if(specifier==='@t3tools/contracts')return pathToFileURL(contracts+'/src/index.ts').href;
  if(specifier.startsWith('@t3tools/')){
   const[pkg,...parts]=specifier.slice(9).split('/');const directory=path.join(checkout,'packages',pkg);
   const manifest=JSON.parse(fs.readFileSync(path.join(directory,'package.json'),'utf8'));
   let exported=manifest.exports[parts.length?'./'+parts.join('/'):'.'];
   if(exported&&typeof exported==='object')exported=exported.import??exported.default;
   if(typeof exported!=='string')throw Error('Unsupported native export '+specifier);
   return write(path.resolve(directory,exported));
  }
  if(specifier.startsWith('.'))return write(path.resolve(path.dirname(from),specifier));
  if(specifier.startsWith('node:'))return specifier;
  return pathToFileURL(require.resolve(specifier)).href;
 }
 function write(file){
  if(visited.has(file))return visited.get(file);
  const destination=path.join(output,'native-'+visited.size+'.mjs'),url=pathToFileURL(destination).href;visited.set(file,url);
  const relative=path.relative(checkout,file);let text=source(relative);
  if(relative==='apps/server/src/provider/CodexProvider.ts'){
   // This leaf depends only on this complete pure function, not provider transports.
   const start=text.indexOf('export function codexPlanLabel('),end=text.indexOf('\nfunction codexAccountEmail(',start);
   if(start<0||end<start)throw Error('Native plan-label boundary changed');text=text.slice(start,end);
  }
  if(relative==='apps/server/src/usage/cliproxyApi.ts'){
   const start=text.indexOf('const decodeConsumeResponse = Schema.decodeUnknownEffect(')+'const decodeConsumeResponse = Schema.decodeUnknownEffect('.length;
   const end=text.indexOf('\n);',start);if(start<0||end<start)throw Error('Native consume schema boundary changed');
   const consume=text.slice(start,end).trim().replace(/,$/,'');text=text.replace('return { readAccounts, consume };','return { readAccounts, consume, credits };');text+='\nexport const ConsumeResponse = '+consume+';\nexport {AuthFiles,ApiResponse,CodexUsage,ClaudeUsage,CreditList,creditRedeemRequestId,describeManagementFailure};\n';
  }
  text=stripTypeScriptTypes(text,{mode:'strip'});
  text=text.replace(/\b(from\s*|import\s*)(["'])([^"']+)\2/g,(_,prefix,quote,specifier)=>prefix+quote+resolve(specifier,file)+quote);
  fs.writeFileSync(destination,text);return url;
 }
 return import(write(path.join(checkout,'apps/server/src/usage/cliproxyApi.ts')));
}
