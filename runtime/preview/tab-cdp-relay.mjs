// A transport grant is issued by the owner, never parsed from an MCP argument.
import http from 'node:http';
import {randomBytes} from 'node:crypto';
const PAGE_DOMAINS=new Set(['Accessibility','Animation','DOM','DOMSnapshot','Emulation','Input','Inspector','LayerTree','Log','Network','Page','Performance','Runtime']);
const FORBIDDEN_PAGE_METHODS=new Set(['Network.getAllCookies','Network.getCookies','Network.setCookie','Network.setCookies','Network.deleteCookies','Network.clearBrowserCookies','Network.clearBrowserCache','Page.crash','Page.close','Page.setDownloadBehavior','DOM.setFileInputFiles']);
export class TabCdpRelay {
 #receiptRequired;#cleanupReceipt;#attachments=new Set();#cleanupTimeout;#cleanupState='open';#cleanupReceipts=[];#upstream;#target;#alive;#sessions=new Map();#targetInfo;#server;#ws;#path;#client;#connected=false;#closed=false;#event;#denied=0;#windows=new Set();#virtualSequence=0;#closeTask;#namespace=randomBytes(8).toString('hex');
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
 async #info(receipt){if(!this.#targetInfo){const r=await this.#upstream.send('Target.getTargetInfo',{targetId:this.#target},undefined,receipt);this.#assert();if(r.targetInfo?.targetId!==this.#target)throw Error('Owner target changed');this.#targetInfo=r.targetInfo;}return this.#targetInfo;}
 #knownTarget(id){return id===this.#target||[...this.#sessions.values()].some(v=>v.targetId===id);}
 async #detach(sessionId){
  const result=await this.#upstream.send('Target.detachFromTarget',{sessionId},undefined,this.#cleanupReceipt);
  this.#sessions.delete(sessionId);this.#cleanupReceipts.push({kind:'detached-owned-session'});return result;
 }
 #attach(targetId,receipt){
  if(!this.#knownTarget(targetId))throw Error('Target outside grant');
  const work=(async()=>{
   let result;
   try{result=await this.#upstream.send('Target.attachToTarget',{targetId,flatten:true},undefined,receipt);}
   catch(error){this.#cleanupReceipts.push({kind:'attachment-outcome-unknown'});this.#cleanupState='unknown';throw error;}
   if(typeof result.sessionId!=='string'||!result.sessionId){this.#cleanupState='unknown';throw Error('Attachment outcome unknown');}
   this.#sessions.set(result.sessionId,{targetId});
   if(this.#closed||!this.#alive()){
    try{await this.#detach(result.sessionId);}catch(error){this.#cleanupState='unknown';throw error;}
    throw Error('Browser grant revoked');
   }
   return result;
  })();
  this.#attachments.add(work);
  work.then(()=>this.#attachments.delete(work),()=>this.#attachments.delete(work));
  return work;
 }
 async #command(method,params,sessionId,receipt){
  this.#assert();
  if(sessionId&&!this.#sessions.has(sessionId))throw Error('Session outside grant');
  if(method==='Target.attachToBrowserTarget'&&!sessionId){const id=`owned-browser-${this.#namespace}-${++this.#virtualSequence}`;this.#sessions.set(id,{targetId:this.#target,virtual:true});return {sessionId:id};}
  if(sessionId&&this.#sessions.get(sessionId).virtual)sessionId=undefined;
  if(method==='Browser.getVersion'&&!sessionId)return this.#upstream.send(method,{},undefined,receipt);
  if(method==='Target.getTargets'&&!sessionId)return {targetInfos:[await this.#info(receipt)]};
  if(method==='Target.getTargetInfo'){
   const targetId=params.targetId??(sessionId?this.#sessions.get(sessionId).targetId:this.#target);
   if(!this.#knownTarget(targetId))throw Error('Target outside grant');
   return this.#upstream.send(method,{targetId},undefined,receipt);
  }
  if(method==='Target.setAutoAttach'&&!sessionId){
   if(params.autoAttach!==true||params.flatten!==true)throw Error('Unsupported root attachment');
   if(![...this.#sessions.values()].some(v=>v.targetId===this.#target)){
    const info=await this.#info(receipt);const attached=await this.#attach(this.#target,receipt);
    this.#send({method:'Target.attachedToTarget',params:{sessionId:attached.sessionId,targetInfo:info,waitingForDebugger:false}});
   }
   return {};
  }
  if(method==='Target.attachToTarget'){if(params.flatten!==true)throw Error('Unflattened attachment refused');return this.#attach(params.targetId,receipt);}
  if(method==='Target.detachFromTarget'){
   if(!this.#sessions.has(params.sessionId))throw Error('Session outside grant');
   if(this.#sessions.get(params.sessionId).virtual){this.#sessions.delete(params.sessionId);return {};}
   return this.#detach(params.sessionId);
  }
  if(method==='Target.setAutoAttach'&&sessionId){
   if(params.flatten!==true)throw Error('Unflattened child attachment refused');
   return this.#upstream.send(method,params,sessionId,receipt);
  }
  if(method==='Browser.getWindowForTarget'){
   if(params.targetId!==this.#target)throw Error('Target outside grant');
   const r=await this.#upstream.send(method,{targetId:this.#target},undefined,receipt);this.#assert();this.#windows.add(r.windowId);return r;
  }
  if(method==='Browser.getWindowBounds'&&this.#windows.has(params.windowId))return this.#upstream.send(method,{windowId:params.windowId},undefined,receipt);
  const domain=method.split('.')[0];
  if(!sessionId||!PAGE_DOMAINS.has(domain)||FORBIDDEN_PAGE_METHODS.has(method))throw Error('Method outside grant');
  return this.#upstream.send(method,params,sessionId,receipt);
 }
 async #receive(data){
  let request;try{request=JSON.parse(data.toString());if(!Number.isSafeInteger(request.id)||typeof request.method!=='string')throw Error('Invalid CDP command');
   const receipt=request.__o3OwnerReceipt;if(this.#receiptRequired&&(typeof receipt!=='string'||!/^[a-f0-9]{48}$/.test(receipt)))throw Error('Owner receipt required');
   const result=await this.#command(request.method,request.params??{},request.sessionId,receipt);this.#assert();this.#send({id:request.id,result,...(request.sessionId?{sessionId:request.sessionId}:{})});
  }catch{this.#denied++;if(request?.id!==undefined)this.#send({id:request.id,error:{code:-32000,message:'Command unavailable for this browser grant'},...(request.sessionId?{sessionId:request.sessionId}:{})});}
 }
 #forward(packet){
  if(this.#closed||!this.#alive())return;
  // Browser-root events never enumerate other threads' targets. Child sessions
  // are accepted only through a previously admitted parent session's event.
  if(!packet.sessionId){
   if(packet.method==='Target.detachedFromTarget'&&this.#sessions.has(packet.params?.sessionId)){this.#sessions.delete(packet.params.sessionId);this.#send(packet);}
   return;
  }
  if(!this.#sessions.has(packet.sessionId)||this.#sessions.get(packet.sessionId).virtual)return;
  if(packet.method==='Target.attachedToTarget'){
   const p=packet.params;if(!p?.sessionId||!['iframe','worker'].includes(p.targetInfo?.type))return;
   this.#sessions.set(p.sessionId,{targetId:p.targetInfo.targetId,parent:packet.sessionId});
  }
  if(packet.method==='Target.detachedFromTarget')this.#sessions.delete(packet.params?.sessionId);
  this.#send(packet);
 }
 close(){if(!this.#closeTask){this.#closed=true;this.#closeTask=Promise.resolve().then(()=>this.#release());}return this.#closeTask;}
 async #release(){
  const alreadyUnknown=this.#cleanupState==='unknown';this.#cleanupState=alreadyUnknown?'unknown':'draining';
  this.#upstream.off('event',this.#event);this.#client?.terminate();this.#ws.close();
  const listenerClosed=new Promise(resolve=>this.#server.close(resolve));
  const drain=(async()=>{
   // No new attach can enter after #closed. Late replies retain their own
   // detach receipt inside the tracked work, including after a deadline.
   await Promise.allSettled([...this.#attachments]);
   const sessions=[...this.#sessions].filter(([,value])=>!value.virtual).map(([id])=>id);
   const outcomes=await Promise.allSettled(sessions.map(sessionId=>this.#detach(sessionId)));
   if(outcomes.some(r=>r.status==='rejected')||this.#cleanupState==='unknown')throw Error('Owned cleanup outcome unknown');
   this.#sessions.clear();await listenerClosed;
  })();
  let timer;
  try{
   await Promise.race([drain,new Promise((_,reject)=>{timer=setTimeout(()=>reject(Error('Owned cleanup deadline; outcome unknown')),this.#cleanupTimeout);})]);
   this.#cleanupState='complete';
  }catch(error){this.#cleanupState='unknown';throw error;}
  finally{clearTimeout(timer);}
 }
}
