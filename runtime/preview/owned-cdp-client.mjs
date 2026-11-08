// Explicit upstream for the sealed Rust gateway. No ambient CDP discovery.
import {EventEmitter} from 'node:events';
export class OwnedCdpClient extends EventEmitter {
 #socket;#pending=new Map();#sequence=0;#closed=false;#ready;
 constructor({WebSocket,endpoint}){
  super();const url=new URL(endpoint);
  if(url.protocol!=='ws:'||url.hostname!=='127.0.0.1'||!url.port||url.username||url.password||url.search||url.hash||url.pathname.length<32)throw Error('Invalid owned upstream');
  this.#socket=new WebSocket(endpoint,{maxPayload:4*1024*1024,perMessageDeflate:false});
  this.#ready=new Promise((resolve,reject)=>{
   this.#socket.once('open',resolve);
   this.#socket.once('error',()=>reject(Error('Owned upstream unavailable')));
   this.#socket.once('close',()=>reject(Error('Owned upstream closed')));
  });
  this.#socket.on('message',data=>{try{
   const packet=JSON.parse(data.toString());if(!packet||Array.isArray(packet))throw Error();
   if(Object.hasOwn(packet,'id')){
    const call=this.#pending.get(packet.id);if(!call||packet.sessionId!==call.session)throw Error();
    this.#pending.delete(packet.id);
    if(Object.hasOwn(packet,'error'))call.reject(Error(typeof packet.error?.message==='string'?packet.error.message:'Owned command refused'));
    else if(Object.hasOwn(packet,'result'))call.resolve(packet.result);
    else throw Error();
   }else if(typeof packet.method==='string')this.emit('event',packet);
   else throw Error();
  }catch{this.close();}});
  this.#socket.on('error',()=>this.#finish());this.#socket.on('close',()=>this.#finish());
 }
 static async open(options){const client=new OwnedCdpClient(options);try{await client.#ready;return client;}catch(error){client.close();throw error;}}
 #finish(){if(this.#closed)return;this.#closed=true;for(const call of this.#pending.values())call.reject(Error('Owned upstream closed'));this.#pending.clear();this.emit('closed');}
 send(method,params={},sessionId,receipt,operation){
  if(this.#closed||this.#socket.readyState!==1||typeof receipt!=='string'||!/^[a-f0-9]{48}$/.test(receipt))return Promise.reject(Error('Owned upstream admission unavailable'));
  const id=++this.#sequence;if(!Number.isSafeInteger(id))return Promise.reject(Error('Owned upstream sequence exhausted'));
  let resolve,reject;const result=new Promise((yes,no)=>{resolve=yes;reject=no;});
  this.#pending.set(id,{resolve,reject,session:sessionId});
  try{this.#socket.send(JSON.stringify({id,method,params,...(sessionId===undefined?{}:{sessionId}),__o3OwnerReceipt:receipt,...(operation===undefined?{}:{__o3OwnerOperation:operation})}),error=>{if(error){this.#pending.delete(id);reject(Error('Owned upstream write failed'));this.close();}});}
  catch{this.#pending.delete(id);reject(Error('Owned upstream write failed'));this.close();}
  return result;
 }
 async endScope(receipt,operation){
  const result=await this.send('O3Owner.endScope',{},undefined,receipt,operation);
  if(result?.scopeEnded!==true)throw Error('Owned scope admission fence unavailable');
 }
 close(){this.#finish();this.#socket.terminate();}
}
