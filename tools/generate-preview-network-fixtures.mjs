#!/usr/bin/env node
// Execute pinned native request/response/failure callbacks and abort settlement with inert services.
import fs from 'node:fs';import crypto from 'node:crypto';import {execFileSync} from 'node:child_process';import {stripTypeScriptTypes} from 'node:module';
const[checkout,output,managerPath,networkPath]=process.argv.slice(2);if(!networkPath)throw Error('usage: generate-preview-network-fixtures.mjs CHECKOUT OUTPUT CR_NETWORK_MANAGER_SOURCE NETWORK_SOURCE');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',sources={};
const source=path=>{const value=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});sources[path]=crypto.createHash('sha256').update(value).digest('hex');return value;};
const browser=source('apps/server/src/preview/ServerBrowser.ts'),pageSource=source('apps/server/src/preview/ServerBrowserPage.ts');
const part=(value,from,to)=>{const a=value.indexOf(from),b=value.indexOf(to,a);if(a<0||b<0)throw Error('pinned source boundary missing');return stripTypeScriptTypes(value.slice(a,b).replaceAll('export const','const').replaceAll('export function','function'));};
const sanitizer=part(pageSource,'export const withoutDataPayload =','\n/**');
const errors=part(pageSource,'const NET_ERROR_CODES:','export const recordingEncoderScript');
const bounded=part(browser,'const pushBounded =','const viewportSettingsEqual');
const limits=[browser.match(/^const ABORTED_NAVIGATION_DOWNLOAD_WAIT_MS = .*;$/m)?.[0],pageSource.match(/^export const DIAGNOSTIC_BUFFER_LIMIT = .*;$/m)?.[0]?.replace('export ','')];if(limits.some(value=>!value))throw Error('pinned limits missing');
const clearing=part(browser,'  const clearAbortedNavigation =','  const saveDownload =');
const callbacks=part(browser,'    const isMainNavigation =','    page.on("console"');
const run=new Function('scenario',`${sanitizer}\n${errors}\nreturn(()=>{
 const handlers={},mainFrame={},childFrame={},requests=new Map(),reports=[],timers=[];let loaded=0,clock=0;
 class Date extends globalThis.Date{constructor(){super(globalThis.Date.UTC(2026,10,7,0,0,clock));}}
 const setTimeout=(callback,delay)=>{timers.push({callback,delay,active:true});return timers.length;};const clearTimeout=id=>{if(timers[id-1])timers[id-1].active=false;};
 ${limits.join("\n")}
 const ServerBrowserPage={withoutDataPayload,parseNetError,DIAGNOSTIC_BUFFER_LIMIT};
 ${bounded}
 const tab={navigationGeneration:0,loading:false,closing:false,abortedNavigation:null,networkEntries:[],fileChooser:null,page:{url:()=>scenario.pageUrl??'about:blank'}};
 const page={on:(name,callback)=>handlers[name]=callback,mainFrame:()=>mainFrame};const report=(_tab,nav)=>reports.push(nav);const reportLoaded=async()=>{loaded++;};const desktop=true,adopted=false;const recordFrameCopies=()=>{};
 ${clearing}\n${callbacks}
 const request=(step)=>{if(!requests.has(step.id))requests.set(step.id,{data:step,url(){return this.data.url??'https://example.com/';},method(){return this.data.verb??'GET';},frame(){return this.data.main===false?childFrame:mainFrame;},isNavigationRequest(){return this.data.navigation!==false;},failure(){return this.failureValue??null;}});return requests.get(step.id);};
 const steps=[];for(const[index,step]of scenario.steps.entries()){clock=index;const before=reports.length,previousLoaded=loaded;
  switch(step.kind){case'request':{const value=request(step);handlers.request(value);break;}case'failed':{const value=request(step);value.failureValue=step.errorText===undefined?null:{errorText:step.errorText};handlers.requestfailed(value);break;}case'response':{const value=request(step);handlers.response({url:()=>step.url??value.url(),request:()=>value,status:()=>step.status});break;}case'load':handlers.load();break;case'frame':handlers.framenavigated(step.main===false?childFrame:mainFrame);break;case'closing':tab.closing=step.value;break;case'pageUrl':scenario.pageUrl=step.url;break;case'tick':for(const timer of timers.filter(timer=>timer.active)){timer.active=false;timer.callback();}break;case'download':{const pending=tab.abortedNavigation;if(pending&&pending.url===step.url){const generation=pending.generation;clearAbortedNavigation(tab);settleAbortedNavigation(tab,generation,step.url,step.offered);}break;}default:throw Error('unknown scenario event');}
  steps.push({networkLength:tab.networkEntries.length,lastNetwork:tab.networkEntries.at(-1)??null,generation:tab.navigationGeneration,loading:tab.loading,reports:reports.slice(before),loadedCalls:loaded-previousLoaded,aborted:tab.abortedNavigation?{url:tab.abortedNavigation.url,generation:tab.abortedNavigation.generation}:null,timerDelays:timers.filter(timer=>timer.active).map(timer=>timer.delay)});
 }return{steps,networkEntries:tab.networkEntries};})();`);
