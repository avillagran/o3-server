import {installBirthProducers} from './birth-producers.mjs';
// The owner supplies an already admitted tab-only endpoint and pinned imports.
// This foundation has no public MCP catalog or ambient endpoint discovery.
export async function attachPage({chromium,endpoint,nativePage,nativeContracts,Schema,admitted,artifactFs,targetId,canonicalBirth}){
 if(typeof targetId!=='string'||!targetId)throw Error('Owner target identity required');
 if(!artifactFs?.installed||!artifactFs.healthy)throw Error('Owned artifact boundary unavailable');
 if(!admitted())throw Error('Browser grant revoked');
 const browser=await chromium.connectOverCDP(endpoint,{noDefaults:true,timeout:15000});
 let cdp,birthProducers;try{
 const pages=browser.contexts().flatMap(context=>context.pages());
 if(!admitted()||pages.length!==1){await browser.close();throw Error('Invalid owned page grant');}
 const page=pages[0];cdp=await page.context().newCDPSession(page);
 const identity=await cdp.send('Target.getTargetInfo');if(identity.targetInfo?.targetId!==targetId)throw Error('Owner target identity mismatch');
 // Original ServerBrowser.ensureTab sets both Page defaults to its 15s
 // navigation budget; Playwright's own defaults are different.
 page.setDefaultTimeout(15000);page.setDefaultNavigationTimeout(15000);
 if(canonicalBirth!==undefined){
  if(!canonicalBirth||Object.keys(canonicalBirth).sort().join(',')!=='kind,targetId'||canonicalBirth.kind!=='new-local-target'||canonicalBirth.targetId!==targetId)throw Error('Invalid source birth installation');
  birthProducers=await installBirthProducers({page,cdp,targetId});
 }
 const allowed=new Set(['click','type','hover','select','drag','press','scroll','waitFor']);
 return {
  page,
  producerBindings:birthProducers?.receipt,
  async invoke(operation,input,ownerTimeoutMs=15000){
   if(!artifactFs.installed||!artifactFs.healthy)throw Error('Owned artifact boundary unavailable');
   if(!admitted())throw Error('Browser grant revoked');
   let result;
   if(allowed.has(operation)){
    const codec=operation==='waitFor'?nativeContracts.PreviewAutomationWaitForInput:nativeContracts.PreviewAutomationActInputs[operation];
    result=await nativePage[operation](page,Schema.decodeUnknownSync(codec)(input));
   }else if(operation==='evaluate'){
    const decoded=Schema.decodeUnknownSync(nativeContracts.PreviewAutomationEvaluateInput)(input);
    const timeout=Schema.decodeUnknownSync(nativeContracts.PreviewAutomationRequest.fields.timeoutMs)(ownerTimeoutMs);
    result=await nativePage.evaluate(cdp,decoded,timeout);
   }else if(operation==='snapshot'){
    const decoded=Schema.decodeUnknownSync(nativeContracts.PreviewAutomationSnapshotInput)(input??{});
    result=await nativePage.snapshot({page,cdp,renderScale:1,...decoded,consoleEntries:[],networkEntries:[],actionTimeline:[]});
   }else throw Error('Operation not implemented by this foundation');
   if(!artifactFs.healthy)throw Error('Owned artifact boundary failed');
   if(!admitted())throw Error('Browser grant revoked');
   return result;
  },
  invalidateRefs(){nativePage.invalidateRefs(page);},
  async close(){birthProducers?.close();await cdp.detach().catch(()=>{});await browser.close();}
 };
 }catch(error){birthProducers?.close();if(cdp)await cdp.detach().catch(()=>{});await browser.close().catch(()=>{});throw error;}
}
