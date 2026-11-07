// The owner supplies an already admitted tab-only endpoint and pinned imports.
// This foundation has no public MCP catalog or ambient endpoint discovery.
export async function attachPage({chromium,endpoint,nativePage,nativeContracts,Schema,admitted,artifactFs}){
 if(!artifactFs?.installed||!artifactFs.healthy)throw Error('Owned artifact boundary unavailable');
 if(!admitted())throw Error('Browser grant revoked');
 const browser=await chromium.connectOverCDP(endpoint,{noDefaults:true,timeout:15000});
 const pages=browser.contexts().flatMap(context=>context.pages());
 if(!admitted()||pages.length!==1){await browser.close();throw Error('Invalid owned page grant');}
 const page=pages[0],cdp=await page.context().newCDPSession(page);
 const allowed=new Set(['click','type','hover','select','drag','press','scroll','waitFor']);
 return {
  page,
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
    result=await nativePage.snapshot({page,cdp,renderScale:1,consoleEntries:[],networkEntries:[],actionTimeline:[]});
   }else throw Error('Operation not implemented by this foundation');
   if(!artifactFs.healthy)throw Error('Owned artifact boundary failed');
   if(!admitted())throw Error('Browser grant revoked');
   return result;
  },
  invalidateRefs(){nativePage.invalidateRefs(page);},
  async close(){await cdp.detach().catch(()=>{});await browser.close();}
 };
}
