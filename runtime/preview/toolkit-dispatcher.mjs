// Private continuation adapter. Invocation and callback receipts originate only
// in the Rust owner; model parameters never select this protocol's authority.
import {createNativePreviewToolkit} from './native-toolkit.mjs';
import {pathToFileURL} from 'node:url';
import path from 'node:path';
import {captureSourceDeadline} from './source-deadline.mjs';
export async function createToolkitDispatcher({client,sourceRoot,nativeRoot,effectRoot,environmentId,attachmentsDir,supportedOperations}){
 const callers=new WeakMap(),callbacks=new WeakMap(),endStarted=new WeakSet(),running=new Set(),invocations=new Set();let closing=false,unknown=false;
 const service=async(method,caller,input)=>{
  const receipt=callers.get(caller);if(!receipt)throw Error('Owned invocation unavailable');
  const reply=await client.service(method,receipt,input);
  if(!reply.ok)throw Error('Owned continuation refused');return reply;
 };
 let nativePage;
 const platform={environmentId,attachmentsDir,supportedOperations,
  captureCaller:()=>{throw Error('Explicit owned invocation required');},
  reserveCallback:async(caller,tool,request,metadata)=>{
   if(callbacks.has(request))throw Error('Duplicate owned callback');
   const reply=await service('reserveCallback',caller,{tool,request,updateCurrentTab:metadata.updateCurrentTab});
   if(!reply.callbackReceipt)throw Error('Owned callback receipt unavailable');
   const completion=Promise.withResolvers();running.add(completion.promise);
   callbacks.set(request,{receipt:reply.callbackReceipt,phase:'reserved',completion});
  },
  abandonCallback:async(caller,request)=>{
   const callback=callbacks.get(request);if(!callback||callback.phase!=='reserved')throw Error('Owned callback cannot be abandoned');
   try{await service('abandonCallback',caller,{callbackReceipt:callback.receipt});callbacks.delete(request);}catch(error){unknown=true;throw error;}finally{callback.completion.resolve();running.delete(callback.completion.promise);}
  },
  invoke:async(caller,tool,request,metadata)=>{
   const callback=callbacks.get(request);if(!callback||callback.phase!=='reserved')throw Error('Unreserved owned callback');
   callback.phase='running';
   try{
    const sourceDeadline=captureSourceDeadline(metadata.deadlineMs);
    const reply=await service('runCallback',caller,{callbackReceipt:callback.receipt,deadlineMs:metadata.deadlineMs,sourceDeadline});
    const result=reply.payload;
    if(!result||typeof result.ok!=='boolean')throw Error('Owned callback result unavailable');
    if(!result.ok){const error=result.error;if(!error||typeof error.tag!=='string'||typeof error.message!=='string')throw Error('Owned callback error unavailable');throw new nativePage.ServerBrowserOperationError(error);}
    return result.value;
   }finally{callback.phase='responded';}
  },
  markUsed:async(caller,request)=>{
   const callback=callbacks.get(request);if(!callback||callback.phase!=='responded')throw Error('Owned callback finalizer unavailable');
   try{await service('markUsed',caller,{callbackReceipt:callback.receipt});callbacks.delete(request);}
   catch(error){unknown=true;throw error;}finally{callback.completion.resolve();running.delete(callback.completion.promise);}
  },
  endInvocation:async caller=>{
   if(endStarted.has(caller)){unknown=true;throw Error('Duplicate owned source end');}
   // Never replay a source end after an ambiguous transport result.
   endStarted.add(caller);
   try{return await service('endInvocation',caller,{});}catch(error){unknown=true;throw error;}
  },
 };
 const toolkit=await createNativePreviewToolkit({sourceRoot,nativeRoot,effectRoot,platform});
 // The factory already verified this complete original source module and the
 // canonical Effect import identity before any callback can reach this adapter.
 nativePage=await import(pathToFileURL(path.join(nativeRoot,'apps/server/src/preview/ServerBrowserPage.ts')).href);
 return {
  async invoke(receipt,tool,input,{signal}={}){
   if(closing||typeof receipt!=='string'||!/^[a-f0-9]{96}$/.test(receipt))throw Error('Owned invocation unavailable');
   const caller={};callers.set(caller,receipt);
   const completion=Promise.withResolvers();invocations.add(completion.promise);
   try{
    const scope=await service('callerScope',caller,{});caller.scope=scope.payload;
    caller.getThreadProjection=async threadId=>(await service('projection',caller,{threadId})).payload;
    return await toolkit.invoke(tool,input,{signal,caller});
   }finally{
    // Publication/registration can fail before the original invocation's
    // finally exists. Its already registered Rust receipt still needs an end.
    try{if(!endStarted.has(caller))await platform.endInvocation(caller);}
    finally{completion.resolve();invocations.delete(completion.promise);}
   }
  },
  async close(){closing=true;await Promise.allSettled([...invocations]);await Promise.allSettled([...running]);await toolkit.disposeBroker();return {known:!unknown};},
 };
}
