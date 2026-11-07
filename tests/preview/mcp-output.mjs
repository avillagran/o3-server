// Actual pinned toolkit + complete Effect MCP registration/handler encoding.
import fs from 'node:fs';import vm from 'node:vm';import {stripTypeScriptTypes,createRequire} from 'node:module';import {pathToFileURL} from 'node:url';import assert from 'node:assert/strict';
assert(fs.existsSync('/run/.containerenv'));fs.mkdirSync('/out/guard',{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const require=createRequire('/deps/package.json'),loaded=new Map();const load=async name=>import(pathToFileURL(require.resolve(name)).href);
const Context=await load('effect/Context'),Effect=await load('effect/Effect'),{Toolkit,McpServer}=await load('effect/ai');
const services={
 '../../McpInvocationContext.ts':{McpInvocationContext:Context.Service('oracle/McpInvocationContext')},
 '../../PreviewAutomationBroker.ts':{PreviewAutomationBroker:Context.Service('oracle/PreviewAutomationBroker')},
 '../../../config.ts':{ServerConfig:Context.Service('oracle/ServerConfig')},
 '../../../orchestration-v2/ThreadManagementService.ts':{ThreadManagementService:Context.Service('oracle/ThreadManagementService')}
};
const module=new vm.SourceTextModule(stripTypeScriptTypes(fs.readFileSync('/native/apps/server/src/mcp/toolkits/preview/tools.ts','utf8'),{mode:'strip'}));
await module.link(async specifier=>{
 if(loaded.has(specifier))return loaded.get(specifier);
 const values=services[specifier]??(specifier==='@t3tools/contracts'?await import('/native/packages/contracts/src/index.ts'):await load(specifier));
 const synthetic=new vm.SyntheticModule(Object.keys(values),function(){for(const [name,value]of Object.entries(values))this.setExport(name,value);});loaded.set(specifier,synthetic);return synthetic;
});await module.evaluate();
const tool=module.namespace.PreviewToolkit.tools.preview_evaluate;assert(tool);const toolkit=Toolkit.make(tool),cases=[];
for(const [name,value]of [['high','\ud800'],['low','\udfff'],['pair','😀'],['object-key',{'\ud800':'\udfff'}],['literal-escape','\\ud800'],['controls','\0\n\t\u2028'],['nested',{list:['a\ud800b',{last:'\udfff'}]}]]){
 const registrations=[];
 const registry={addTool:registration=>Effect.sync(()=>{registrations.push(registration);})};
 let register=McpServer.registerToolkit(toolkit).pipe(Effect.provide(toolkit.toLayer({preview_evaluate:()=>Effect.succeed({value})})),Effect.provideService(McpServer.McpServer,registry));
 for(const exported of Object.values(services))for(const service of Object.values(exported))register=register.pipe(Effect.provideService(service,{}));
 await Effect.runPromise(register);assert.equal(registrations.length,1);
 const result=await Effect.runPromise(registrations[0].handle({expression:'synthetic private value'}).pipe(Effect.provide(registrations[0].services)));
 assert.equal(result.isError,false);assert.deepEqual(result.structuredContent.value,value);assert.equal(result.content[0].type,'text');assert.deepEqual(JSON.parse(result.content[0].text).value,value);
 const encoded=JSON.stringify({jsonrpc:'2.0',id:1,result});const decoded=JSON.parse(encoded);assert.deepEqual(decoded.result.structuredContent.value,value);
 cases.push({name,encoded});
}
assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,wholeNativeToolkit:true,wholeEffectMcpRegisterToolkit:true,injectedBoundaries:'Opaque services and synthetic application handler only; no native policy/HTTP claim',cases},null,2)+'\n');console.log('NATIVE_MCP_UTF16_OUTPUT_PASS');