const scenarios=[];
for(const errorText of ['net::ERR_FAILED','net::ERR_TIMED_OUT','net::ERR_CONNECTION_CLOSED','net::ERR_CONNECTION_RESET','net::ERR_CONNECTION_REFUSED','net::ERR_NAME_NOT_RESOLVED','net::ERR_INTERNET_DISCONNECTED','net::ERR_ADDRESS_UNREACHABLE','net::ERR_CERT_AUTHORITY_INVALID','net::ERR_EMPTY_RESPONSE','net::ERR_HTTP2_PROTOCOL_ERROR','lowercase error','net::ERR_ABORTED'])for(const main of [false,true])for(const navigation of [false,true])for(const tracked of [false,true])for(const newer of [false,true]){
 const old={id:'old',url:'data:text/plain;private=HIDDEN,HIDDEN',verb:'POST',main,navigation};scenarios.push({steps:[...(tracked?[{kind:'request',...old}]:[]),...(newer?[{kind:'request',id:'new',url:'https://example.com/new'}]:[]),{kind:'failed',...old,errorText},{kind:'tick'}]});
}
for(const url of ['https://example.com/','data:text/html;private=HIDDEN,HIDDEN','"\\x64ata:text/plain,private'])for(const verb of ['GET','POST','PATCH'])for(const status of [0,200,302,404,503])scenarios.push({steps:[{kind:'request',id:'r',url,verb,main:false},{kind:'response',id:'r',status}]});
for(const pageUrl of ['about:blank','https://example.com/already'])for(const offered of [undefined,{id:'owned-download',fileName:'fixture.txt'}])for(const closing of [false,true])scenarios.push({pageUrl,steps:[{kind:'request',id:'r',url:'https://example.com/download'},{kind:'failed',id:'r',errorText:'net::ERR_ABORTED'},{kind:'closing',value:closing},{kind:'download',url:'https://example.com/download',...(offered===undefined?{}:{offered})},{kind:'tick'}]});
scenarios.push({steps:[{kind:'request',id:'r',main:false},...Array.from({length:205},(_,i)=>({kind:'response',id:'r',url:'https://example.com/'+i,status:200}))]});
scenarios.push({steps:[{kind:'request',id:'a',url:'https://example.com/a'},{kind:'failed',id:'a',errorText:'ERR_ABORTED'},{kind:'request',id:'b',url:'https://example.com/b'},{kind:'tick'},{kind:'load'},{kind:'frame',main:false},{kind:'frame'}]});
let adapterCases=[],upstream={};
{
 const manager=fs.readFileSync(managerPath,'utf8'),network=fs.readFileSync(networkPath,'utf8');
 upstream={version:'1.60.0',managerSha256:crypto.createHash('sha256').update(manager).digest('hex'),networkSha256:crypto.createHash('sha256').update(network).digest('hex'),managerUrl:'https://raw.githubusercontent.com/microsoft/playwright/v1.60.0/packages/playwright-core/src/server/chromium/crNetworkManager.ts',networkUrl:'https://raw.githubusercontent.com/microsoft/playwright/v1.60.0/packages/playwright-core/src/server/network.ts',scope:'Exact isNavigationRequest predicate and stripFragmentFromUrl helper; reviewed data/unknown-ID/response-URL policies'};
 if(upstream.managerSha256!=='dcdaa9a027a5980f6ae99384b837fbcf244806c0b0c45a7290bbd6d995a7673d'||upstream.networkSha256!=='072589438a49b2a29c36a0d8d2dd4813761db7b512397ef305bf48277ca52ae8')throw Error('pinned upstream source mismatch');
 const predicate=manager.match(/const isNavigationRequest = ([^;]+);/)?.[1];if(!predicate)throw Error('navigation predicate missing');
 const classify=new Function('requestWillBeSentEvent','return '+predicate);const strip=new Function(part(network,'export function stripFragmentFromUrl','export type ResourceType')+';return stripFragmentFromUrl;')();
 for(const id of ['r','loader'])for(const loaderId of ['r','loader',undefined])for(const type of ['Document','Image','Other',undefined])for(const url of ['https://example.com/a#fragment','https://example.com/a','data:text/plain,HIDDEN','DATA:text/plain,HIDDEN'])for(const frameId of ['main-frame','child-frame']){
  const input={requestId:id,...(loaderId===undefined?{}:{loaderId}),...(type===undefined?{}:{type}),frameId,request:{url,method:'GET'}};adapterCases.push({input,expected:url.startsWith('data:')?null:{kind:'request',id,url:strip(url),verb:'GET',main:frameId==='main-frame',navigation:classify(input)}});
 }
}
const cases=scenarios.map(input=>({input,expected:run(structuredClone(input))}));fs.writeFileSync(output,JSON.stringify({revision,sources,scope:'Actual request/response/requestfailed/load/framenavigated callbacks, pushBounded, abort clearing/settlement and parseNetError; inert page/report/timer services',cases,upstream,adapterCases},null,2)+'\n');console.log(`${cases.length} complete native network/failure/abort callback traces, ${adapterCases.length} pinned upstream request classification cases`);
