import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';
const revision='83a3674ffd84ffb33de768210c806cb1ac53fd80';
const manifest=JSON.parse(fs.readFileSync('/native/SOURCE.json'));
if(manifest.revision!==revision)throw Error('Wrong source pin');
const sources={};
for(const [name,expected] of Object.entries(manifest.files)){if(crypto.createHash('sha256').update(fs.readFileSync('/native/'+name)).digest('hex')!==expected)throw Error('Changed transitive source '+name);}
const manifestSha256=crypto.createHash('sha256').update(fs.readFileSync('/native/SOURCE.json')).digest('hex');
function source(file){const data=fs.readFileSync('/native/'+file);const hash=crypto.createHash('sha256').update(data).digest('hex');if(hash!==manifest.files[file])throw Error('Source changed '+file);sources[file]=hash;return data.toString();}
const require=createRequire('/native/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const schemaUrl=pathToFileURL(require.resolve('effect/Schema')).href;
const Schema=await import(schemaUrl);
source('packages/contracts/src/speech.ts');source('packages/shared/src/speech.ts');source('apps/server/src/speech/nativeLanguage.ts');
const C=await import('file:///native/packages/contracts/src/index.ts');
if(!Schema.is(C.ProviderInstanceId)('codex'))throw Error('Canonical Effect identity required');
const shared=await import('file:///native/packages/shared/src/speech.ts');
const defaults=C.DEFAULT_SPEECH_TRANSCRIPTION_OPTIONS;
const json=new TextEncoder().encode(JSON.stringify(defaults));
const concatenate=(...items)=>Uint8Array.from(items.flatMap(item=>Array.from(item)));
const bom=Uint8Array.from([239,187,191]);
function body(metadata,pcm=Uint8Array.from([0,0,0,0]),declared=metadata.length){const header=new Uint8Array(4);new DataView(header.buffer).setUint32(0,declared,true);return concatenate(header,metadata,pcm);}
const rows=[
['ordinary',body(json)],['utf8-bom',body(concatenate(bom,json))],['double-bom',body(concatenate(bom,bom,json))],['space-before-bom',body(concatenate([32],bom,json))],['bom-after-json',body(concatenate(json,bom))],
['bom-in-language',body(new TextEncoder().encode(JSON.stringify({...defaults,speechLanguage:'\ufeffen'})))],
['invalid-continuation',body(Uint8Array.from([195,40]))],['surrogate-utf8',body(Uint8Array.from([237,160,128]))],['overlong-utf8',body(Uint8Array.from([192,175]))],['truncated-utf8',body(Uint8Array.from([226,130]))],['invalid-final-byte',body(concatenate(json,[255]))],['utf16-bom',body(Uint8Array.from([255,254,123,0]))],
['missing-header',Uint8Array.from([1,2,3])],['missing-pcm',body(json,new Uint8Array())],['one-pcm-byte',body(json,Uint8Array.from([0]))],['header-u32-max',body(json,Uint8Array.from([0]),0xffffffff)],
['max-options',body(concatenate(json,new Uint8Array(128000-json.length).fill(32)))],['over-max-options',body(concatenate(json,new Uint8Array(128001-json.length).fill(32)))],
];
const cases=[];
for(const [name,input]of rows){const backing=new Uint8Array(input.length+11);backing.fill(255);backing.set(input,7);try{const decoded=shared.decodeSpeechPcmRequest(backing.subarray(7,7+input.length));cases.push({name,bodyHex:Buffer.from(input).toString('hex'),offset:7,allowed:true,options:decoded.options,pcmHex:Buffer.from(decoded.pcm).toString('hex')});}catch(error){cases.push({name,bodyHex:Buffer.from(input).toString('hex'),offset:7,allowed:false,error:{name:error.name,message:error.message}});}}
fs.writeFileSync('/out/request-boundaries.json',JSON.stringify({revision,sources,manifestSha256,cases},null,2)+'\n');console.log(JSON.stringify(cases.map(({name,allowed})=>({name,allowed}))));
