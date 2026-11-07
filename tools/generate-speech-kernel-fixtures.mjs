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
const language=await import('file:///native/apps/server/src/speech/nativeLanguage.ts');
const pcmSource=source('apps/server/src/speech/SpeechService.ts');
const pcmBody=pcmSource.slice(pcmSource.indexOf('const SAMPLE_RATE ='),pcmSource.indexOf('export class SpeechOperationError'));
if(!pcmBody.includes('export function decodeSpeechPcm'))throw Error('Missing whole PCM function');
fs.writeFileSync('/tmp/pcm.mts',`import * as Schema from ${JSON.stringify(schemaUrl)};\n`+pcmBody);
const pcm=await import('file:///tmp/pcm.mts');
const cloudSource=source('apps/server/src/speech/CloudSpeech.ts');
const waveBody=cloudSource.slice(cloudSource.indexOf('function wave('),cloudSource.indexOf('function audioBody('));
if(!waveBody.includes('Math.round'))throw Error('Missing whole WAV function');
fs.writeFileSync('/tmp/wave.mts','export '+waveBody);
const {wave}=await import('file:///tmp/wave.mts');
const hex=bytes=>Buffer.from(bytes).toString('hex');
const floats=bits=>new Float32Array(Uint32Array.from(bits).buffer);
const bitsOf=values=>Array.from(new Uint32Array(values.buffer,values.byteOffset,values.length));
const pcms=[];
for(const byteLength of [0,1,3,4,8,pcm.MAX_SPEECH_BYTES,pcm.MAX_SPEECH_BYTES+4]){
 const bytes=new Uint8Array(byteLength);try{const result=pcm.decodeSpeechPcm(bytes);pcms.push({byteLength,allowed:true,samples:result.length});}catch(e){pcms.push({byteLength,error:{tag:e._tag,message:e.message,byteLength:e.byteLength}});}
}
const special=[0,0x80000000,0x3f800000,0xbf800000,0x3f800001,0xbf800001,0x7f800000,0xff800000,0x7fc00001,0x00000001,0x80000001,0x37800000,0xb7800000];
for(const bit of special){const bytes=new Uint8Array(floats([bit]).buffer);const backing=new Uint8Array(20);backing.fill(255);backing.set(bytes,8);try{const result=pcm.decodeSpeechPcm(backing.subarray(8,12));pcms.push({bits:[bit],viewOffset:8,allowed:true,decodedBits:bitsOf(result)});}catch(e){pcms.push({bits:[bit],viewOffset:8,error:{tag:e._tag,message:e.message,byteLength:e.byteLength}});}}
let seed=4314;const generated=[];for(let n=0;n<256;n++){seed=(Math.imul(seed,1664525)+1013904223)>>>0;generated.push(Math.fround((seed/0xffffffff)*2-1));}
const waves=[];for(const values of [[],[0,-0,1,-1],[1/65534,-1/65536,3/65534,-3/65536],generated]){const data=Float32Array.from(values);waves.push({bits:bitsOf(data),waveHex:hex(wave(data))});}
const languages=[];for(const advertised of [[],['en'],['de-DE','en-US'],['pt_BR','pt-PT','en'],['ZH_cn','zh-TW'],['tr','İ']])for(const wanted of ['auto','en','en-US','de','DE_de','pt','pt-BR','zh','zh-CN','zz','İ'])languages.push({advertised,wanted,result:language.nativeSpeechLanguage(advertised,wanted)??null});
const effective=[];for(const model of [{languages:[],supportsLanguageDetection:false},{languages:[],supportsLanguageDetection:true},{languages:['en'],supportsLanguageDetection:false},{languages:['de','en'],supportsLanguageDetection:false},{languages:['de','fr'],supportsLanguageDetection:true}])for(const intent of ['auto','en','de','zz'])effective.push({model,intent,result:shared.effectiveSpeechLanguage(model,intent)});
const vocabularies=[];const dictionary=[{term:'OpenAI',aliases:['open ai','GPT']}];
for(const personal of [[],dictionary,[{term:'é',aliases:['Beta']}]])for(const project of [[],[{term:'GPT',aliases:['OpenAI','Project']}],[{term:'e\u0301',aliases:['beta']}]])for(const hints of [[],['  Project\n name  ','Open ai','\ufeffNew\ufeffword','\u0085NEL'],['a'.repeat(50),'a'.repeat(51),'   ']]){const input={personal,project,hints};vocabularies.push({input,result:shared.resolveSpeechVocabulary(input)});}
fs.writeFileSync('/out/fixtures.json',JSON.stringify({revision,effect:'4.1.2',schemaUrl,manifestSha256,sources,pcms,waves,languages,effective,vocabularies,boundaries:['Whole pinned native functions; no engine or HTTP','PCM max length tested with synthetic raw samples','Unicode locale and lone-surrogate representation require separate qualification']},null,2)+'\n');
console.log(JSON.stringify({pcm:pcms.length,wave:waves.length,language:languages.length,effective:effective.length,vocabulary:vocabularies.length}));
