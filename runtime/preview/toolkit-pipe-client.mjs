// Private continuations over the supervisor-owned framed pipes. No URL or
// browser capability is accepted here, and input cannot replace control fields.
const methods=new Set(['callerScope','projection','reserveCallback','runCallback','markUsed','abandonCallback','endInvocation']);
const receipt=value=>typeof value==='string'&&/^[a-f0-9]{96}$/.test(value);
const keys=(value,allowed)=>value&&typeof value==='object'&&!Array.isArray(value)&&Object.keys(value).every(key=>allowed.includes(key));
export class ToolkitPipeClient {
 #send;#nonce;#next=0;#pending=new Map();#closed=false;
 constructor({send,nonce}){
  if(typeof send!=='function'||typeof nonce!=='string'||!/^[a-f0-9]{48}$/.test(nonce))throw Error('Invalid owned toolkit transport');
  this.#send=send;this.#nonce=nonce;
 }
 service(method,caller,input){
  if(this.#closed||!methods.has(method)||!receipt(caller))return Promise.reject(Error('Owned toolkit service unavailable'));
  const id=++this.#next;if(!Number.isSafeInteger(id)){this.close();return Promise.reject(Error('Owned toolkit sequence exhausted'));}
  const result=Promise.withResolvers();this.#pending.set(id,result);
  try{
   Promise.resolve(this.#send({kind:'service',nonce:this.#nonce,id,caller,method,input})).catch(()=>this.close());
  }catch{this.close();}
  return result.promise;
 }
 accept(packet){
  if(this.#closed||!keys(packet,['kind','nonce','id','ok','payload','callbackReceipt'])||packet.kind!=='service-result'||packet.nonce!==this.#nonce||!Number.isSafeInteger(packet.id)||packet.id<=0||typeof packet.ok!=='boolean'||!Object.hasOwn(packet,'payload')||(Object.hasOwn(packet,'callbackReceipt')&&!receipt(packet.callbackReceipt))){this.close();throw Error('Owned toolkit response protocol failure');}
  const pending=this.#pending.get(packet.id);
  if(!pending){this.close();throw Error('Owned toolkit response correlation failure');}
  this.#pending.delete(packet.id);
  pending.resolve({ok:packet.ok,payload:packet.payload,...(packet.callbackReceipt===undefined?{}:{callbackReceipt:packet.callbackReceipt})});
 }
 close(){
  if(this.#closed)return;this.#closed=true;
  for(const pending of this.#pending.values())pending.reject(Error('Owned toolkit service closed'));
  this.#pending.clear();
 }
}
