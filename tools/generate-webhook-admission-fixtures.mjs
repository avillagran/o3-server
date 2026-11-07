#!/usr/bin/env node
// Execute pinned validation and verification functions with exact Effect and Node HMAC.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!checkout||!contracts||!output) throw Error('usage: generate-webhook-admission-fixtures.mjs T3_CHECKOUT CONTRACTS OUTPUT');
const revision='c55db3e9c72fdf584f4b54192fb39beb88667b59';
const paths=['apps/server/src/scheduledTasks/webhookVerification.ts','packages/contracts/src/scheduledTask.ts'];
const sources=paths.map(p=>execFileSync('git',['-C',checkout,'show',`${revision}:${p}`],{encoding:'utf8'}));
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2') throw Error('Effect 4.1.2 required');
const modules={};
for(const name of ['Effect','Layer','crypto/Mac','crypto/SecureRandom']) modules[name]=await import(pathToFileURL(require.resolve('effect/'+name)).href);
let code=stripTypeScriptTypes(sources[0]).replace(/from ["'](effect\/[^"']+)["']/g,(_,name)=>'from '+JSON.stringify(pathToFileURL(require.resolve(name)).href));
const start=sources[1].indexOf('export function isWebhookDedupeTemplate('),end=sources[1].indexOf('\nconst WebhookDedupeTemplate',start);
if(start<0||end<start) throw Error('Pinned validator not found');
code+='\n'+stripTypeScriptTypes(sources[1].slice(start,end));
const native=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const mac=modules['crypto/Mac'].layerWebCryptoWith({hmac:(hash,key,data)=>crypto.createHmac(hash,key).update(data).digest()}).pipe(modules.Layer.provide(modules['crypto/SecureRandom'].layerWebCrypto));
const cases=[];
for(const secret of ['synthetic-key','é-key','', 'emoji-😀']) for(const prefix of ['','sha256=','É:','İ:','🧪:']) for(const encoding of ['hex','base64']) {
 const body=Buffer.from('Synthetic body\n😀','utf8');
 const tag=crypto.createHmac('sha256',secret).update(body).digest(encoding);
 for(const value of [null,prefix+tag,prefix.toLowerCase()+tag,prefix+tag.toUpperCase(),' '+prefix+tag+' ','\ufeff'+prefix+tag+'\ufeff','\u0085'+prefix+tag+'\u0085',prefix+'bad',prefix+tag+'.extra']) {
   const signature={header:'X-Signature',prefix,encoding};
   const headers=value===null?{}:{'x-signature':value};
   const expected=await modules.Effect.runPromise(native.verifyWebhookSignature({signature,secret,headers,body}).pipe(modules.Effect.provide(mac)));
   cases.push({signature,secret,headers,body:Array.from(body),expected});
 }
}
const templates=['','constant','{{body}}','{{ headers.x }}','{{query.group}}','{{request}}','{{request.method}}','{{body.items[].name}}','{{body..name}}','{{body.}}','{{{body}}}','{{body}}}','{{body}} {{unknown}}','{{body}} {{','{{body}} }}','{{body.\u0085}}','{{\u0085body}}','{{\ufeffbody\ufeff}}','{{body.x y}}','{{body.x\ny}}','prefix {{body.id}} suffix'];
const validators=templates.map(template=>({template,expected:native.isWebhookDedupeTemplate(template)}));
const fixture={revision,effect:'4.1.2',sources:paths.map((path,i)=>({path,sha256:crypto.createHash('sha256').update(sources[i]).digest('hex')})),verification:cases,templates:validators};
const serialized=Object.entries(fixture).map(([key,value])=>'  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))).join(',\n');
fs.writeFileSync(output,'{\n'+serialized+'\n}\n');
console.log(`${cases.length} signature and ${validators.length} template cases`);
