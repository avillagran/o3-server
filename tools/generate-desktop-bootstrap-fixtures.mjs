#!/usr/bin/env node
// Execute the native bootstrap schema and rotating-token functions with private synthetic keys.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-desktop-bootstrap-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87';
const paths=['packages/contracts/src/desktopBootstrap.ts','packages/contracts/src/baseSchemas.ts','packages/shared/src/desktopBootstrapToken.ts'];
const sources=paths.map(path=>execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'}));
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect 4.1.2 required');
const modules={};let code='';
for(const name of ['Effect','Layer','Option','Schema','SchemaTransformation','crypto/Mac','crypto/SecureRandom']){
 const url=pathToFileURL(require.resolve('effect/'+name)).href;
 modules[name]=await import(url);
 if(!name.includes('/'))code+='import * as '+name+' from '+JSON.stringify(url)+';\n';
}
const start=sources[1].indexOf('// Hermes copies'),end=sources[1].indexOf('export const DpopFailureReason');
if(start<0||end<start)throw Error('Pinned base schema boundary missing');
code+=stripTypeScriptTypes(sources[1].slice(start,end))+'\n';
code+=stripTypeScriptTypes(sources[0].replace(/^import[\s\S]*?;\s*$/gm,''));
const schema=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const tokenCode=stripTypeScriptTypes(sources[2]).replace(/from ["'](effect\/[^"']+)["']/g,(_,name)=>'from '+JSON.stringify(pathToFileURL(require.resolve(name)).href));
const native=await import('data:text/javascript;base64,'+Buffer.from(tokenCode).toString('base64'));
const layer=modules['crypto/Mac'].layerWebCryptoWith({hmac:(hash,key,data)=>crypto.createHmac(hash,key).update(data).digest()}).pipe(modules.Layer.provide(modules['crypto/SecureRandom'].layerWebCrypto));
const run=async effect=>{try{return {value:await modules.Effect.runPromise(effect.pipe(modules.Effect.provide(layer)))};}catch(error){if(error?._tag==='CryptoError'&&error.reason?._tag==='InvalidKey')return {error:'InvalidKey'};throw error;}};
const window=native.DESKTOP_BOOTSTRAP_TOKEN_WINDOW_MS,tokens=[],verification=[];
for(const secret of ['','private-synthetic-key','clé-😀',' \ufeff key\n'])for(const nowMs of [-window-1,-window,-1,0,1,window-1,window,window+1,2*window-1,2*window,1794000000000]){
 const expected=await run(native.currentDesktopBootstrapToken(secret,nowMs));tokens.push({secret,nowMs,expected});
 if(expected.error){for(const token of ['', 'f'.repeat(64)])verification.push({secret,nowMs,token,expected:await run(native.isValidDesktopBootstrapToken(secret,token,nowMs))});continue;}
 for(const offset of [-3,-2,-1,0,1,2,3]){
  const token=(await run(native.currentDesktopBootstrapToken(secret,nowMs+offset*window))).value;
  verification.push({secret,nowMs,token,expected:await run(native.isValidDesktopBootstrapToken(secret,token,nowMs))});
 }
 for(const token of ['',expected.value.toUpperCase(),expected.value+'0',' '+expected.value,expected.value.slice(1),expected.value.slice(0,-1)+'g'])verification.push({secret,nowMs,token,expected:await run(native.isValidDesktopBootstrapToken(secret,token,nowMs))});
}
const base={mode:'desktop',noBrowser:true,port:3773,host:'127.0.0.1',desktopBootstrapToken:'synthetic-token',tailscaleServeEnabled:false,tailscaleServePort:443};
const decode=modules.Schema.decodeUnknownOption(schema.DesktopBackendBootstrap),envelopes=[];
const add=input=>{const result=decode(input);envelopes.push({input,expected:modules.Option.isSome(result)?result.value:null});};
add(base);add({...base,futureProperty:'ignored'});
const fields=Object.keys(schema.DesktopBackendBootstrap.fields);
for(const name of fields){const missing={...base};delete missing[name];add(missing);for(const value of [null,false,true,0,1,1.5,65535,65536,'',' \ufeff value \ufeff',{},[]])add({...base,[name]:value});}
for(const value of [null,[],false,'text',1])add(value);
// Import the complete native reader and its real local dependencies.
const extraPaths=['packages/shared/src/HostProcess.ts','packages/shared/src/schemaJson.ts','apps/server/src/bootstrap.ts'];
const extraSources=extraPaths.map(path=>execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'}));
const moduleUrl=source=>'data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(source).replace(/from ["'](effect\/[^"']+)["']/g,(_,name)=>'from '+JSON.stringify(pathToFileURL(require.resolve(name)).href))).toString('base64');
const hostUrl=moduleUrl(extraSources[0]),jsonUrl=moduleUrl(extraSources[1]);
const reader=await import(moduleUrl(extraSources[2].replace('"@t3tools/shared/HostProcess"',JSON.stringify(hostUrl)).replace('"@t3tools/shared/schemaJson"',JSON.stringify(jsonUrl))));
const directory=fs.mkdtempSync('/tmp/o3-bootstrap-native-'),fdCases=[];
const inspect=async(fd,timeoutMs=100)=>{try{const result=await modules.Effect.runPromise(reader.readBootstrapEnvelope(schema.DesktopBackendBootstrap,fd,{timeoutMs}));return modules.Option.isSome(result)?{value:result.value}:{none:true};}catch(error){return {error:error._tag};}};
const text=JSON.stringify(base);
for(const [label,bytes] of [
 ['line',Buffer.from(text+'\n')],['eof',Buffer.from(text)],['crlf',Buffer.from(text+'\r\nignored')],['cr',Buffer.from(text+'\rignored')],['first-line',Buffer.from(text+'\n{bad')],['empty',Buffer.alloc(0)],['blank',Buffer.from('\n'+text)],['bad-json',Buffer.from('{bad\n')],['bad-schema',Buffer.from('{}\n')],['raw-invalid-utf8',Buffer.alloc(0)]
]){
 // The raw-byte case exercises Node's replacement decoder inside a JSON string.
 let input=bytes;if(label==='raw-invalid-utf8'){const marker=Buffer.from(text.replace('synthetic-token','MARKER'));const offset=marker.indexOf('MARKER');input=Buffer.concat([marker.subarray(0,offset),Buffer.from([255]),marker.subarray(offset+6)]);}
 const file=directory+'/'+label;fs.writeFileSync(file,input);const fd=fs.openSync(file,'r');
 const expected=await inspect(fd);fs.closeSync(fd);fdCases.push({label,bytes:[...input],expected});
}
fdCases.push({label:'unavailable',fd:-1,expected:await inspect(-1)});
const closedPath=directory+'/closed';fs.writeFileSync(closedPath,'');const closedFd=fs.openSync(closedPath,'r');fs.closeSync(closedFd);fdCases.push({label:'closed',expected:await inspect(closedFd)});
const fifo=directory+'/timeout';execFileSync('mkfifo',[fifo]);const fifoFd=fs.openSync(fifo,fs.constants.O_RDWR|fs.constants.O_NONBLOCK);fdCases.push({label:'timeout',timeoutMs:25,expected:await inspect(fifoFd,25)});fs.closeSync(fifoFd);
paths.push(...extraPaths);sources.push(...extraSources);
const fixture={revision,effect:'4.1.2',sources:paths.map((path,i)=>({path,sha256:crypto.createHash('sha256').update(sources[i]).digest('hex')})),tokens,verification,envelopes,fdCases};
const serialized=Object.entries(fixture).map(([key,value])=>'  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))).join(',\n');
fs.writeFileSync(output,'{\n'+serialized+'\n}\n');console.log(`${tokens.length} tokens, ${verification.length} comparisons, ${envelopes.length} schema cases`);
