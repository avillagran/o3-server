// Explicit private fixture producer. The original entry, native Page,
// Playwright and underlying O3 client bytes are retained without modification.
import fs from 'node:fs';import assert from 'node:assert/strict';import {createHash} from 'node:crypto';
import {OwnedCdpClient as OriginalClient} from '../../runtime/preview/owned-cdp-client-product.mjs';
export class OwnedCdpClient {
 static async open(options){
  const client=await OriginalClient.open(options),send=client.send.bind(client);
  let injected=false,sequence=0,outstanding=0,maximum=0,fixtureMaximum=0,tracing=false,started=0,scope,remoteTermination,releaseTermination,terminationDelivered=false,diagnosticHealthy=true,ledgerBytes=0;
  const records=[],remotes=[],browserOrder=[];
  const ledger=fs.openSync(process.env.HOME+'/capacity-events.jsonl',fs.constants.O_WRONLY|fs.constants.O_CREAT|fs.constants.O_EXCL|fs.constants.O_NOFOLLOW,0o600);
  process.once('exit',()=>{try{fs.closeSync(ledger);}catch{}});
  function record(value){
   records.push(value);const bytes=Buffer.from(JSON.stringify(value)+'\n');
   if(!diagnosticHealthy)return;
   try{if(ledgerBytes+bytes.length>65536)throw Error();let offset=0;while(offset<bytes.length){const count=fs.writeSync(ledger,bytes,offset,bytes.length-offset);if(count<=0)throw Error();offset+=count;}ledgerBytes+=bytes.length;}
   catch{diagnosticHealthy=false;try{process.stderr.write('PRIVATE_CAPACITY_DIAGNOSTICS_INVALID\n');}catch{}}
  }
  const marker='O3_PRIVATE_CAPACITY_BROWSER_STARTED';
  const expressions=Array.from({length:8},(_,index)=>`console.debug(${JSON.stringify(marker)},${index});new Promise(resolve=>(globalThis.__o3PrivateCapacityGates??=[]).push(resolve))`);
  const reference=session=>createHash('sha256').update(session??'root').digest('hex');
  function base(method,params,session,receipt,operation,phase){
   const id=++sequence;const cdp=!method.startsWith('O3Owner.');if(cdp)outstanding++;maximum=Math.max(maximum,outstanding);if(tracing)fixtureMaximum=Math.max(fixtureMaximum,outstanding);
   record({event:'gateway-dispatched',sequence:id,method,phase,sessionHash:reference(session),outstanding});
   const remote=send(method,params,session,receipt,operation).finally(()=>{if(cdp)outstanding--;record({event:'gateway-settled',sequence:id,method,phase,outstanding});});
   if(phase==='fixture-background'||phase==='fixture-native-evaluate')remotes.push(remote.catch(()=>{}));
   return remote;
  }
  client.on('event',packet=>{
   if(scope&&packet.sessionId===scope.session&&packet.method==='Runtime.consoleAPICalled'&&packet.params?.args?.[0]?.value===marker){started++;const index=packet.params.args[1]?.value;browserOrder.push(index);record({event:'browser-evaluation-started',index,count:started,sessionHash:reference(scope.session)});}
  });
  const until=async predicate=>{const deadline=performance.now()+2000;while(!predicate()){assert(performance.now()<deadline,'Expected actual browser evaluation event');await new Promise(resolve=>setTimeout(resolve,5));}};
  client.send=function(method,params={},session,receipt,operation){
   if(!injected&&method==='Runtime.evaluate'&&params.expression==="'native scoped result'"&&operation==='preview_evaluate'){
    injected=true;scope={session,receipt,operation};assert(session);assert(/^[a-f0-9]{48}$/.test(receipt));
    return (async()=>{
     // Preserve the actual requested operation result. The additional producer
     // is an explicitly separate fixture phase, not a replacement response.
     const requested=await base(method,params,session,receipt,operation,'original-request');
     await base('Runtime.enable',{},session,receipt,operation,'fixture-setup');
     assert.equal(outstanding,0);tracing=true;
     for(let index=0;index<7;index++)void client.send('Runtime.evaluate',{expression:expressions[index],awaitPromise:true,returnByValue:true},session,receipt,operation).catch(()=>{});
     await until(()=>started===7);
     const {evaluate}=await import('/native/apps/server/src/preview/ServerBrowserPage.ts');
     const {SessionControl}=await import('/native/apps/server/src/preview/SessionControl.ts');
     const control=new SessionControl('private-capacity-agent');
     await assert.rejects(control.agent('private-capacity-agent',()=>evaluate({send:(name,input)=>client.send(name,input,session,receipt,operation)},{expression:expressions[7],awaitPromise:true,returnByValue:true},100)),error=>error.tag==='PreviewAutomationTimeoutError');
     assert.equal(started,8);assert.deepEqual(browserOrder,[0,1,2,3,4,5,6,7]);assert(remoteTermination);
     const termination=records.find(record=>record.method==='Runtime.terminateExecution'&&record.event==='gateway-dispatched');
     assert.equal(termination.outstanding,9);
     await control.agent('private-capacity-agent',async()=>assert.equal(terminationDelivered,false));
     await control.close();
     // Fixture cleanup waits for the real remote acknowledgment separately
     // from the original native tail. It then resolves every owned deferred
     // evaluation, retaining truthful counts for all still-outstanding calls.
     await remoteTermination;
     await base('Runtime.evaluate',{expression:'globalThis.__o3PrivateCapacityGates.splice(0).forEach(resolve=>resolve(null));true',returnByValue:true},session,receipt,operation,'fixture-cleanup');
     await Promise.all(remotes);assert.equal(outstanding,0);tracing=false;
     return requested;
    })();
   }
   if(scope&&method==='Runtime.evaluate'&&expressions.includes(params.expression)){
    assert.equal(session,scope.session);assert.equal(receipt,scope.receipt);assert.equal(operation,scope.operation);
    return base(method,params,session,receipt,operation,started<7?'fixture-background':'fixture-native-evaluate');
   }
   if(scope&&method==='Runtime.terminateExecution'){
    assert.equal(session,scope.session);assert.equal(receipt,scope.receipt);assert.equal(operation,scope.operation);
    remoteTermination=base(method,params,session,receipt,operation,'native-timeout-termination');
    // Hold only the JavaScript delivery after the underlying real wire result.
    // Remote wire completion remains counted independently and is never hidden.
    return remoteTermination.then(value=>new Promise(resolve=>{releaseTermination=()=>{terminationDelivered=true;resolve(value);};}));
   }
   const result=base(method,params,session,receipt,operation,method==='O3Owner.endScope'?'scope-marker':'platform-or-original');
   if(scope&&method==='O3Owner.endScope'&&receipt===scope.receipt){
    assert.equal(operation,scope.operation);
    return result.then(value=>{
     assert.equal(value.scopeEnded,true);assert.equal(terminationDelivered,false);assert(releaseTermination);
     record({event:'scope-marker-ack',nativeTerminationDeliveryStillPending:true,outstanding});
     releaseTermination();
     assert(diagnosticHealthy);const proof={complete:true,fixtureBackgroundProducers:7,actualBrowserEvaluationsStarted:started,terminationWireOutstanding:9,maximumRealGatewayOutstanding:maximum,fixtureMaximumGatewayOutstanding:fixtureMaximum,browserOrder,diagnosticHealthy,wireOutstandingAtMarkerAck:outstanding,originalReceiptAndSessionPreserved:true,originalNativeTailDidNotWaitForTerminationDelivery:true,records,qualification:'Seven explicit fixture CDP producers plus one whole-native evaluation. JavaScript termination delivery is deliberately held after its real wire result; underlying wire counters remain independent. This is not nine MCP calls or public toolkit acceptance.'};
     const fd=fs.openSync(process.env.HOME+'/capacity-proof.json',fs.constants.O_WRONLY|fs.constants.O_CREAT|fs.constants.O_EXCL|fs.constants.O_NOFOLLOW,0o600);
     try{fs.writeFileSync(fd,JSON.stringify(proof,null,2)+'\n');}finally{fs.closeSync(fd);}
     return value;
    });
   }
   return result;
  };
  return client;
 }
}
