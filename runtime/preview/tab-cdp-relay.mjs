// A transport grant is issued by the owner, never parsed from an MCP argument.
import http from 'node:http';
import {randomBytes} from 'node:crypto';
const PAGE_DOMAINS=new Set(['Accessibility','Animation','DOM','DOMSnapshot','Emulation','Input','Inspector','LayerTree','Log','Network','Page','Performance','Runtime']);
const FORBIDDEN_PAGE_METHODS=new Set(['Network.getAllCookies','Network.getCookies','Network.setCookie','Network.setCookies','Network.deleteCookies','Network.clearBrowserCookies','Network.clearBrowserCache','Page.crash','Page.close','Page.setDownloadBehavior','DOM.setFileInputFiles']);
export class TabCdpRelay {
 #upstream;#target;#alive;#sessions=new Map();#targetInfo;#server;#ws;#path;#client;#connected=false;#closed=false;#event;#denied=0;#windows=new Set();#virtualSequence=0;#closeTask;#namespace=randomBytes(8).toString('hex');
 constructor({upstream,targetId,admitted,WebSocketServer}){
  if(!targetId||typeof admitted!=='function')throw Error('Owner target grant required');
  this.#upstream=upstream;this.#target=targetId;this.#alive=admitted;
  this.#path='/'+randomBytes(32).toString('base64url');this.#server=http.createServer((_q,r)=>{r.writeHead(404);r.end();});
  this.#ws=new WebSocketServer({noServer:true,maxPayload:256*1024*1024,perMessageDeflate:false});
  this.#server.on('upgrade',(request,socket,head)=>{
   if(this.#closed||!this.#alive()||this.#connected||request.url!==this.#path||request.headers.origin||!['127.0.0.1','::ffff:127.0.0.1'].includes(socket.remoteAddress)){socket.end('HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n');return;}
   this.#connected=true;this.#ws.handleUpgrade(request,socket,head,client=>{this.#client=client;client.on('message',data=>{void this.#receive(data);});client.on('close',()=>{void this.close();});});
  });
  this.#event=packet=>this.#forward(packet);upstream.on('event',this.#event);
 }
 get denied(){return this.#denied;}
 async listen(){await new Promise((resolve,reject)=>{this.#server.once('error',reject);this.#server.listen(0,'127.0.0.1',resolve);});return `ws://127.0.0.1:${this.#server.address().port}${this.#path}`;}
 #assert(){if(this.#closed||!this.#alive())throw Error('Browser grant revoked');}
 #send(packet){if(this.#client?.readyState===1)this.#client.send(JSON.stringify(packet));}
 async #info(){if(!this.#targetInfo){const r=await this.#upstream.send('Target.getTargetInfo',{targetId:this.#target});this.#assert();if(r.targetInfo?.targetId!==this.#target)throw Error('Owner target changed');this.#targetInfo=r.targetInfo;}return this.#targetInfo;}
 #knownTarget(id){return id===this.#target||[...this.#sessions.values()].some(v=>v.targetId===id);}
 async #attach(targetId){if(!this.#knownTarget(targetId))throw Error('Target outside grant');const result=await this.#upstream.send('Target.attachToTarget',{targetId,flatten:true});this.#sessions.set(result.sessionId,{targetId});this.#assert();return result;}
 async #command(method,params,sessionId){
  this.#assert();
  if(sessionId&&!this.#sessions.has(sessionId))throw Error('Session outside grant');
  if(method==='Target.attachToBrowserTarget'&&!sessionId){const id=`owned-browser-${this.#namespace}-${++this.#virtualSequence}`;this.#sessions.set(id,{targetId:this.#target,virtual:true});return {sessionId:id};}
  if(sessionId&&this.#sessions.get(sessionId).virtual)sessionId=undefined;
  if(method==='Browser.getVersion'&&!sessionId)return this.#upstream.send(method,{});
  if(method==='Target.getTargets'&&!sessionId)return {targetInfos:[await this.#info()]};
  if(method==='Target.getTargetInfo'){
   const targetId=params.targetId??(sessionId?this.#sessions.get(sessionId).targetId:this.#target);
   if(!this.#knownTarget(targetId))throw Error('Target outside grant');
   return this.#upstream.send(method,{targetId});
  }
  if(method==='Target.setAutoAttach'&&!sessionId){
   if(params.autoAttach!==true||params.flatten!==true)throw Error('Unsupported root attachment');
   if(![...this.#sessions.values()].some(v=>v.targetId===this.#target)){
    const info=await this.#info();const attached=await this.#attach(this.#target);
    this.#send({method:'Target.attachedToTarget',params:{sessionId:attached.sessionId,targetInfo:info,waitingForDebugger:false}});
   }
   return {};
  }
  if(method==='Target.attachToTarget'){if(params.flatten!==true)throw Error('Unflattened attachment refused');return this.#attach(params.targetId);}
  if(method==='Target.detachFromTarget'){
   if(!this.#sessions.has(params.sessionId))throw Error('Session outside grant');
   if(this.#sessions.get(params.sessionId).virtual){this.#sessions.delete(params.sessionId);return {};}
   const r=await this.#upstream.send(method,{sessionId:params.sessionId});this.#sessions.delete(params.sessionId);return r;
  }
  if(method==='Target.setAutoAttach'&&sessionId){
   if(params.flatten!==true)throw Error('Unflattened child attachment refused');
   return this.#upstream.send(method,params,sessionId);
  }
  if(method==='Browser.getWindowForTarget'){
   if(params.targetId!==this.#target)throw Error('Target outside grant');
   const r=await this.#upstream.send(method,{targetId:this.#target});this.#assert();this.#windows.add(r.windowId);return r;
  }
  if(method==='Browser.getWindowBounds'&&this.#windows.has(params.windowId))return this.#upstream.send(method,{windowId:params.windowId});
  const domain=method.split('.')[0];
  if(!sessionId||!PAGE_DOMAINS.has(domain)||FORBIDDEN_PAGE_METHODS.has(method))throw Error('Method outside grant');
  return this.#upstream.send(method,params,sessionId);
 }
 async #receive(data){
  let request;try{request=JSON.parse(data.toString());if(!Number.isSafeInteger(request.id)||typeof request.method!=='string')throw Error('Invalid CDP command');
   const result=await this.#command(request.method,request.params??{},request.sessionId);this.#assert();this.#send({id:request.id,result,...(request.sessionId?{sessionId:request.sessionId}:{})});
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
 close(){if(!this.#closeTask)this.#closeTask=this.#release();return this.#closeTask;}
 async #release(){
  if(this.#closed)return;this.#closed=true;this.#upstream.off('event',this.#event);
  this.#client?.terminate();this.#ws.close();const listenerClosed=new Promise(resolve=>this.#server.close(resolve));
  const sessions=[...this.#sessions].filter(([,value])=>!value.virtual).map(([id])=>id);this.#sessions.clear();
  await Promise.all([listenerClosed,Promise.allSettled(sessions.map(sessionId=>this.#upstream.send('Target.detachFromTarget',{sessionId})))]);
 }
}
