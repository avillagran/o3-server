#!/usr/bin/env node
// Execute the exact pinned navigation-report functions with inert page/CDP/manager services.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {stripTypeScriptTypes} from 'node:module';
const [checkout,output]=process.argv.slice(2);
if(!output)throw Error('usage: generate-preview-navigation-fixtures.mjs CHECKOUT OUTPUT');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',sources={};
const source=path=>{const text=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});sources[path]=crypto.createHash('sha256').update(text).digest('hex');return text;};
const browser=source('apps/server/src/preview/ServerBrowser.ts'),page=source('apps/server/src/preview/ServerBrowserPage.ts');
const extract=(text,start,end)=>{const from=text.indexOf(start),to=text.indexOf(end,from);if(from<0||to<0)throw Error('pinned source boundary missing');return text.slice(from,to);};
const sanitizer=extract(page,'export const withoutDataPayload =','\n/**').replace('export ','');
const reporting=stripTypeScriptTypes(extract(browser,'  const report = (','  const reportLiveTabs ='));
const sanitizerJs=stripTypeScriptTypes(sanitizer);
const run=new Function('input',`${sanitizerJs}\nreturn (async()=>{const reports=[];const Effect={ignore:null};const runFork=()=>{};const ServerBrowserPage={withoutDataPayload};const manager={reportStatus:input=>{reports.push(input);return{pipe:()=>undefined}}};let currentUrl=input.beforeUrl;const tab={threadId:'thread',tabId:'tab',navigationGeneration:input.generation,page:{url:()=>currentUrl,title:async()=>{tab.navigationGeneration=input.afterGeneration;tab.loading=input.loading;currentUrl=input.afterUrl;if(input.titleFails)throw Error('synthetic title failure');return input.title;}},cdp:{send:async method=>{if(method!=='Page.getNavigationHistory')throw Error('unexpected method');if(input.historyFails)throw Error('synthetic history failure');return input.history;}}};${reporting}\nawait reportLoaded(tab);await Promise.resolve();await Promise.resolve();await Promise.resolve();return reports;})();`);
const inputs=[];
for(const beforeUrl of ['about:blank','chrome-error://chromewebdata/','https://example.com/a','data:text/plain;private=secret,private payload'])for(const loading of [false,true])for(const changedGeneration of [false,true])for(const changedUrl of [false,true])for(const titleFails of [false,true])for(const historyFails of [false,true])for(const currentIndex of [0,1,3])inputs.push({beforeUrl,afterUrl:changedUrl?'https://example.com/changed':beforeUrl,generation:3,afterGeneration:changedGeneration?4:3,loading,title:'Title 😀',titleFails,historyFails,history:{currentIndex,entries:[{},{},{}]}});
for(const title of ['', 'x'.repeat(512),'x'.repeat(513),'😀'.repeat(256),'😀'.repeat(257)])for(const beforeUrl of ['https://example.com/'+ 'x'.repeat(2100),'https://example.com/','data:text/html,<private>','"\\u0064\\u0061ta:text/plain;secret=x,private',' \tDa\nta:text/plain,private'])inputs.push({beforeUrl,afterUrl:beforeUrl,generation:0,afterGeneration:0,loading:false,title,titleFails:false,historyFails:false,history:null});
const cases=[];for(const input of inputs)cases.push({input,expected:await run(input)});
const sanitizers=[];for(const input of ['https://example.com/','data:;base64,PRIVATE','DATA:text/plain;charset=PRIVATE,PRIVATE','data:text/plain,PRIVATE','data:invalid,PRIVATE',"'\\x64ata:application/json;private=x,PRIVATE",'\\😀data:text/plain,PRIVATE','\\Tdata:text/plain,PRIVATE','\\ndata:text/plain,PRIVATE','\u0000data:text/plain,PRIVATE','data:'+ 'a'.repeat(65)+'/x,PRIVATE']){const fn=new Function(`${sanitizerJs}\nreturn withoutDataPayload;`)();sanitizers.push({input,expected:fn(input)});}
const relaySource=source('apps/desktop/src/preview/CdpRelay.ts');
const relayModule=await import('data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(relaySource)).toString('base64'));
const relayCases=[];
for(const autoAttach of [false,true]){
 const writes=[],calls=[];let closes=0;
 const relay=relayModule.createCdpRelayConnection({targetId:async()=> 'page',title:()=> 'Initial',url:()=> 'https://example.com/',userAgent:()=> 'synthetic',setDownloadDirectory:()=>{},send:async(method,params,session,owner)=>{calls.push({method,params,...(session===undefined?{}:{session}),...(owner===undefined?{}:{owner})});return method==='Page.getFrameTree'?{frameTree:{frame:{id:'main-frame'}}}:{};}},line=>writes.push(JSON.parse(line)),()=>{closes++;});
 const send=async command=>{relay.receive(JSON.stringify(command));await Promise.resolve();await Promise.resolve();await Promise.resolve();};
 await send({id:1,method:'Target.getTargets',params:{}});await send({id:2,method:'Target.attachToTarget',params:{targetId:'page',flatten:true}});
 const sessionId=writes.find(message=>message.id===2).result.sessionId;
 await send({id:3,method:'Page.enable',params:{},sessionId});await send({id:4,method:'Target.setDiscoverTargets',params:{discover:true}});
 if(autoAttach)await send({id:5,method:'Target.setAutoAttach',params:{autoAttach:true,waitForDebuggerOnStart:false,flatten:true}});
 await send({id:6,method:'Page.getFrameTree',params:{},sessionId});
 const before=writes.length;relay.event('Page.loadEventFired',{timestamp:1},undefined);relayCases.push({autoAttach,sessionId,responses:writes.slice(0,before),events:writes.slice(before),calls,closes});
}
fs.writeFileSync(output,JSON.stringify({revision,sources,scope:'Exact report/reportLoaded/withoutDataPayload bodies plus complete CdpRelay; inert page, history, manager and debugger target',cases,sanitizers,relayCases},null,2)+'\n');
console.log(`${cases.length} native report observations, ${sanitizers.length} data redaction cases, ${relayCases.length} actual relay event-gating traces`);
