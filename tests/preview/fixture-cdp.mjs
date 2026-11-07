// Private fixture transport. Production obtains the same abstraction from its owner.
import {EventEmitter} from 'node:events';
export class FixtureCdp extends EventEmitter {
 #socket;#sequence=0;#pending=new Map();calls=[];
 constructor(socket){super();this.#socket=socket;socket.on('message',data=>{const packet=JSON.parse(data.toString());if(packet.id){const pending=this.#pending.get(packet.id);if(pending){this.#pending.delete(packet.id);packet.error?pending.reject(Object.assign(Error(packet.error.message),packet.error)):pending.resolve(packet.result);}}else this.emit('event',packet);});socket.on('close',()=>{for(const p of this.#pending.values())p.reject(Error('Owned browser closed'));this.#pending.clear();});}
 send(method,params={},sessionId){this.calls.push({method,params,sessionId});const id=++this.#sequence;return new Promise((resolve,reject)=>{this.#pending.set(id,{resolve,reject});this.#socket.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}));});}
 close(){this.#socket.close();}
}
