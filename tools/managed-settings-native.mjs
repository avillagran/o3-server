// Load the pinned pure managed-settings graph without importing server services.
import fs from 'node:fs';
import path from 'node:path';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
export async function loadManagedSettingsNative(checkout,contracts,output,source){
 const require=createRequire(contracts+'/package.json');
 const visited=new Map();
 fs.mkdirSync(output,{recursive:true});
 function resolve(specifier,from){
  if(specifier==='@t3tools/contracts')return pathToFileURL(contracts+'/src/index.ts').href;
  if(specifier.startsWith('@t3tools/')){
   const [pkg,...parts]=specifier.slice('@t3tools/'.length).split('/');
   const directory=path.join(checkout,'packages',pkg);
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
  const destination=path.join(output,'native-'+visited.size+'.mjs');const url=pathToFileURL(destination).href;visited.set(file,url);
  const relative=path.relative(checkout,file);let text=source(relative);
  if(relative==='apps/server/src/sourceControl/sourceControlHostSecrets.ts'){
   // Exact pure helpers; the omitted tail imports secret resolvers and server drivers.
   text='import {isSecretReference,secretSettingsFields} from "@t3tools/contracts";\nimport {builtInSourceControlDefinitions as sourceControlDefinitions} from "@t3tools/source-control-definitions/registry";\n'+text.slice(text.indexOf('export const SOURCE_CONTROL_HOST_SECRET_FIELDS'),text.indexOf('type SourceControlHostsSettings'));
  }
  text=stripTypeScriptTypes(text,{mode:'strip'});
  text=text.replace(/\b(from\s*|import\s*)(["'])([^"']+)\2/g,(match,prefix,quote,specifier)=>prefix+quote+resolve(specifier,file)+quote);
  if(relative==='apps/server/src/settingsSecrets.ts')text+='\nexport {providerSecretFields,usageLimitSecretFields,easDeviceHostSecretFields,simlockDeviceHostSecretFields};\n';
  fs.writeFileSync(destination,text);return url;
 }
 const settings=await import(write(path.join(checkout,'apps/server/src/policy/managedSettings.ts')));
 const secrets=await import(write(path.join(checkout,'apps/server/src/settingsSecrets.ts')));
 const hosts=await import(write(path.join(checkout,'apps/server/src/sourceControl/sourceControlHostSecrets.ts')));
 const serverSettings=source('apps/server/src/serverSettings.ts');
 const persistedFile=path.join(output,'persisted.mjs');
 const imports='import {ServerSettings,ResponseStreamingMode,ProjectId,ProjectSettingsOverrides,keepUnknownOverrideKeys} from '+JSON.stringify(pathToFileURL(contracts+'/src/index.ts').href)+';\n'+['Schema','Effect','SchemaTransformation'].map(name=>'import * as '+name+' from '+JSON.stringify(pathToFileURL(require.resolve('effect/'+name)).href)+';').join('\n');
 fs.writeFileSync(persistedFile,imports+'\n'+stripTypeScriptTypes(serverSettings.slice(serverSettings.indexOf('const PersistedResponseStreamingMode'),serverSettings.indexOf('// Parse first and decode second')),{mode:'strip'})+'\nexport {PersistedServerSettings};');
 const persisted=await import(pathToFileURL(persistedFile).href);
 return {settings,secrets,hosts,persisted};

}
