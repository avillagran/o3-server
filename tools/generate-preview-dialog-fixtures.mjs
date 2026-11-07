#!/usr/bin/env node
// Execute native dialog status/broadcast/resolution and pinned Playwright CDP adaptation.
import NodePath from 'node:path';import fs from 'node:fs';import crypto from 'node:crypto';import {execFileSync} from 'node:child_process';import {createRequire,stripTypeScriptTypes} from 'node:module';import {pathToFileURL} from 'node:url';
const[checkout,staged,crPagePath,dialogPath,output]=process.argv.slice(2);if(!output)throw Error('CHECKOUT STAGED_NATIVE CR_PAGE_SOURCE DIALOG_SOURCE OUTPUT required');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87',hash=value=>crypto.createHash('sha256').update(value).digest('hex');
const browserPath='apps/server/src/preview/ServerBrowser.ts',browser=execFileSync('git',['-C',checkout,'show',`${revision}:${browserPath}`],{encoding:'utf8'});
const crPage=fs.readFileSync(crPagePath,'utf8'),dialogSource=fs.readFileSync(dialogPath,'utf8');
if(hash(crPage)!=='90ae14016912a7b75b98ed73221a511dd0c2f56ad27b44a89ea7f98914affe7f'||hash(dialogSource)!=='41040c64aa2dd2be43257eef4abc40f9edb48fb4cf263efe819e1cb1ba814efe')throw Error('Pinned Playwright1.60 source mismatch');
const provenance=JSON.parse(fs.readFileSync(staged+'/SOURCE.json','utf8'));if(provenance.revision!==revision)throw Error('Pinned contracts required');
for(const[path,digest]of Object.entries(provenance.files))if(hash(fs.readFileSync(staged+'/'+path))!==digest)throw Error('Changed source '+path);
const require=createRequire(staged+'/package.json'),Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
if(JSON.parse(fs.readFileSync(require.resolve('effect/package.json'),'utf8')).version!=='4.1.2')throw Error('Effect4.1.2 required');
const previewContracts=await import(pathToFileURL(staged+'/packages/contracts/src/previewAutomation.ts').href);const{PreviewAutomationDialogInput,PreviewAutomationStatus,PreviewAutomationError}=previewContracts;
const{OrchestratorMcpFailure}=await import(pathToFileURL(staged+'/packages/contracts/src/orchestratorMcp.ts').href);
const{ProviderInstanceId}=await import(pathToFileURL(staged+'/packages/contracts/src/providerInstance.ts').href);if(!Schema.is(ProviderInstanceId)('codex'))throw Error('Canonical Effect identity required');
const part=(text,start,end)=>{const a=text.indexOf(start),b=text.indexOf(end,a);if(a<0||b<0)throw Error('Pinned source boundary missing '+start);return text.slice(a,b);};
const js=text=>stripTypeScriptTypes(text.replaceAll('export const','const').replaceAll('export class','class'));
const NativeDialog=new Function('SdkObject','assert',js(part(dialogSource,'export class Dialog extends','export class DialogManager'))+';return Dialog;')(class{},(condition,message)=>{if(!condition)throw Error(message);});
const onProtocolDialog=new Function('dialog','return '+js('function '+part(crPage,'  _onDialog(event:','  _handleException(').trim()))({Dialog:NativeDialog});
const dialogStatus=part(browser,'  const dialogStatus =','  const updateFileChooserInterception =');
const broadcasting=part(browser,'  const broadcastControl =','  const resolveDialog =');
const resolution=part(browser,'  const resolveDialog =','  const report =');
const opening=part(browser,'    page.on("dialog",','    page.on("download",');
const remeasure=part(browser,'      const remeasure = () =>','      cdp.on("Page.frameResized",');
const closing=part(browser,'      cdp.on("Page.javascriptDialogClosed",','      await cdp.send("Page.enable");');
const canonical=async name=>import(pathToFileURL(require.resolve('effect/'+name)).href);
const Tool=await canonical('ai/Tool'),Context=await canonical('Context'),Option=await canonical('Option'),SchemaAST=await canonical('SchemaAST');
const effectRoot=NodePath.dirname(require.resolve('effect/package.json')),mcpSource=fs.readFileSync(effectRoot+'/src/ai/McpServer.ts','utf8');
const InternalStructuredOutput=await import(new URL('./internal/structured-output.js',pathToFileURL(require.resolve('effect/ai/McpServer'))));
const toolSourcePath='apps/server/src/mcp/toolkits/preview/tools.ts',toolSource=execFileSync('git',['-C',checkout,'show',`${revision}:${toolSourcePath}`],{encoding:'utf8'});
const metadataBody=part(toolSource,'const browserTool =','const safeBrowserTool =')+part(toolSource,'const PreviewDialogTool =','const PreviewNavigateTool =');
const PreviewToolFailure=Schema.Union([PreviewAutomationError,OrchestratorMcpFailure]);
const nativeTool=new Function('Tool','PreviewAutomationDialogInput','PreviewAutomationStatus','PreviewToolFailure','dependencies',js(metadataBody)+';return PreviewDialogTool;')(Tool,PreviewAutomationDialogInput,PreviewAutomationStatus,PreviewToolFailure,[]);
const nativeJsonSchema=new Function('Schema','SchemaAST','InternalStructuredOutput',js(part(mcpSource,'const toolJsonSchema =','\n/**'))+';return toolJsonSchema;')(Schema,SchemaAST,InternalStructuredOutput);
const annotationBody=part(part(mcpSource,'    const mcpTool = new McpTool({','    registrations.push({'),'annotations: {','      _meta: toolMeta').replace(/^annotations:\s*/,'').replace(/,\s*$/,'');
const annotations=new Function('tool','Tool','Context','Option','return '+js('('+annotationBody+')')+';')(nativeTool,Tool,Context,Option);
const toolMetadata={name:nativeTool.name,description:Tool.getDescription(nativeTool),inputSchema:nativeJsonSchema(nativeTool.parametersSchema,Tool.getStrictMode(nativeTool)===true),annotations};
const statusWireCases=[{available:false,visible:false,tabId:null,url:null,title:null,loading:false},{available:true,visible:false,tabId:'tab',url:'https://example.com/',title:null,loading:false,dialog:{type:'prompt',message:'Owned',defaultValue:'default'}}].map(input=>({input,expected:Schema.encodeSync(Schema.toCodecJson(PreviewAutomationStatus))(input)}));
const AsyncFunction=Object.getPrototypeOf(async function(){}).constructor;
const run=new AsyncFunction('input','NativeDialog','onProtocolDialog',`${js(dialogStatus)}
 const handlers={},commands=[],aborted=[],controls=[];let invalidations=0,interceptions=0,remeasures=0;
 const units=text=>Array.from({length:text.length},(_,index)=>text.charCodeAt(index));
 const project=value=>value===null?null:{type:value.type,messageUnits:units(value.message),defaultValueUnits:units(value.defaultValue)};
 const capture=value=>controls.push({...value,dialog:project(value.dialog)});
 const ServerBrowserPage={invalidateRefs:()=>invalidations++},updateFileChooserInterception=()=>interceptions++,broadcastViewport=async()=>{remeasures++;},constVoid=()=>{};
 const page={on:(name,callback)=>handlers[name]=callback,mainFrame:()=>({_id:'frame'}),frameManager:{frame:()=>input.frameExists===false?null:{},frameAbortedNavigation:(id,message)=>aborted.push({id,message})},browserContext:{dialogManager:{_dialogWillClose:()=>{},dialogDidOpen:server=>handlers.dialog(client(server))}}};
 const cdp={on:(name,callback)=>handlers[name]=callback};
 const control=input.controller==='human'?{controller:'a',agentId:null,generation:9}:input.controller==='agent'?{controller:null,agentId:'agent',generation:8}:{controller:null,agentId:null,generation:7};
 const tab={dialog:null,page,control,viewers:new Set([{id:'a',canOperate:true,push:capture},{id:'b',canOperate:false,push:capture}])};
 ${js(broadcasting)}\n${js(resolution)}\n${js(opening)}\n${js(remeasure)}\n${js(closing)}
 const client=server=>({type:()=>server.type(),message:()=>server.message(),defaultValue:()=>server.defaultValue(),accept:prompt=>server._accept(prompt),dismiss:()=>server._dismiss()});
 let replacements=0;
 const context={_page:page,_targetId:'frame',_isMainFrame:()=>input.main!==false,_client:{send:async(method,params)=>{
  commands.push({method,params:JSON.parse(JSON.stringify(params))});
  if(input.mode==='fail')throw Error('Synthetic dialog command rejected');
  if(input.mode==='replace'&&replacements===0){replacements++;onProtocolDialog.call(context,{type:'confirm',message:'Replacement '+replacements,defaultPrompt:'replacement'});}
  if(input.mode==='externalClose')await handlers['Page.javascriptDialogClosed']();
 }}};
 if(!input.absent)onProtocolDialog.call(context,{type:input.type,message:input.message,defaultPrompt:input.defaultValue});
 if(input.closeOnly)await handlers['Page.javascriptDialogClosed']();
 const before=dialogStatus(tab),outcomes=[];
 for(let index=0;index<(input.repeat?2:1);index++){
  try{await resolveDialog(tab,{accept:input.accept,...(input.promptText===undefined?{}:{promptText:input.promptText})});outcomes.push({ok:true});}catch(error){outcomes.push({ok:false,message:error.message});}
 }
 const after=dialogStatus(tab);
 return{before:project(before),after:project(after),beforeWellFormed:before===null||before.message.isWellFormed()&&before.defaultValue.isWellFormed(),afterWellFormed:after===null||after.message.isWellFormed()&&after.defaultValue.isWellFormed(),outcomes,commands,aborted,controls,invalidations,interceptions,remeasures};`);
