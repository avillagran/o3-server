#!/usr/bin/env node
// Execute pinned endpoint-origin and runtime-config decoders without connecting to a relay.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-relay-origin-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='c55db3e9c72fdf584f4b54192fb39beb88667b59';
const paths=['apps/server/src/scheduledTasks/ScheduledTaskService.ts','packages/contracts/src/relay.ts','packages/contracts/src/baseSchemas.ts','apps/server/src/server.ts','apps/server/src/cloud/config.ts'];
const sources=paths.map(path=>execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'}));
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect 4.1.2 required');
let code='';const modules={};
for(const name of ['Effect','Option','Schema','SchemaTransformation']){
 const url=pathToFileURL(require.resolve('effect/'+name)).href;
 modules[name]=await import(url);code+='import * as '+name+' from '+JSON.stringify(url)+';\n';
}
function between(source,start,end){const a=source.indexOf(start),b=source.indexOf(end,a);if(a<0||b<a)throw Error('Pinned source boundary missing');return source.slice(a,b);}
code+=stripTypeScriptTypes(between(sources[2],'// Hermes copies','export const NonNegativeInt'))+'\n';
code+=stripTypeScriptTypes(between(sources[1],'export const RelayManagedEndpointProviderKind','export type RelayManagedEndpointProviderKind'))+'\n';
code+=stripTypeScriptTypes(between(sources[1],'export const RelayManagedEndpointRuntimeConfig','export type RelayManagedEndpointRuntimeConfig'))+'\n';
code+=stripTypeScriptTypes(between(sources[0],'const ENDPOINT_KEY','export class ScheduledTaskWebhookOrigin'));
const native=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const origins=[];
for(const relayUrl of ['','/','///','https://relay.example','https://relay.example///',' https://relay.example/','https://relay.example/ ','\n'])for(const tunnelName of [undefined,'','0123456789abcdef','managed-0123456789abcdef','managed-0123456789ABCDEF','managed-0123456789abcde','managed-0123456789abcdef0','managed-0123456789abcdef-',' managed-0123456789abcdef','managed-0123456789abcdef\n']){
 origins.push({relayUrl,tunnelName:tunnelName??null,expected:native.relayHookBaseUrl({relayUrl,tunnelName})});
}
const configs=[];const decode=modules.Schema.decodeUnknownOption(modules.Schema.fromJsonString(native.RelayManagedEndpointRuntimeConfig));
const inspect=input=>{const value=decode(JSON.stringify(input));configs.push({input,expected:modules.Option.isSome(value)?{tunnelName:value.value.tunnelName??null}:null});};
const values=[undefined,null,'',' ','\ufeff token \ufeff','\u0085','token','managed-0123456789abcdef',1,false,{},[]];
for(const providerKind of [undefined,null,'manual','cloudflare_tunnel','t3_relay','unknown',1])for(const connectorToken of values)for(const tunnelName of values){
 inspect({...providerKind===undefined?{}:{providerKind},...connectorToken===undefined?{}:{connectorToken},...tunnelName===undefined?{}:{tunnelName}});
}
for(const tunnelId of values)inspect({providerKind:'t3_relay',connectorToken:'fixture',tunnelName:'managed-0123456789abcdef',...tunnelId===undefined?{}:{tunnelId}});
for(const input of [null,1,'text',false,[],{providerKind:'manual',connectorToken:'fixture',futureField:{preserved:false}}])inspect(input);
const fixture={revision,effect:'4.1.2',sources:paths.map((path,i)=>({path,sha256:crypto.createHash('sha256').update(sources[i]).digest('hex')})),origins,configs};
const serialized=Object.entries(fixture).map(([key,value])=>'  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))).join(',\n');
fs.writeFileSync(output,'{\n'+serialized+'\n}\n');console.log(`${origins.length} origin and ${configs.length} native config cases`);
