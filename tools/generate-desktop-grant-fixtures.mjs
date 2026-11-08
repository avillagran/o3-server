#!/usr/bin/env node
// Execute the complete native grant store with private configuration, repository and clock services.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!output)throw Error('usage: generate-desktop-grant-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',sources={};
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const source=path=>{const text=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});sources[path]=crypto.createHash('sha256').update(text).digest('hex');return text;};
const url=code=>'data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(code).replace(/from ["'](effect\/[^"']+)["']/g,(_,name)=>'from '+JSON.stringify(pathToFileURL(require.resolve(name)).href))).toString('base64');
const effect=async name=>import(pathToFileURL(require.resolve('effect/'+name)).href);
const Effect=await effect('Effect'),Layer=await effect('Layer'),Option=await effect('Option'),TestClock=await effect('testing/TestClock'),Mac=await effect('crypto/Mac'),SecureRandom=await effect('crypto/SecureRandom');
const configUrl=url('import * as Context from "effect/Context"; export class ServerConfig extends Context.Service()("probe/ServerConfig") {}');
const repositoryUrl=url('import * as Context from "effect/Context"; import * as Layer from "effect/Layer"; export class AuthPairingLinkRepository extends Context.Service()("probe/AuthPairingLinks") {} export const layer=Layer.succeed(AuthPairingLinkRepository,{});');
const config=await import(configUrl),repository=await import(repositoryUrl);
const authSource=source('packages/contracts/src/auth.ts');
const constants=[...authSource.matchAll(/export const Auth\w+Scope = "[^"\n]+" as const;/g)].map(match=>match[0]).join('\n');
const scopesSource=authSource.match(/export const AuthStandardClientScopes = \[[\s\S]*?\] as const;/)?.[0];
if(!scopesSource)throw Error('Native scope declaration missing');
const contractsUrl=url(constants+'\n'+scopesSource);
const tokenUrl=url(source('packages/shared/src/desktopBootstrapToken.ts'));
let storeSource=source('apps/server/src/auth/PairingGrantStore.ts');
for(const [from,to] of [['@t3tools/contracts',contractsUrl],['@t3tools/shared/desktopBootstrapToken',tokenUrl],['../config.ts',configUrl],['../persistence/AuthPairingLinks.ts',repositoryUrl]])storeSource=storeSource.replace(JSON.stringify(from),JSON.stringify(to));
// Only expose the existing constructor; no native reducer or grant logic is rewritten.
const store=await import(url(storeSource+'\nexport {make};\n'));
const tokens=await import(tokenUrl),nativeContracts=await import(contractsUrl);
const macLayer=Mac.layerWebCryptoWith({hmac:(hash,key,data)=>crypto.createHmac(hash,key).update(data).digest()}).pipe(Layer.provide(SecureRandom.layerWebCrypto));
const cases=[];
for(const configuration of [{},{desktopBootstrapToken:''},{desktopBootstrapToken:'seed'},{desktopBootstrapToken:'seed',desktopBootstrapSecret:'rotating'},{desktopBootstrapToken:'seed',desktopBootstrapSecret:''}]){
 for(const elapsed of [0,1,43200000,86399999,86400000,86400001]){
  for(const credentialKind of ['seed','wrong','current','previous','too-old']){
   for(const scopeInput of [undefined,...[[],['orchestration:read'],['unknown'],['unknown','orchestration:read']].map(requestedScopes=>({requestedScopes})),...[[],['orchestration:read'],['unknown'],['unknown','orchestration:read']].map(requiredScopes=>({requiredScopes}))]){
    const consumed=[];const input={configuration,elapsed,credentialKind,...scopeInput};
    const program=Effect.gen(function*(){
     yield* TestClock.setTime(0);
     const service=yield* store.make;
     yield* TestClock.setTime(elapsed);
     let credential=credentialKind;
     if(['current','previous','too-old'].includes(credentialKind))credential=yield* tokens.currentDesktopBootstrapToken('rotating',elapsed+({'current':0,'previous':-1,'too-old':-2}[credentialKind])*43200000);
     const results=[];
     for(let repeat=0;repeat<2;repeat++){
      const result=yield* Effect.result(service.consume(credential,scopeInput));
      if(result._tag==='Failure')results.push({error:result.failure._tag});
      else results.push({method:result.success.method,subject:result.success.subject,scopes:result.success.scopes,expiresAt:result.success.expiresAt.epochMilliseconds});
     }
     return {credential,results};
    }).pipe(Effect.provideService(config.ServerConfig,configuration),Effect.provideService(repository.AuthPairingLinkRepository,{consumeAvailable:request=>Effect.sync(()=>{consumed.push(request.credential);return Option.none();}),getByCredential:()=>Effect.succeed(Option.none())}),Effect.provide(macLayer),Effect.provide(SecureRandom.layerWebCrypto),Effect.provide(TestClock.layer()),Effect.scoped);
    try{const expected=await Effect.runPromise(program);cases.push({input,expected,repositoryCalls:consumed});}catch(error){if(error?._tag==='CryptoError'&&error.reason?._tag==='InvalidKey')cases.push({input,expected:{error:'InvalidKey'},repositoryCalls:consumed});else throw error;}
   }
  }
 }
}
const metadata=JSON.stringify({revision,effect:'4.1.2',sources,standardScopes:nativeContracts.AuthStandardClientScopes},null,2);
fs.writeFileSync(output,metadata.slice(0,-2)+',\n  \"cases\": [\n'+cases.map(value=>'    '+JSON.stringify(value)).join(',\n')+'\n  ]\n}\n');
console.log(`${cases.length} native grant scenarios`);
