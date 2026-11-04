#!/usr/bin/env node
// Generate protocol fixtures by running only the pure handoff functions from a selected MIT source file.
import {readFileSync,writeFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const [sourcePath,outputPath]=process.argv.slice(2);
if (!sourcePath || !outputPath) throw Error('Usage: generate-handoff-fixtures.mjs handoffBudget.ts output.json');
const source=readFileSync(sourcePath,'utf8');
const pure=source.slice(source.indexOf('export function historicalMessage('));
if(!pure.startsWith('export function historicalMessage(') || !pure.includes('export function selectHistory')) throw Error('Unrecognized source');
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
writeFileSync(outputPath,JSON.stringify({source:'8b44f42b14c8e273156474a924ccf29680dd7857:packages/provider-core/src/server/handoffBudget.ts',items,messages,coverage,cases},null,2)+'\n');
