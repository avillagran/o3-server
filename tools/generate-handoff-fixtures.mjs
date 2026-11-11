#!/usr/bin/env node
// Generate protocol fixtures by running only the pure handoff functions from a selected MIT source file.
import {readFileSync,writeFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const [sourcePath,outputPath]=process.argv.slice(2);
if (!sourcePath || !outputPath) throw Error('Usage: generate-handoff-fixtures.mjs handoffBudget.ts output.json');
const source=readFileSync(sourcePath,'utf8');
const historySource=source.slice(source.indexOf('export function historicalMessage('));
const pure='const HANDOFF_BYTE_CAP=64_000;\n'+source.slice(source.indexOf('export function latestNativeContextUsage('),source.indexOf('export function historicalMessage('))+historySource;
if(!historySource.startsWith('export function historicalMessage(') || !pure.includes('export function selectHistory')) throw Error('Unrecognized source');
const api=await import('data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(pure)).toString('base64'));
const items=[
 {type:'user_message',text:'First request 😀 "quote"\n\t'+('inicio '.repeat(25))},
 {type:'assistant_message',text:'Answer 日本語 '+('data '.repeat(20))},
 {type:'command_execution',input:'printf test',exitCode:0,output:'stdout\r\noutput'},
 {type:'error',failure:{message:'Failed native step'}},
 {type:'run_interrupt_result',message:'Interrupted'},
 {type:'file_change',changes:[{path:'src/日本語.rs'},{path:'README.md'}]},
 {type:'proposed_plan',markdown:'# Plan\n- First\n- Second'},
 {type:'dynamic_tool',toolName:'Read',output:'foreign result'},
 {type:'reasoning',text:'hidden native reasoning'},
 {type:'file_change',fileName:'fallback.txt'},
 {type:'user_message',text:'Latest user\n'+('continuation 😀 '.repeat(40))},
 {type:'assistant_message',text:'Latest answer\\control\u0001'}
].map((item,index)=>({id:'item-'+index,threadId:'source',runId:index===0?null:'run',providerThreadId:index===0?null:'provider-thread',status:'completed',...item}));
const messages=items.flatMap(i=>api.historicalMessage(i)??[]);
const coverage=api.handoffCoverage({threadId:'source',coveredRunOrdinals:{from:1,to:3},items});
const cases=[0,256,512,1024,2048,4096,16000].map(budget=>{const selected=api.selectHistory({messages,coverage,omittedItems:3,budget});return{budget,selected,cost:api.historyCost(selected.messages,selected.context),responseItems:api.historyResponseItems(selected.messages,selected.context),rendered:api.renderHistory(selected.messages,selected.context)}});
const usageCases=[
 {sameNativeThread:false,sameSelection:true,reuseTelemetry:true,previousUsage:{usedTokens:10000,maxTokens:200000}},
 {sameNativeThread:true,sameSelection:true,reuseTelemetry:true,previousUsage:{usedTokens:50000,maxTokens:200000,autoCompactThreshold:170000}},
 {sameNativeThread:true,sameSelection:false,reuseTelemetry:false,previousUsage:{usedTokens:50000,maxTokens:200000,autoCompactThreshold:170000},knownModelWindow:1000000},
 {sameNativeThread:true,sameSelection:false,reuseTelemetry:true,previousUsage:{usedTokens:50000,maxTokens:200000,autoCompactThreshold:170000},knownModelWindow:1000000},
 {sameNativeThread:true,sameSelection:false,reuseTelemetry:false,previousUsage:{usedTokens:50000,maxTokens:0}},
 {sameNativeThread:true,sameSelection:true,reuseTelemetry:false,previousUsage:null}
].map(input=>({input,expected:api.contextUsageForHandoff(input)}));
const budgetCases=[
 {tokenCap:16000,userText:'current 😀 "quoted"\n',attachments:[],usage:null,nativeContextEstimate:0},
 {tokenCap:64000,userText:'current',attachments:[],usage:{usedTokens:100000,maxTokens:128000},nativeContextEstimate:0},
 {tokenCap:16000,userText:'current',attachments:[{type:'image'},{type:'file'}],usage:{usedTokens:1000,maxTokens:64000,autoCompactThreshold:48000},nativeContextEstimate:0},
 {tokenCap:16000,userText:'x'.repeat(100000),attachments:[],usage:null,nativeContextEstimate:0,modelContextWindow:128000},
 {tokenCap:64000,userText:'current',attachments:[{type:'file'}],usage:null,nativeContextEstimate:20000,modelContextWindow:65535},
 {tokenCap:0,userText:'',attachments:[],usage:null,nativeContextEstimate:0},
 {tokenCap:100000,userText:'current',attachments:[],usage:{usedTokens:0,maxTokens:1000000},nativeContextEstimate:999999},
 {tokenCap:64000,userText:'current',attachments:[],usage:{usedTokens:10000,maxTokens:200000,autoCompactThreshold:180000},nativeContextEstimate:999999,modelContextWindow:128000}
].map(input=>({input,expected:api.handoffBudget({...input,providerThread:{contextUsage:input.usage}})}));
writeFileSync(outputPath,JSON.stringify({source:'8b44f42b14c8e273156474a924ccf29680dd7857:packages/provider-core/src/server/handoffBudget.ts',items,messages,coverage,cases,usageCases,budgetCases},null,2)+'\n');
