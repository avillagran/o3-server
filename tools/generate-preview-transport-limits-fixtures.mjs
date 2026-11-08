#!/usr/bin/env node
// Execute native relay value-size admission and extract the pinned Playwright receive bound.
import fs from 'node:fs';import crypto from 'node:crypto';import {execFileSync} from 'node:child_process';import {stripTypeScriptTypes} from 'node:module';
const[checkout,transportPath,output]=process.argv.slice(2);if(!output)throw Error('usage: generate-preview-transport-limits-fixtures.mjs CHECKOUT PLAYWRIGHT_TRANSPORT OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',file='apps/desktop/src/preview/CdpRelay.ts';
const source=execFileSync('git',['-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'}),transport=fs.readFileSync(transportPath,'utf8');
const hash=value=>crypto.createHash('sha256').update(value).digest('hex');
if(hash(transport)!=='e6b580f46cf8b45db46c1a95a438a6393b523666eba6aa0c6dd770c5818b411f')throw Error('Pinned Playwright transport source mismatch');
const start=source.indexOf('const MAX_CDP_PAYLOAD_SIZE ='),end=source.indexOf('const BROWSER_CONTEXT_ID =',start);if(start<0||end<0)throw Error('Missing native size boundary');
const native=new Function(stripTypeScriptTypes(source.slice(start,end))+';return {fitsPayloadBudget,MAX_CDP_PAYLOAD_SIZE};')();
const expression=transport.match(/maxPayload:\s*([^,]+),/)?.[1];if(!expression||!/^\d+(\s*\*\s*\d+)*$/.test(expression))throw Error('Missing pinned receive bound');
const receiveBytes=new Function('return '+expression)();
const inputs=[{text:'x',repeat:0},{text:'x',repeat:4*1024*1024+1},{text:'x',repeat:5*1024*1024},{text:'界',repeat:2*1024*1024},{text:'🙂',repeat:1024*1024},{text:'"',repeat:1024*1024}];
const cases=inputs.map(input=>{const message={id:8,sessionId:'opaque/session 😀',result:{result:{value:input.text.repeat(input.repeat)}}};return{input,expected:{fits:native.fitsPayloadBudget(message),serializedUtf8Bytes:Buffer.byteLength(JSON.stringify(message)),serializedUtf16Units:JSON.stringify(message).length}};});
const sharedString='x'.repeat(1024*1024);
const budgetCases=[63,64,65].map(copies=>({input:{sharedStringLength:sharedString.length,copies},expected:{fits:native.fitsPayloadBudget({id:8,result:Array(copies).fill(sharedString)})}}));
fs.writeFileSync(output,JSON.stringify({revision,relaySource:{file,sha256:hash(source)},playwright:{version:'1.60.0',url:'https://raw.githubusercontent.com/microsoft/playwright/v1.60.0/packages/playwright-core/src/server/transport.ts',sha256:hash(transport),receiveBytes},nativeSerializedValueBudget:native.MAX_CDP_PAYLOAD_SIZE,cases,budgetCases},null,2)+'\n');console.log(`${cases.length} native serialized-value budget cases; Playwright receive ${receiveBytes} bytes`);
