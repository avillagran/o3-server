// Execute complete pinned native lifetime helpers with explicit resource doubles.
import fs from "node:fs";
import vm from "node:vm";
import crypto from "node:crypto";
import { execFileSync } from "node:child_process";
import { stripTypeScriptTypes } from "node:module";
const [root, output] = process.argv.slice(2);
const revision = "4cb6945dc60ed03f567dc5daafd083e52308e82b";
const file = "apps/server/src/preview/ServerBrowser.ts";
if (!root || !output) throw new Error("Exact source root and explicit output are required");
if (execFileSync("git", ["-C", root, "rev-parse", "HEAD"], {encoding:"utf8"}).trim() !== revision) throw new Error("Wrong native revision");
const source = fs.readFileSync(root + "/" + file, "utf8");
if (execFileSync("git", ["-C",root,"show",revision+":"+file], {encoding:"utf8"}) !== source) throw new Error("Changed native source");
function extract(start, end) {
  const a = source.indexOf(start), b = source.indexOf(end, a);
  if (a < 0 || b <= a) throw new Error("Native extraction boundary changed: " + start);
  return source.slice(a,b);
}
const body = [
  extract("const AGENT_TAB_LIMIT =", "const DOWNLOAD_LIMIT ="),
  extract("const tabKey =", "const pushBounded ="),
  extract("  const openingReservations =", "  // A Playwright launch"),
  extract("  // Desktop browsing bypasses streamed viewers", "  const tabOwner ="),
  extract("  const markUsed =", "  const runOperation ="),
].join("\n");
const script = stripTypeScriptTypes(body, {mode:"strip"});
function environment(input) {
  const tabs = new Map(), pendingTabs = new Map(), dropped = [];
  const context = vm.createContext({tabs,pendingTabs,dropped,Date:{now:()=>input.now},desktopChannel:{isShown:tab=>tab.shown},dropTab:tab=>{dropped.push(tab.index);tabs.delete(tab.threadId+"\0"+tab.tabId);}});
  input.tabs.forEach((tab,index)=>tabs.set(tab.threadId+"\0"+tab.tabId,{...tab,index,desktop:tab.desktop?{}:null,control:{agentId:tab.agentId},viewers:new Set(Array.from({length:tab.viewers},(_,n)=>n))}));
  input.pending.forEach(p=>pendingTabs.set(p.threadId+"\0"+p.tabId,{agentId:p.agentId}));
  context.input=input;
  vm.runInContext(script,context);
  return {context,dropped};
}
function native(input) {
  const {context}=environment(input);
  const result=vm.runInContext(`(()=>{
    const result={serverFull:serverAtTabLimit(),agentFull:agentAtTabLimit(input.agent),idle:idleAgentTabs().map(tab=>tab.index),latest:latestThreadTab(input.thread,input.latestAgent??undefined)?.index??null};
    freeIdleAgentSlot(input.agent);result.reclaim=dropped.splice(0);
    return result;
  })()`,context);
  const sweep=environment(input);
  vm.runInContext('closeIdleAgentTabs()',sweep.context);
  result.headless=sweep.dropped;
  return result;
}
// Each observation gets a fresh Map so one simulated drop cannot change a
// separate decision's tie ordering. No browser/resource cleanup runs here.
function observe(input) {
  const first=native(input);
  const tabs=new Map(),pendingTabs=new Map();
  input.tabs.forEach((tab,index)=>tabs.set(tab.threadId+"\0"+tab.tabId,{...tab,index,control:{agentId:tab.agentId}}));
  const context=vm.createContext({tabs,pendingTabs,Date:{now:()=>input.now},input});
  vm.runInContext(script,context);
  first.marked=vm.runInContext('markUsed({threadId:input.thread,agentSessionId:input.agent,tabId:input.explicit??undefined}); [...tabs.values()].map(tab=>tab.usedAt)',context);
  return first;
}
const tab=(index,extra={})=>({threadId:"thread",tabId:"tab-"+index,agentId:"agent-"+Math.floor(index/8),desktop:false,viewers:0,shown:false,usedAt:index,createdAt:index,...extra});
const cases=[];
const add=input=>cases.push({input,result:observe(input)});
for(const count of [0,1,7,8,9,31,32,33])for(const age of [0,1_799_999,1_800_000,1_800_001,1_860_000])for(const mode of ["headless","desktop","shown","viewer","unclaimed","mixed"]){
  const tabs=Array.from({length:count},(_,i)=>tab(i,{desktop:mode!=="headless",shown:mode==="shown"||mode==="mixed"&&i%3===0,viewers:mode==="viewer"?1:0,agentId:mode==="unclaimed"?null:"agent-"+Math.floor(i/8)}));
  add({tabs,pending:[],now:age,thread:"thread",agent:"fresh",latestAgent:"agent-0",explicit:count?"tab-0":null});
}
for(const own of [0,7,8])for(const reservations of [0,1,8,31,32])for(const overlap of [false,true]){
  const tabs=Array.from({length:32},(_,i)=>tab(i,{desktop:true,usedAt:i<3?0:i,createdAt:1,agentId:i<own?"fresh":"other"}));
  const pending=Array.from({length:reservations},(_,i)=>({threadId:"thread",tabId:(overlap?"tab-":"pending-")+i,agentId:i%2?"fresh":null}));
  add({tabs,pending,now:1_800_001,thread:"thread",agent:"fresh",latestAgent:null,explicit:"tab-2"});
}
add({tabs:[tab(0,{agentId:"fresh",createdAt:9}),tab(1,{agentId:"fresh",createdAt:9,usedAt:99}),tab(2,{threadId:"other",viewers:2})],pending:[],now:2_000_000,thread:"thread",agent:"fresh",latestAgent:"fresh",explicit:"tab-2"});
const constants=vm.createContext({});vm.runInContext(stripTypeScriptTypes(extract("const AGENT_TAB_LIMIT =","const DOWNLOAD_LIMIT ="),{mode:"strip"}),constants);
const values=vm.runInContext('({agent:AGENT_TAB_LIMIT,server:SERVER_TAB_LIMIT,idle:AGENT_TAB_IDLE_MS,sweep:IDLE_SWEEP_INTERVAL})',constants);
const metadata={revision,sources:{[file]:crypto.createHash("sha256").update(source).digest("hex")},constants:values};
fs.writeFileSync(output,JSON.stringify(metadata).slice(0,-1)+',"cases":[\n'+cases.map(row=>JSON.stringify(row)).join(',\n')+'\n]}\n',{flag:"wx"});
