// Whole native toolkit evaluated with only inert service dependencies injected.
import fs from 'node:fs';import vm from 'node:vm';import {stripTypeScriptTypes,createRequire} from 'node:module';import {pathToFileURL} from 'node:url';import assert from 'node:assert/strict';
assert(fs.existsSync('/run/.containerenv'));
const require=createRequire('/deps/package.json');const loaded=new Map();
const load=async name=>import(pathToFileURL(require.resolve(name)).href);
const Context=await load('effect/Context'),Schema=await load('effect/Schema'),{Tool}=await load('effect/ai');
const services={
 '../../McpInvocationContext.ts':{McpInvocationContext:Context.Service('oracle/McpInvocationContext')},
 '../../PreviewAutomationBroker.ts':{PreviewAutomationBroker:Context.Service('oracle/PreviewAutomationBroker')},
 '../../../config.ts':{ServerConfig:Context.Service('oracle/ServerConfig')},
 '../../../orchestration-v2/ThreadManagementService.ts':{ThreadManagementService:Context.Service('oracle/ThreadManagementService')}
};
const source=fs.readFileSync('/native/apps/server/src/mcp/toolkits/preview/tools.ts','utf8');const module=new vm.SourceTextModule(stripTypeScriptTypes(source,{mode:'strip'}));
await module.link(async specifier=>{
 if(loaded.has(specifier))return loaded.get(specifier);
 const values=services[specifier]??(specifier==='@t3tools/contracts'?await import('/native/packages/contracts/src/index.ts'):await load(specifier));
 const synthetic=new vm.SyntheticModule(Object.keys(values),function(){for(const [name,value]of Object.entries(values))this.setExport(name,value);});loaded.set(specifier,synthetic);return synthetic;
});await module.evaluate();
const tools=Object.values(module.namespace.PreviewToolkit.tools);assert.equal(tools.length,12);
const catalog=tools.map(tool=>({name:tool.name,description:Tool.getDescription(tool),inputSchema:Tool.getJsonSchema(tool),readonly:Context.get(tool.annotations,Tool.Readonly),destructive:Context.get(tool.annotations,Tool.Destructive),idempotent:Context.get(tool.annotations,Tool.Idempotent),openWorld:Context.get(tool.annotations,Tool.OpenWorld)}));
const contracts=await import('/native/packages/contracts/src/previewAutomation.ts');const actions=[];
for(const [action,schema]of Object.entries(contracts.PreviewAutomationActInputs)){
 for(const value of [{},{locator:'#x',text:'a',key:'Enter',values:['x'],paths:[],source:'#a',target:'#b',deltaY:1},{x:1},{x:1,y:2,timeoutMs:-1},{locator:'',timeoutMs:0}]){
  try{const decoded=Schema.decodeUnknownSync(schema)(value);actions.push({action,input:value,valid:true,decoded});}catch{actions.push({action,input:value,valid:false});}
 }
}
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,wholeToolkit:true,injectedBoundaries:'Opaque service dependencies only; actual Tool/Toolkit/Schema/contracts',catalog,actions},null,2)+'\n');console.log(JSON.stringify({tools:catalog.length,actionCases:actions.length}));
