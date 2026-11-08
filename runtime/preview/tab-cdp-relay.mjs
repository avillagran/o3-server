// A transport grant is issued by the owner, never parsed from an MCP argument.
import http from 'node:http';
import {randomBytes} from 'node:crypto';
const PAGE_DOMAINS=new Set(['Accessibility','Animation','DOM','DOMSnapshot','Emulation','Input','Inspector','LayerTree','Log','Network','Page','Performance','Runtime']);
const FORBIDDEN_PAGE_METHODS=new Set(['Network.getAllCookies','Network.getCookies','Network.setCookie','Network.setCookies','Network.deleteCookies','Network.clearBrowserCookies','Network.clearBrowserCache','Page.crash','Page.close','Page.setDownloadBehavior','DOM.setFileInputFiles']);
export class TabCdpRelay {
 #receiptRequired;#cleanupReceipt;#attachments=new Set();#parentAttachments=new Map();#retirements=new Set();#detaches=new Map();#lateCleanup=new Map();#retiredSessions=new Map();#cleanupTimeout;#cleanupState='open';#cleanupReceipts=[];#upstream;#target;#alive;#sessions=new Map();#targetInfo;#server;#ws;#path;#client;#connected=false;#closed=false;#event;#denied=0;#windows=new Set();#virtualSequence=0;#closeTask;#namespace=randomBytes(8).toString('hex');
 constructor({upstream,targetId,admitted,WebSocketServer,cleanupTimeoutMs=15000,requireReceipt=false,cleanupReceipt}){
  if(!targetId||typeof admitted!=='function')throw Error('Owner target grant required');
  if(!Number.isSafeInteger(cleanupTimeoutMs)||cleanupTimeoutMs<=0||cleanupTimeoutMs>30000)throw Error('Invalid owner cleanup deadline');
  if(requireReceipt&&(typeof cleanupReceipt!=='string'||!/^[a-f0-9]{48}$/.test(cleanupReceipt)))throw Error('Owner cleanup receipt required');
  this.#receiptRequired=requireReceipt;this.#cleanupReceipt=cleanupReceipt;this.#cleanupTimeout=cleanupTimeoutMs;
  this.#upstream=upstream;this.#target=targetId;this.#alive=admitted;
  this.#path='/'+randomBytes(32).toString('base64url');this.#server=http.createServer((_q,r)=>{r.writeHead(404);r.end();});
  this.#ws=new WebSocketServer({noServer:true,maxPayload:256*1024*1024,perMessageDeflate:false});
  this.#server.on('upgrade',(request,socket,head)=>{
   if(this.#closed||!this.#alive()||this.#connected||request.url!==this.#path||request.headers.origin||!['127.0.0.1','::ffff:127.0.0.1'].includes(socket.remoteAddress)){socket.end('HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n');return;}
   this.#connected=true;this.#ws.handleUpgrade(request,socket,head,client=>{this.#client=client;client.on('message',data=>{void this.#receive(data);});client.on('close',()=>{this.close().catch(()=>{ /* Rejected owner receipt remains observable through close/cleanup. */ });});});
  });
  this.#event=packet=>this.#forward(packet);upstream.on('event',this.#event);
 }
 get denied(){return this.#denied;}
 get cleanup(){return {state:this.#cleanupState,pendingAttachments:this.#attachments.size,receipts:structuredClone(this.#cleanupReceipts)};}
 async listen(){await new Promise((resolve,reject)=>{this.#server.once('error',reject);this.#server.listen(0,'127.0.0.1',resolve);});return `ws://127.0.0.1:${this.#server.address().port}${this.#path}`;}
 #assert(){if(this.#closed||!this.#alive())throw Error('Browser grant revoked');}
 #send(packet){if(this.#client?.readyState===1)this.#client.send(JSON.stringify(packet));}
 async #info(receipt,operation){if(!this.#targetInfo){const r=await this.#upstream.send('Target.getTargetInfo',{targetId:this.#target},undefined,receipt,operation);this.#assert();if(r.targetInfo?.targetId!==this.#target)throw Error('Owner target changed');this.#targetInfo=r.targetInfo;}return this.#targetInfo;}
 #record(id){return this.#sessions.get(id)??this.#retiredSessions.get(id);}
 #lineageParent(record){return record.parent??(record.eventParent===record.browserParent?undefined:record.eventParent);}
 #closing(record){
  const seen=new Set();
  while(record){
   if(record.closing)return true;
   if(record.browserParent){const owner=this.#sessions.get(record.browserParent);if(!owner?.virtual||owner.closing)return true;}
   const parent=this.#lineageParent(record);if(!parent)return false;
   if(seen.has(parent))return true;seen.add(parent);
   record=this.#record(parent);if(!record)return true;
  }
  return false;
 }
 #knownTarget(id){return id===this.#target||[...this.#sessions.values()].some(v=>v.targetId===id&&!this.#closing(v));}
 #assertBrowserParent(id){if(id){const parent=this.#sessions.get(id);if(!parent?.virtual||parent.closing)throw Error('Browser parent unavailable');}}
 #assertAttachParent(browserParent,eventParent){
  this.#assertBrowserParent(browserParent);
  if(eventParent){const parent=this.#sessions.get(eventParent);if(!parent||this.#closing(parent))throw Error('Attachment parent unavailable');}
 }
 #retainingAncestor(record){
  if(this.#closed||this.#sessions.get(record.browserParent)?.closing)return true;
  const seen=new Set();let parent=this.#lineageParent(record);
  while(parent&&!seen.has(parent)){seen.add(parent);const current=this.#record(parent);if(!current)return false;if(current.closing)return true;parent=this.#lineageParent(current);}
  return false;
 }
 #forgetRetiredTree(id){
  const owned=new Set([id]);let added=true;
  while(added){added=false;for(const [child,record]of this.#retiredSessions){if(owned.has(this.#lineageParent(record))&&!owned.has(child)){owned.add(child);added=true;}}}
  for(const session of owned)this.#retiredSessions.delete(session);
 }
 #terminal(id){
  const record=this.#sessions.get(id);if(!record)return;
  if(this.#closed||this.#closing(record))this.#retiredSessions.set(id,record);
  this.#sessions.delete(id);
 }
 #children(id){return [...this.#sessions].filter(([,record])=>this.#lineageParent(record)===id).map(([child])=>child);}
 async #drainChildren(id,record){
  for(;;){
   const pending=new Set([...(record.pendingCleanup??[]),...this.#children(id).map(child=>this.#detach(child))]);
   if(!pending.size)return;
   if((await Promise.allSettled(pending)).some(result=>result.status==='rejected'))throw Error('Owned child cleanup outcome unknown');
  }
 }
 #detach(sessionId){
  if(this.#detaches.has(sessionId))return this.#detaches.get(sessionId);
  const record=this.#sessions.get(sessionId);if(!record||record.virtual)throw Error('Session outside grant');
  record.closing=true;
  const work=Promise.resolve().then(async()=>{
   await Promise.allSettled([...(this.#parentAttachments.get(sessionId)??[])]);
   await this.#drainChildren(sessionId,record);
   const result=await this.#upstream.send('Target.detachFromTarget',{sessionId},record.parent,this.#cleanupReceipt);
   this.#terminal(sessionId);
   // Child events may arrive while the parent acknowledgment is pending.
   // Each child needs its own notification/acknowledgment, never an inferred one.
   await this.#drainChildren(sessionId,record);
   this.#cleanupReceipts.push({kind:'detached-owned-session'});
   if(!this.#retainingAncestor(record))this.#forgetRetiredTree(sessionId);
   return result;
  });
  this.#detaches.set(sessionId,work);
  work.then(()=>this.#detaches.delete(sessionId),()=>{this.#detaches.delete(sessionId);this.#cleanupState='unknown';});
  return work;
 }
 #cleanupLateChild(parent,id){
  const record=this.#sessions.get(id),work=this.#detach(id);
  parent.pendingCleanup??=new Set();parent.pendingCleanup.add(work);
  this.#lateCleanup.set(work,record.browserParent);
  const settled=()=>{parent.pendingCleanup.delete(work);this.#lateCleanup.delete(work);};
  work.then(settled,()=>{settled();this.#cleanupState='unknown';});
 }
 #attach(targetId,receipt,operation,browserParent,eventParent){
  // Fence before starting any upstream write, including after #info awaits.
  this.#assert();this.#assertAttachParent(browserParent,eventParent);
  if(!this.#knownTarget(targetId))throw Error('Target outside grant');
  const work=Promise.resolve().then(async()=>{
   this.#assert();this.#assertAttachParent(browserParent,eventParent);
   let result;
   try{result=await this.#upstream.send('Target.attachToTarget',{targetId,flatten:true},undefined,receipt,operation);}
   catch(error){this.#cleanupReceipts.push({kind:'attachment-outcome-unknown'});this.#cleanupState='unknown';throw error;}
   if(typeof result.sessionId!=='string'||!result.sessionId){this.#cleanupState='unknown';throw Error('Attachment outcome unknown');}
   // A virtual browser parent has no upstream session. Preserve that local
   // event route separately from the physical CDP parent used for cleanup.
   this.#sessions.set(result.sessionId,{targetId,browserParent,eventParent});
   const parent=browserParent?this.#sessions.get(browserParent):undefined;
   const caller=eventParent?this.#sessions.get(eventParent):undefined;
   if(this.#closed||!this.#alive()||(browserParent&&(!parent?.virtual||parent.closing))||(eventParent&&(!caller||this.#closing(caller)))){
    try{await this.#detach(result.sessionId);}catch(error){this.#cleanupState='unknown';throw error;}
    throw Error('Browser grant revoked');
   }
   return result;
  });
  this.#attachments.add(work);
  const parents=new Set([browserParent,eventParent].filter(Boolean));
  for(const id of parents){let pending=this.#parentAttachments.get(id);if(!pending){pending=new Set();this.#parentAttachments.set(id,pending);}pending.add(work);}
  const settled=()=>{
   this.#attachments.delete(work);
   for(const id of parents){const pending=this.#parentAttachments.get(id);pending?.delete(work);if(!pending?.size)this.#parentAttachments.delete(id);}
  };
  work.then(settled,settled);
  return work;
 }
 #depth(id){let count=0,current=this.#record(id);while(current&&this.#lineageParent(current)){if(++count>this.#sessions.size+this.#retiredSessions.size)throw Error('Invalid owned lineage');current=this.#record(this.#lineageParent(current));}return count;}
 #retireBrowserParent(id){
  const parent=this.#sessions.get(id);
  if(!parent?.virtual||parent.closing)throw Error('Browser parent unavailable');
  parent.closing=true;
  const work=(async()=>{
   // In-flight attachments observe closing and reap their actual reply before
   // this local parent can be reported retired. Other parents keep running.
   await Promise.allSettled([...(this.#parentAttachments.get(id)??[])]);
   for(;;){
    const pending=new Set([
     ...[...this.#lateCleanup].filter(([,owner])=>owner===id).map(([work])=>work),
     ...[...this.#detaches].filter(([session])=>this.#record(session)?.browserParent===id).map(([,work])=>work),
    ]);
    if((await Promise.allSettled(pending)).some(result=>result.status==='rejected')||this.#cleanupState==='unknown')throw Error('Owned parent cleanup outcome unknown');
    const owned=[...this.#sessions].filter(([,value])=>value.browserParent===id).map(([session])=>session).sort((a,b)=>this.#depth(b)-this.#depth(a));
    if(!owned.length&&!pending.size)break;
    for(const session of owned){if(this.#sessions.has(session))await this.#detach(session);}
   }
   this.#sessions.delete(id);
   for(const [session,value]of this.#retiredSessions){if(value.browserParent===id)this.#retiredSessions.delete(session);}
  })();
  this.#retirements.add(work);
  work.then(()=>this.#retirements.delete(work),()=>{this.#retirements.delete(work);this.#cleanupState='unknown';});
  return work;
 }
 async #command(method,params,sessionId,receipt,operation){
  this.#assert();
  if(sessionId&&!this.#sessions.has(sessionId))throw Error('Session outside grant');
  if(sessionId&&this.#closing(this.#sessions.get(sessionId)))throw Error('Browser parent or ancestor closing');
  if(method==='Target.attachToBrowserTarget'&&!sessionId){const id=`owned-browser-${this.#namespace}-${++this.#virtualSequence}`;this.#sessions.set(id,{targetId:this.#target,virtual:true});return {sessionId:id};}
  const eventParent=sessionId;
  const virtual=sessionId&&this.#sessions.get(sessionId).virtual;
  const browserParent=virtual?sessionId:this.#sessions.get(sessionId)?.browserParent;
  if(virtual)sessionId=undefined;
  if(method==='Browser.getVersion'&&!sessionId)return this.#upstream.send(method,{},undefined,receipt,operation);
  if(method==='Target.getTargets'&&!sessionId)return {targetInfos:[await this.#info(receipt,operation)]};
  if(method==='Target.getTargetInfo'){
   const targetId=params.targetId??(sessionId?this.#sessions.get(sessionId).targetId:this.#target);
   if(!this.#knownTarget(targetId))throw Error('Target outside grant');
   return this.#upstream.send(method,{targetId},undefined,receipt,operation);
  }
  if(method==='Target.setAutoAttach'&&!sessionId){
   if(params.autoAttach!==true||params.flatten!==true)throw Error('Unsupported root attachment');
   if(![...this.#sessions.values()].some(v=>v.targetId===this.#target)){
    const info=await this.#info(receipt,operation);const attached=await this.#attach(this.#target,receipt,operation,browserParent,eventParent);
    this.#send({method:'Target.attachedToTarget',params:{sessionId:attached.sessionId,targetInfo:info,waitingForDebugger:false},...(eventParent?{sessionId:eventParent}:{})});
   }
   return {};
  }
  if(method==='Target.attachToTarget'){if(params.flatten!==true)throw Error('Unflattened attachment refused');return this.#attach(params.targetId,receipt,operation,browserParent,eventParent);}
  if(method==='Target.detachFromTarget'){
   if(!this.#sessions.has(params.sessionId))throw Error('Session outside grant');
   if(this.#sessions.get(params.sessionId).eventParent!==eventParent)throw Error('Session outside browser parent');
   if(this.#sessions.get(params.sessionId).virtual){await this.#retireBrowserParent(params.sessionId);return {};}
   return this.#detach(params.sessionId);
  }
  if(method==='Target.setAutoAttach'&&sessionId){
   if(params.flatten!==true)throw Error('Unflattened child attachment refused');
   return this.#upstream.send(method,params,sessionId,receipt,operation);
  }
  if(method==='Browser.getWindowForTarget'){
   if(params.targetId!==this.#target)throw Error('Target outside grant');
   const r=await this.#upstream.send(method,{targetId:this.#target},undefined,receipt,operation);this.#assert();this.#windows.add(r.windowId);return r;
  }
  if(method==='Browser.getWindowBounds'&&this.#windows.has(params.windowId))return this.#upstream.send(method,{windowId:params.windowId},undefined,receipt,operation);
  const domain=method.split('.')[0];
  if(!sessionId||!PAGE_DOMAINS.has(domain)||FORBIDDEN_PAGE_METHODS.has(method))throw Error('Method outside grant');
  return this.#upstream.send(method,params,sessionId,receipt,operation);
 }
 async #receive(data){
  let request;try{request=JSON.parse(data.toString());if(!Number.isSafeInteger(request.id)||typeof request.method!=='string')throw Error('Invalid CDP command');
   const operation=request.__o3OwnerOperation;if(operation!==undefined&&typeof operation!=='string')throw Error('Invalid owner operation');
   const receipt=request.__o3OwnerReceipt;if(this.#receiptRequired&&(typeof receipt!=='string'||!/^[a-f0-9]{48}$/.test(receipt)))throw Error('Owner receipt required');
   const result=await this.#command(request.method,request.params??{},request.sessionId,receipt,operation);this.#assert();this.#send({id:request.id,result,...(request.sessionId?{sessionId:request.sessionId}:{})});
  }catch{this.#denied++;if(request?.id!==undefined)this.#send({id:request.id,error:{code:-32000,message:'Command unavailable for this browser grant'},...(request.sessionId?{sessionId:request.sessionId}:{})});}
 }
 #forward(packet){
  const stopped=this.#closed||!this.#alive();
  // Browser-root events never enumerate other threads' targets. Child sessions
  // are accepted only through a previously admitted parent session's event.
  if(!packet.sessionId){
   if(packet.method==='Target.detachedFromTarget'&&this.#sessions.has(packet.params?.sessionId)){
    const record=this.#sessions.get(packet.params.sessionId),eventParent=record.eventParent??record.parent;
    this.#terminal(packet.params.sessionId);
    if(!stopped){if(!eventParent)this.#send(packet);else if(this.#record(eventParent))this.#send({...packet,sessionId:eventParent});}
   }
   return;
  }
  const parent=this.#record(packet.sessionId);
  if(!parent||parent.virtual)return;
  const retiring=stopped||this.#closing(parent);
  if(packet.method==='Target.attachedToTarget'){
   const p=packet.params;if(!p?.sessionId||!['iframe','worker'].includes(p.targetInfo?.type))return;
   this.#sessions.set(p.sessionId,{targetId:p.targetInfo.targetId,parent:packet.sessionId,eventParent:packet.sessionId,browserParent:parent.browserParent});
   if(retiring){this.#cleanupLateChild(parent,p.sessionId);return;}
  }
  if(packet.method==='Target.detachedFromTarget')this.#terminal(packet.params?.sessionId);
  if(!stopped&&(packet.method==='Target.detachedFromTarget'||!retiring))this.#send(packet);
 }
 close(){if(!this.#closeTask){this.#closed=true;this.#closeTask=Promise.resolve().then(()=>this.#release());}return this.#closeTask;}
 async #release(){
  const alreadyUnknown=this.#cleanupState==='unknown';this.#cleanupState=alreadyUnknown?'unknown':'draining';
  this.#client?.terminate();this.#ws.close();
  const listenerClosed=new Promise(resolve=>this.#server.close(resolve));
  const drain=(async()=>{
   // No new attach can enter after #closed. Late replies retain their own
   // detach receipt inside the tracked work, including after a deadline.
   let rejected=false;const attempted=new Set();
   for(;;){
    const pending=new Set([...this.#attachments,...this.#retirements,...this.#detaches.values(),...this.#lateCleanup.keys()]);
    // A revoked attach may reject after its independently acknowledged reap.
    // Actual uncertain attach/detach/retirement paths mark cleanup unknown.
    await Promise.allSettled(pending);
    const sessions=[...this.#sessions].filter(([id,value])=>!value.virtual&&!attempted.has(id)).map(([id])=>id).sort((a,b)=>this.#depth(b)-this.#depth(a));
    if(!sessions.length&&!this.#attachments.size&&!this.#retirements.size&&!this.#detaches.size&&!this.#lateCleanup.size)break;
    for(const sessionId of sessions){attempted.add(sessionId);if(this.#sessions.has(sessionId)){try{await this.#detach(sessionId);}catch{rejected=true;this.#cleanupState='unknown';}}}
   }
   if(rejected||this.#cleanupState==='unknown')throw Error('Owned cleanup outcome unknown');
   this.#sessions.clear();this.#retiredSessions.clear();this.#upstream.off('event',this.#event);await listenerClosed;
  })();
  let timer;
  try{
   await Promise.race([drain,new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('Owned cleanup deadline; outcome unknown')),this.#cleanupTimeout);})]);
   this.#cleanupState='complete';
  }catch(error){this.#cleanupState='unknown';throw error;}
  finally{clearTimeout(timer);}
 }
}
