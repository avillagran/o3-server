#!/usr/bin/env node
// Execute the pinned pure managed-limit functions; no networking or Tailscale calls.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-managed-limits-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='c55db3e9c72fdf584f4b54192fb39beb88667b59';
const paths=['apps/server/src/policy/managedLimits.ts','apps/server/src/auth/utils.ts','packages/shared/src/hostClassification.ts'];
const sources=paths.map(path=>execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'}));
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect 4.1.2 required');
const effectUrl=pathToFileURL(require.resolve('effect/Effect')).href;
const schemaUrl=pathToFileURL(require.resolve('effect/Schema')).href;
const Effect=await import(effectUrl);
function between(source,start,end){const a=source.indexOf(start),b=source.indexOf(end,a);if(a<0||b<a)throw Error('Pinned source boundary missing');return source.slice(a,b);}
let code=sources[0].replace(/^import[\s\S]*?;\s*$/gm,'');
code=code.replace(between(code,'// Node throws ENOTDIR','export const refuseRemoteClients'),'');
code=stripTypeScriptTypes(code);
code='import * as Effect from '+JSON.stringify(effectUrl)+';\nimport * as Schema from '+JSON.stringify(schemaUrl)+';\nimport * as NodeNet from "node:net";\n'+
stripTypeScriptTypes(between(sources[2],'export const normalizeHostname','const parseIpv4Address'))+'\n'+
stripTypeScriptTypes(between(sources[1],'export function loopbackBindAddress','export const isRemoteReachableHost'))+'\n'+code;
const native=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const cases=[];
const documents=[undefined,{schemaVersion:1,organization:'Example Corp'},...[['desktop'],['local-browser'],['remote'],['desktop','local-browser'],['desktop','remote'],['local-browser','remote'],['desktop','local-browser','remote']].flatMap(clients=>[undefined,false,true].map(connect=>({schemaVersion:1,organization:'Example Corp',limits:{clients,...(connect===undefined?{}:{connect})}}))),{schemaVersion:1,organization:'Example Corp',limits:{connect:false}}];
const hosts=[undefined,'','localhost','LOCALHOST.','localhost...','[localhost]','localhost.evil','127.0.0.1','127.2.3.4','127.0.0.01','127.1','127.256.0.1',' 127.0.0.1','127.0.0.1.','[127.0.0.1]','::1','[::1]','[::1].','0:0:0:0:0:0:0:1','::ffff:127.0.0.1','0.0.0.0','::','10.0.0.1','example.test'];
for(const document of documents)for(const authPolicy of ['desktop-managed-local','loopback-browser','remote-reachable','unsafe-no-auth'])for(const host of hosts)for(const tailscaleServeEnabled of [false,true])for(const privateNetworkOnly of [undefined,true]){
 const policy={document};const input={authPolicy,...(host===undefined?{}:{host}),tailscaleServeEnabled,...(privateNetworkOnly===undefined?{}:{privateNetworkOnly})};
 const result=await Effect.runPromise(native.checkServerReachability(policy,input).pipe(Effect.map(host=>({host})),Effect.catch(error=>Effect.succeed({error:error.message}))));
 cases.push({document:document??null,input,allowsAuth:native.allowsAuthPolicy(policy,authPolicy),allowsConnect:native.allowsConnect(policy),result});
}
const fixture={revision,effect:'4.1.2',sources:paths.map((path,i)=>({path,sha256:crypto.createHash('sha256').update(sources[i]).digest('hex')})),cases};
const serialized=Object.entries(fixture).map(([key,value])=>'  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))).join(',\n');
fs.writeFileSync(output,'{\n'+serialized+'\n}\n');console.log(`${cases.length} native client/reachability cases`);