const inputs=[];for(const type of ['alert','confirm','prompt','beforeunload'])for(const accept of [false,true])for(const mode of ['success','replace','externalClose','fail'])for(const controller of ['unclaimed','human','agent'])inputs.push({type,accept,mode,controller,message:'Owned message',defaultValue:'default',promptText:'Answer🙂'});
for(const type of ['alert','beforeunload'])for(const frameExists of [false,true])for(const main of [false,true])for(const accept of [false,true])inputs.push({type,frameExists,main,accept,mode:'success',controller:'unclaimed',message:'Owned frame',defaultValue:''});
for(const value of ['a'.repeat(3999)+'🙂','a'.repeat(4000)+'tail','🙂'.repeat(2001)])for(const field of ['message','defaultValue'])inputs.push({type:'prompt',accept:true,mode:'success',controller:'unclaimed',message:'Owned',defaultValue:'default',[field]:value});
for(const accept of [false,true])inputs.push({absent:true,accept,controller:'unclaimed'});
for(const mode of ['success','fail','replace','externalClose'])inputs.push({type:'prompt',accept:true,mode,repeat:true,controller:'unclaimed',message:'Repeated',defaultValue:''});
inputs.push({type:'alert',accept:false,mode:'success',closeOnly:true,controller:'unclaimed',message:'External close',defaultValue:''});
const cases=[];for(const input of inputs)cases.push({input,expected:await run(input,NativeDialog,onProtocolDialog)});
const schemaCases=[null,false,[],{}, {accept:true},{accept:false,promptText:' exact '},{accept:'true'},{accept:null},{accept:true,promptText:null},{accept:true,promptText:7},{accept:true,tabId:' tab '},{accept:true,tabId:''},{accept:true,extra:'ignored'}].map(input=>{try{return{input,expected:{ok:true,value:Schema.decodeUnknownSync(PreviewAutomationDialogInput)(input)}};}catch(error){return{input,expected:{ok:false,message:error.message}};}});
fs.writeFileSync(output,JSON.stringify({revision,effectVersion:'4.1.2',sources:{[browserPath]:hash(browser),playwrightCrPage:hash(crPage),playwrightDialog:hash(dialogSource)},stagedProvenanceSha256:hash(fs.readFileSync(staged+'/SOURCE.json')),canonicalEffectSchemaModule:require.resolve('effect/Schema'),toolMetadata,toolSource:{file:toolSourcePath,sha256:hash(toolSource)},frameworkSources:{'ai/McpServer.ts':hash(mcpSource),'ai/Tool.ts':hash(fs.readFileSync(effectRoot+'/src/ai/Tool.ts')),'ai/Toolkit.ts':hash(fs.readFileSync(effectRoot+'/src/ai/Toolkit.ts'))},statusWireCases,scope:'Actual native dialogStatus/broadcastControl/resolveDialog/open/desktop-close callbacks and actual pinned Playwright Dialog class/CDP adapter; inert page/protocol/invalidation services; status text projected losslessly to UTF16 units',cases,schemaCases},null,2)+'\n');console.log(cases.length+' complete native dialog lifecycle traces; '+schemaCases.length+' native input schema cases');
