// Private framed worker entry. Rust owns the scoped endpoint and close receipt.
import fs from 'node:fs';import path from 'node:path';import {createRequire} from 'node:module';import {pathToFileURL} from 'node:url';import {createHash} from 'node:crypto';
import {FrameDecoder,MAGIC,MAX_FRAME_BYTES as MAX} from './framed-ipc.mjs';
import {RetainingArtifactFs} from './retaining-artifact-fs.mjs';import {attachPage} from './page-worker.mjs';
import {OwnedCdpClient} from './owned-cdp-client.mjs';import {TabCdpRelay} from './tab-cdp-relay.mjs';import {OwnerScopeTransport} from './owner-scope-transport.mjs';import {permitsPageOperation} from './tool-page-operations.mjs';
if(process.versions.node!=='26.13.0'){process.stderr.write('NATIVE_PAGE_NODE_VERSION_UNAVAILABLE\n');process.exit(2);}
let initializing=false,ready=false,closing=false,nonce,page,artifacts;let lastId=0,finishTask,nativePageModule;let ownerId=0,ownerActive=false;let active=0,tail=Promise.resolve();let upstream,relay,scopeTransport,runtimeScope;
function keys(value,allowed){return value&&typeof value==='object'&&!Array.isArray(value)&&Object.keys(value).every(k=>allowed.includes(k));}
async function send(value){const body=Buffer.from(JSON.stringify(value));if(body.length>MAX)throw Error('frame_limit');const header=Buffer.alloc(12);MAGIC.copy(header);header.writeUInt32LE(body.length,8);await new Promise((resolve,reject)=>process.stdout.write(Buffer.concat([header,body]),error=>error?reject(error):resolve()));}
function fatal(){if(closing)return;closing=true;process.stderr.write('NATIVE_PAGE_WORKER_PROTOCOL_FAILURE\n');process.exitCode=2;process.stdin.destroy();void finish(false);}
function verifyFile(root,name,digest){if(path.isAbsolute(name)||name.split('/').includes('..'))throw Error('manifest_path');const file=path.join(root,name);if(!fs.lstatSync(file).isFile()||fs.lstatSync(file).isSymbolicLink()||createHash('sha256').update(fs.readFileSync(file)).digest('hex')!==digest)throw Error('installation_integrity');}
async function initialize(packet){
 if(initializing||!keys(packet,['kind','protocol','nonce','targetId','endpoint','configuration'])||packet.protocol!==1||typeof packet.nonce!=='string'||!/^[a-f0-9]{48}$/.test(packet.nonce)||typeof packet.targetId!=='string'||!packet.targetId)throw Error('init');
 initializing=true;nonce=packet.nonce;const c=packet.configuration;
 if(!keys(c,['nativeRoot','playwrightRoot','effectRoot','ownerScopes'])||!['nativeRoot','playwrightRoot','effectRoot'].every(k=>typeof c[k]==='string'&&path.isAbsolute(c[k])))throw Error('configuration');
 let endpoint=new URL(packet.endpoint);if(endpoint.protocol!=='ws:'||endpoint.hostname!=='127.0.0.1'||!endpoint.port||endpoint.username||endpoint.password||endpoint.search||endpoint.hash||endpoint.pathname.length<32)throw Error('grant');
 const native=JSON.parse(fs.readFileSync(new URL('./native-manifest.json',import.meta.url),'utf8'));for(const [name,digest]of Object.entries(native.files))verifyFile(c.nativeRoot,name,digest);
 const playwright=JSON.parse(fs.readFileSync(new URL('./playwright-manifest.json',import.meta.url),'utf8'));if(playwright.version!=='1.60.0')throw Error('playwright_version');for(const row of playwright.files){if(!row.path.startsWith('package/'))throw Error('playwright_path');verifyFile(c.playwrightRoot,row.path.slice(8),row.sha256);}
 const effect=JSON.parse(fs.readFileSync(new URL('./effect-manifest.json',import.meta.url),'utf8'));for(const [name,digest]of Object.entries(effect.files))verifyFile(c.effectRoot,name,digest);
 if(JSON.parse(fs.readFileSync(path.join(c.effectRoot,'package.json'),'utf8')).version!=='4.1.2')throw Error('effect_version');
 if(fs.realpathSync(path.join(c.nativeRoot,'node_modules/@t3tools/contracts'))!==fs.realpathSync(path.join(c.nativeRoot,'packages/contracts'))||fs.realpathSync(path.join(c.nativeRoot,'node_modules/effect'))!==fs.realpathSync(c.effectRoot))throw Error('module_identity');
 const home=process.env.HOME;if(!home||!path.isAbsolute(home))throw Error('home');for(const name of ['playwright-temp','quarantine','guard'])fs.mkdirSync(path.join(home,name),{mode:0o700});
 process.env.TMPDIR=path.join(home,'playwright-temp');process.env.O3_RECURSIVE_FS_GUARD_DIR=path.join(home,'guard');await import('./fs-guard.mjs');
 artifacts=new RetainingArtifactFs({tempRoot:process.env.TMPDIR,quarantineRoot:path.join(home,'quarantine')});artifacts.install();
 const require=createRequire(path.join(c.nativeRoot,'package.json'));const schemaPath=require.resolve('effect/Schema');if(!fs.realpathSync(schemaPath).startsWith(fs.realpathSync(c.effectRoot)+path.sep))throw Error('effect_identity');
 const Schema=await import(pathToFileURL(schemaPath).href);const nativePage=await import(pathToFileURL(path.join(c.nativeRoot,'apps/server/src/preview/ServerBrowserPage.ts')).href);nativePageModule=nativePage;const nativeContracts=await import(pathToFileURL(path.join(c.nativeRoot,'packages/contracts/src/previewAutomation.ts')).href);
 const playwrightRequire=createRequire(path.join(c.playwrightRoot,'package.json'));const {chromium}=playwrightRequire(c.playwrightRoot);
 const attach=()=>attachPage({chromium,endpoint:endpoint.href,nativePage,nativeContracts,Schema,admitted:()=>!closing,artifactFs:artifacts,targetId:packet.targetId});
 if(c.ownerScopes!==undefined){
  if(!keys(c.ownerScopes,['runtimeReceipt','cleanupReceipt'])||!['runtimeReceipt','cleanupReceipt'].every(key=>typeof c.ownerScopes[key]==='string'&&/^[a-f0-9]{48}$/.test(c.ownerScopes[key]))||c.ownerScopes.runtimeReceipt===c.ownerScopes.cleanupReceipt)throw Error('owner_configuration');
  const {ws:WebSocket,wsServer:WebSocketServer}=playwrightRequire(path.join(c.playwrightRoot,'lib/utilsBundle.js'));
  upstream=await OwnedCdpClient.open({WebSocket,endpoint:endpoint.href});
  relay=new TabCdpRelay({upstream,targetId:packet.targetId,admitted:()=>!closing,WebSocketServer,requireReceipt:true,cleanupReceipt:c.ownerScopes.cleanupReceipt});
  endpoint=new URL(await relay.listen());
  scopeTransport=new OwnerScopeTransport({WebSocket,endpoint:endpoint.href,onFault:fatal});
  runtimeScope=scopeTransport.admit(c.ownerScopes.runtimeReceipt);
  page=await scopeTransport.run(runtimeScope,attach);
 }else page=await attach();
 if(closing){await page.close();return;}
 ready=true;await send({kind:'ready',protocol:1,nonce});
}
async function invoke(packet){
 if(!ready||closing||!keys(packet,['kind','id','nonce','operation','input','timeoutMs','ownerScope'])||packet.nonce!==nonce||!Number.isSafeInteger(packet.id)||packet.id<=lastId||!Number.isSafeInteger(packet.timeoutMs)||packet.timeoutMs<=0||packet.timeoutMs>120000||typeof packet.operation!=='string'||active>=8)throw Error('invoke');
 lastId=packet.id;
 const aliases=new Map(Object.entries({preview_click:'click',preview_type:'type',preview_press:'press',preview_snapshot:'snapshot',preview_evaluate:'evaluate',preview_wait_for:'waitFor'}));
 let operation=aliases.get(packet.operation)??packet.operation,input=packet.input;
 if(operation==='preview_act'){if(!input||!permitsPageOperation('preview_act',input.action))throw Error('action');const {action,...rest}=input;operation=action;input=rest;}
 let scope;
 if(scopeTransport){
  if(!keys(packet.ownerScope,['receipt','tool'])||!permitsPageOperation(packet.ownerScope.tool,operation))throw Error('operation_scope');
  scope=scopeTransport.admit(packet.ownerScope.receipt,packet.ownerScope.tool);
 }else if(packet.ownerScope!==undefined)throw Error('unexpected_owner_scope');
 active++;
 const run=async()=>{
  if(closing)return;
  let result;
  try{const invoke=()=>page.invoke(operation,input,packet.timeoutMs);const value=await (scopeTransport?scopeTransport.run(scope,invoke):invoke());result={kind:'result',id:packet.id,nonce,ok:true,value:value??null};}
  catch(error){const failure=nativePageModule.toOperationError(error,packet.timeoutMs);result={kind:'result',id:packet.id,nonce,ok:false,error:{tag:failure.tag,message:failure.message,...(failure.detail===undefined?{}:{detail:failure.detail})}};}
  finally{
   try{if(scopeTransport){await upstream.endScope(packet.ownerScope.receipt,packet.ownerScope.tool);scopeTransport.finish(scope);}}
   finally{active--;}
  }
  if(!closing)await send(result);
 };
 // The owner still supplies control receipts. One canonical Page keyboard is
 // shared by legacy aliases and preview_act, never a second Rust key state.
 if(operation==='snapshot'||operation==='waitFor')void run().catch(fatal);
 else{const result=tail.then(run);tail=result.catch(()=>{});void result.catch(fatal);}
}
async function ownerControl(packet){
 if(!ready||closing||!keys(packet,['kind','id','nonce','action'])||packet.nonce!==nonce||!Number.isSafeInteger(packet.id)||packet.id<=ownerId||packet.action!=='invalidateRefs'||ownerActive)throw Error('owner_control');
 ownerId=packet.id;ownerActive=true;
 try{page.invalidateRefs();await send({kind:'owner-result',nonce,id:packet.id,ok:true});}
 finally{ownerActive=false;}
}
function finish(success=true){
 if(finishTask)return finishTask;closing=true;ready=false;finishTask=(async()=>{
 try{if(page)await page.close();if(relay)await relay.close();if(scopeTransport){scopeTransport.finish(runtimeScope);scopeTransport.dispose();}upstream?.close();if(artifacts&&!artifacts.healthy)success=false;const guardRefusals=fs.readdirSync(path.join(process.env.HOME,'guard')).filter(name=>name.endsWith('.attempts.jsonl')).length;if(guardRefusals)success=false;await send({kind:'closed',nonce,artifactHealthy:!!artifacts?.healthy,guardRefusals,nativeRecursiveRequests:artifacts?.receipts.length??0,executedRecursiveRemovals:0});}
 catch{success=false;}
 process.exit(success?0:2);
 })();return finishTask;
}
const decoder=new FrameDecoder(packet=>{
 if(packet.kind==='init')void initialize(packet).catch(fatal);
 else if(packet.kind==='invoke')void invoke(packet).catch(fatal);
 else if(packet.kind==='owner-control')void ownerControl(packet).catch(fatal);
 else if(packet.kind==='close'&&packet.nonce===nonce&&keys(packet,['kind','nonce']))void finish();
 else throw Error('frame_kind');
});
process.stdin.on('data',chunk=>{try{decoder.feed(chunk);}catch{fatal();}});
process.stdin.on('end',()=>{if(!closing)void finish(false);});

process.on('SIGTERM',()=>{void finish(false);});
process.on('SIGINT',()=>{void finish(false);});
