import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {createRequire} from 'node:module';
import {createHash} from 'node:crypto';
import {RetainingArtifactFs} from '/source/runtime/preview/retaining-artifact-fs.mjs';
import {TabCdpRelay} from '/source/runtime/preview/tab-cdp-relay.mjs';
import {FixtureCdp} from '/source/tests/preview/fixture-cdp.mjs';
import {attachPage} from './page-worker.mjs';

assert(fs.existsSync('/run/.containerenv'));
assert.equal(process.versions.node,'26.13.0');
for(const name of ['tmp','quarantine','profile','guard'])fs.mkdirSync('/out/'+name,{mode:0o700});
process.env.TMPDIR='/out/tmp';process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';
await import('/source/runtime/preview/fs-guard.mjs');
const artifacts=new RetainingArtifactFs({tempRoot:'/out/tmp',quarantineRoot:'/out/quarantine'});
artifacts.install();
const require=createRequire('/playwright/package.json');
const {chromium}=require('/playwright');
const {ws:WebSocket,wsServer:WebSocketServer}=require('/playwright/lib/utilsBundle.js');
const nativePage=await import('/native/apps/server/src/preview/ServerBrowserPage.ts');
const nativeContracts=await import('/native/packages/contracts/src/previewAutomation.ts');
const Schema=await import('/deps/node_modules/effect/dist/Schema.js');
const child=spawn('/usr/bin/chromium',['--headless=new','--no-sandbox','--disable-gpu','--no-first-run','--disable-background-networking','--disable-dev-shm-usage','--remote-debugging-port=0','--user-data-dir=/out/profile','about:blank'],{stdio:['ignore','ignore','pipe']});
const childTerminal=new Promise(resolve=>child.once('close',(code,signal)=>resolve({code,signal})));
let stderr='',upstream,relay,attached,ownedCdp,result,failure;
const cases=[];
const hash=data=>createHash('sha256').update(Buffer.from(data,'base64')).digest('hex');
const dimensions=part=>({width:part.width,height:part.height});
const decode=Schema.decodeUnknownSync(nativeContracts.PreviewAutomationSnapshot);
const html='<!doctype html><style>html,body{margin:0;width:100%;height:100%;overflow:hidden}body{background:linear-gradient(to bottom,blue 0 50%,red 50% 100%)}</style><button>Owned full-resolution capture</button>';
async function colors(part){
 return attached.page.evaluate(async data=>{
  const image=await createImageBitmap(new Blob([Uint8Array.from(atob(data),c=>c.charCodeAt(0))],{type:'image/png'}));
  try{
   const canvas=document.createElement('canvas');canvas.width=image.width;canvas.height=image.height;
   const context=canvas.getContext('2d');context.drawImage(image,0,0);
   return [.25,.75].map(y=>Array.from(context.getImageData(Math.floor(image.width*.75),Math.floor(image.height*y),1,1).data));
  }finally{image.close();}
 },part.data);
}
try{
 const endpoint=await new Promise((resolve,reject)=>{
  const timer=setTimeout(()=>reject(Error('Private Chromium startup exceeded20s')),20000);
  child.stderr.on('data',bytes=>{stderr=(stderr+bytes).slice(-4096);const match=/DevTools listening on (ws:\/\/127\.0\.0\.1:[^\s]+)/.exec(stderr);if(match){clearTimeout(timer);resolve(match[1]);}});
  child.once('error',error=>{clearTimeout(timer);reject(error);});
  child.once('exit',()=>{clearTimeout(timer);reject(Error('Private Chromium exited during startup'));});
 });
 const socket=new WebSocket(endpoint);await once(socket,'open');upstream=new FixtureCdp(socket);
 const target=(await upstream.send('Target.createTarget',{url:'about:blank'})).targetId;
 relay=new TabCdpRelay({upstream,targetId:target,admitted:()=>true,WebSocketServer});
 attached=await attachPage({chromium,endpoint:await relay.listen(),targetId:target,nativePage,nativeContracts,Schema,admitted:()=>true,artifactFs:artifacts});
 await attached.page.setViewportSize({width:2048,height:1024});
 await attached.page.setContent(html);
 const beforeInvalid=upstream.calls.length;
 await assert.rejects(attached.invoke('snapshot',{fullScreenshot:'true'}));
 assert.equal(upstream.calls.length,beforeInvalid);
 cases.push({name:'invalid-native-input-refused-before-CDP'});
 const full=decode(await attached.invoke('snapshot',{fullScreenshot:true}));
 assert.deepEqual(dimensions(full.screenshot),{width:1280,height:640});
 assert.deepEqual(dimensions(full.fullScreenshot),{width:2048,height:1024});
 assert.notEqual(hash(full.screenshot.data),hash(full.fullScreenshot.data));
 for(const part of [full.screenshot,full.fullScreenshot]){
  const pixels=await colors(part);assert(pixels[0][2]-pixels[0][0]>100);assert(pixels[1][0]-pixels[1][2]>100);
 }
 cases.push({name:'wide-owned-viewport-keeps-full-and-capped-images',capped:dimensions(full.screenshot),full:dimensions(full.fullScreenshot),pixelsPreserved:true});
 for(const input of [{},{fullScreenshot:false}]){
  const plain=decode(await attached.invoke('snapshot',input));
  assert(!Object.hasOwn(plain,'fullScreenshot'));
  assert.deepEqual(dimensions(plain.screenshot),{width:1280,height:640});
  cases.push({name:Object.hasOwn(input,'fullScreenshot')?'explicit-false-keeps-capped-only':'omitted-keeps-capped-only'});
 }
 await attached.page.setViewportSize({width:640,height:480});
 const small=decode(await attached.invoke('snapshot',{fullScreenshot:true}));
 assert.deepEqual(dimensions(small.screenshot),{width:640,height:480});
 assert(!Object.hasOwn(small,'fullScreenshot'));
 cases.push({name:'uncapped-viewport-does-not-duplicate-image'});
 // The complete native function also preserves full physical pixels at2xDPR.
 // This is a trusted actual-CDP measurement, separate from the current1x worker.
 await attached.page.setViewportSize({width:1280,height:800});
 ownedCdp=await attached.page.context().newCDPSession(attached.page);
 await ownedCdp.send('Emulation.setDeviceMetricsOverride',{width:1280,height:800,deviceScaleFactor:2,mobile:false});
 const highDpi=decode(await nativePage.snapshot({page:attached.page,cdp:ownedCdp,renderScale:2,fullScreenshot:true,consoleEntries:[],networkEntries:[],actionTimeline:[]}));
 assert.deepEqual(dimensions(highDpi.screenshot),{width:1280,height:800});
 assert.deepEqual(dimensions(highDpi.fullScreenshot),{width:2560,height:1600});
 cases.push({name:'whole-native-function-at-actual2x-DPR',capped:dimensions(highDpi.screenshot),full:dimensions(highDpi.fullScreenshot)});
 result={accepted:true,actualChromium:true,actualScopedPage:true,wholeNativePage:true,nativeRevision:'6417151c093e54dbba0e6c824c69d03983793b9a',cases,qualification:'Physical Page capture and candidate typed worker forwarding only. Native diagnostics are explicit empty fixture inputs; no Status absence, full router, MCP save/image projection or public selector acceptance.'};
}catch(error){failure=error;}
finally{
 if(ownedCdp)await ownedCdp.detach().catch(()=>{});
 if(attached)await attached.close().catch(()=>{});
 if(relay)await relay.close().catch(()=>{});
 upstream?.close();
 if(child.exitCode===null&&child.signalCode===null){
  child.kill('SIGTERM');
  const ended=await Promise.race([childTerminal,new Promise(resolve=>setTimeout(()=>resolve(null),5000))]);
  if(ended===null){child.kill('SIGKILL');failure??=Error('Private Chromium required hard cleanup');}
 }
 const terminal=await childTerminal;
 const refusals=fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length;
 assert(artifacts.healthy);assert.equal(refusals,0);
 assert(artifacts.receipts.every(row=>row.executedRecursiveRemoval===false));
 if(result){result.chromiumTerminal=terminal;result.artifactReceipts=artifacts.receipts;result.guardRefusals=refusals;}
}
if(failure){console.error('FULL_CAPTURE_FAIL',failure.stack);process.exitCode=1;}
else console.log('FULL_CAPTURE_RESULT '+JSON.stringify(result));
