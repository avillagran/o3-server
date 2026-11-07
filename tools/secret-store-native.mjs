// Load complete pinned store methods and complete private-settings transaction methods.
import fs from'node:fs';import path from'node:path';import{createRequire,stripTypeScriptTypes}from'node:module';import{pathToFileURL}from'node:url';import{loadManagedSettingsNative}from'./managed-settings-native.mjs';
export async function loadSecretStoreNative(checkout,contracts,output,source){
 const require=createRequire(contracts+'/package.json');fs.mkdirSync(output,{recursive:true});
 const url=name=>pathToFileURL(require.resolve('effect/'+name)).href;
 const config=source('apps/server/src/config.ts');const configStart=config.indexOf('export class ServerConfig extends Context.Service<'),configEnd=config.indexOf('\nexport const make =',configStart);if(configStart<0||configEnd<configStart)throw Error('Native config tag boundary changed');
 const configFile=path.join(output,'config-tag.mjs');fs.writeFileSync(configFile,'import * as Context from '+JSON.stringify(url('Context'))+';\n'+stripTypeScriptTypes(config.slice(configStart,configEnd),{mode:'strip'}));
 let store=stripTypeScriptTypes(source('apps/server/src/auth/ServerSecretStore.ts'),{mode:'strip'});store=store.replace(/\bfrom\s+(["'])([^"']+)\1/g,(_,q,spec)=>'from '+q+(spec==='../config.ts'?pathToFileURL(configFile).href:spec.startsWith('effect/')?url(spec.slice(7)):(()=>{throw Error('Unexpected store import '+spec)})())+q);const storeFile=path.join(output,'store.mjs');fs.writeFileSync(storeFile,store);
 const nativeStore=await import(pathToFileURL(storeFile).href),nativeConfig=await import(pathToFileURL(configFile).href);
 const settings=await loadManagedSettingsNative(checkout,contracts,path.join(output,'settings-runtime'),source);
 const original=source('apps/server/src/serverSettings.ts');
 function slice(start,end){const at=original.indexOf(start),to=original.indexOf(end,at);if(at<0||to<at)throw Error('Native settings method boundary changed '+start);return original.slice(at,to);}
 // Whole original methods; no persistence/rollback reducer is rewritten in the oracle.
 const redaction=slice('export function redactServerSettingsForClient(', '\nexport function applyProviderInstanceMutation(');
 const materialize=slice('  const materializeSettingsSecrets = ', '\n  // Background sweeps read settings');
 const transaction=slice('  type SecretChange = ', '\n  const updateAndPersistSettings = ');
 const C=await import(pathToFileURL(contracts+'/src/index.ts').href);
 const visited=new Map();
 function write(file){
  if(visited.has(file))return visited.get(file);const target=path.join(output,'registry-'+visited.size+'.mjs'),uri=pathToFileURL(target).href;visited.set(file,uri);
  function resolve(spec){
   if(spec==='@t3tools/contracts')return pathToFileURL(contracts+'/src/index.ts').href;
   if(spec.startsWith('@t3tools/')){const[pkg,...parts]=spec.slice(9).split('/');const root=path.join(checkout,'packages',pkg);const manifest=JSON.parse(fs.readFileSync(path.join(root,'package.json'),'utf8'));let item=manifest.exports[parts.length?'./'+parts.join('/'):'.'];if(typeof item==='object')item=item.import??item.default;if(typeof item!=='string')throw Error('Unsupported secret export '+spec);return write(path.resolve(root,item));}
   if(spec.startsWith('.'))return write(path.resolve(path.dirname(file),spec));if(spec.startsWith('node:'))return spec;return pathToFileURL(require.resolve(spec)).href;
  }
  let text=stripTypeScriptTypes(source(path.relative(checkout,file)),{mode:'strip'});text=text.replace(/\b(from\s*|import\s*)(["'])([^"']+)\2/g,(_,prefix,q,spec)=>prefix+q+resolve(spec)+q);fs.writeFileSync(target,text);return uri;
 }
 const registryFile=path.join(checkout,'packages/secret-provider-definitions/package.json');const registryManifest=JSON.parse(fs.readFileSync(registryFile,'utf8'));let registry=registryManifest.exports['./registry'];if(typeof registry==='object')registry=registry.import??registry.default;if(typeof registry!=='string')throw Error('Native secret registry export changed');
 const definitions=await import(write(path.resolve(path.dirname(registryFile),registry)));
 const knownKinds=definitions.builtInSecretProviderDefinitions.definitions.map(definition=>definition.kind);
 const header='import {isSecretReference,ServerSettingsError} from '+JSON.stringify(pathToFileURL(contracts+'/src/index.ts').href)+';\n'+['Effect','Option','Schema'].map(n=>'import * as '+n+' from '+JSON.stringify(url(n))+';').join('\n')+'\n';
 const secretModule=Object.keys(settings.secrets).map(name=>'const '+name+'=secrets.'+name+';').join('\n');
 const methodSource=slice('const secretProviderConfigGuards = ', '\n// Rejects a kind')+'\n'+redaction+'\n'+materialize+'\n'+transaction;
 const methodJs=stripTypeScriptTypes(methodSource,{mode:'strip'}).replace('export function redactServerSettingsForClient','function redactServerSettingsForClient');
 const file=path.join(output,'transactions.mjs');fs.writeFileSync(file,header+'export function transactionMethods(secretStore,settingsPath,secrets,secretProviderDefinitions){\n'+secretModule+'\nconst textEncoder=new TextEncoder();const textDecoder=new TextDecoder();\n'+methodJs+'\nreturn {redact:redactServerSettingsForClient,echo:withEchoedSecretReferences,materialize:materializeSettingsSecrets,plan:persistSettingsSecrets,apply:applySettingsSecretChanges};\n}\n');

 const methods=await import(pathToFileURL(file).href);return{store:nativeStore,config:nativeConfig,methods,settings,C,knownKinds,definitions:definitions.builtInSecretProviderDefinitions};
}
