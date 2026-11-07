// Load complete pinned credential/API/discovery modules; inject only their services.
import fs from 'node:fs';
import path from 'node:path';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
export async function loadSourceCredentialsNative(checkout,contracts,output,source){
 const require=createRequire(contracts+'/package.json');const visited=new Map();fs.mkdirSync(output,{recursive:true});
 function resolve(specifier,from){
  if(specifier==='@t3tools/contracts')return pathToFileURL(contracts+'/src/index.ts').href;
  if(specifier.startsWith('@t3tools/')){
   const[pkg,...parts]=specifier.slice(9).split('/'),root=path.join(checkout,'packages',pkg),manifest=JSON.parse(fs.readFileSync(path.join(root,'package.json'),'utf8'));
   let item=manifest.exports[parts.length?'./'+parts.join('/'):'.'];if(item&&typeof item==='object')item=item.import??item.default;if(typeof item!=='string')throw Error('Unsupported credential export '+specifier);return write(path.resolve(root,item));
  }
  if(specifier.startsWith('.'))return write(path.resolve(path.dirname(from),specifier));
  if(specifier.startsWith('node:'))return specifier;
  return pathToFileURL(require.resolve(specifier)).href;
 }
 function write(file){
  if(visited.has(file))return visited.get(file);const target=path.join(output,'native-'+visited.size+'.mjs'),uri=pathToFileURL(target).href;visited.set(file,uri);
  let text=stripTypeScriptTypes(source(path.relative(checkout,file)),{mode:'strip'});
  text=text.replace(/\b(from\s*|import\s*)(["'])([^"']+)\2/g,(_,prefix,q,specifier)=>prefix+q+resolve(specifier,file)+q);
  if(file.endsWith('/BitbucketApi.ts'))text+='\nexport {resolveCredential,credentialFrom,authFromCredential,BitbucketUserSchema};\n';
  if(file.endsWith('/GitCafeCredentials.ts'))text+='\nexport {credentialPassword};\n';
  fs.writeFileSync(target,text);return uri;
 }
 const importFile=relative=>import(write(path.join(checkout,relative)));
 return {
  cafe:await importFile('packages/source-control-gitcafe/src/server/GitCafeCredentials.ts'),
  cafeDiscovery:await importFile('packages/source-control-gitcafe/src/server/GitCafeSourceControlProvider.ts'),
  bitbucket:await importFile('packages/source-control-bitbucket/src/server/BitbucketApi.ts'),
  host:await importFile('packages/source-control-core/src/server/SourceControlHost.ts'),
  hostProcess:await importFile('packages/shared/src/HostProcess.ts'),
  limits:await importFile('packages/source-control-core/src/server/SourceControlRateLimit.ts'),
 };
}
