// Explicit public WebSocket transport adapter for one sealed private endpoint.
// The Rust owner issues and authorizes opaque tickets; this module grants no scope.
import {AsyncLocalStorage} from 'node:async_hooks';
const installations=new WeakMap();
const FIELD='__o3OwnerReceipt';
export class OwnerScopeTransport {
 #endpoint;#storage=new AsyncLocalStorage();#scopes=new WeakMap();#active=new Map();#installation;#fault;#closed=false;
 constructor({WebSocket,endpoint,onFault}){
  const url=new URL(endpoint);
  if(url.protocol!=='ws:'||url.hostname!=='127.0.0.1'||!url.port||url.username||url.password||url.search||url.hash||url.pathname.length<32||typeof onFault!=='function')throw Error('Invalid owned scope transport');
  this.#endpoint=endpoint;this.#fault=onFault;const prototype=WebSocket.prototype;
  let installation=installations.get(prototype);
  if(!installation){
   const descriptor=Object.getOwnPropertyDescriptor(prototype,'send');if(typeof descriptor?.value!=='function'||!descriptor.configurable)throw Error('Unsupported public WebSocket transport');
   const endpoints=new Map();
   const replacement={send(data,options,callback){
    const adapter=endpoints.get(this.url);if(!adapter)return Reflect.apply(descriptor.value,this,arguments);
    const tagged=adapter.decorate(data);const args=Array.from(arguments);args[0]=tagged;return Reflect.apply(descriptor.value,this,args);
   }}.send;
   Object.defineProperty(prototype,'send',{...descriptor,value:replacement});installation={prototype,descriptor,replacement,endpoints};installations.set(prototype,installation);
  }
  if(installation.endpoints.has(endpoint))throw Error('Owned endpoint already registered');
  installation.endpoints.set(endpoint,this);this.#installation=installation;
 }
 // This is called only by the signed owner IPC consumer, never with MCP input.
 admit(ticket){
  if(this.#closed||typeof ticket!=='string'||!/^[a-f0-9]{48}$/.test(ticket)||this.#active.has(ticket)||this.#active.size>=256)throw Error('Invalid owner receipt admission');
  const scope=Object.freeze({});this.#scopes.set(scope,{ticket,retired:false});this.#active.set(ticket,scope);return scope;
 }
 run(scope,callback){const record=this.#scopes.get(scope);if(this.#closed||!record||record.retired||typeof callback!=='function')throw Error('Inactive owner receipt');return this.#storage.run(scope,callback);}
 finish(scope){const record=this.#scopes.get(scope);if(!record||record.retired)throw Error('Invalid owner receipt completion');record.retired=true;this.#active.delete(record.ticket);}
 decorate(data){
  try{
   if(this.#closed)throw Error();const record=this.#scopes.get(this.#storage.getStore());if(!record)throw Error();
   const value=JSON.parse(typeof data==='string'?data:data.toString());
   if(!value||Array.isArray(value)||!Number.isSafeInteger(value.id)||value.id<0||typeof value.method!=='string'||Object.hasOwn(value,FIELD))throw Error();
   // A late continuation keeps its original ticket. It is never relabeled as
   // the currently running callback; the Rust owner decides whether cleanup is permitted.
   return JSON.stringify({...value,[FIELD]:record.ticket});
  }catch{
   try{this.#fault();}catch{ /* Diagnostics cannot replace the functional fence. */ }
   throw Error('Owned browser scope transport unavailable');
  }
 }
 dispose(){
  if(this.#active.size)throw Error('Owner scopes still active');
  this.#closed=true;const installed=this.#installation;installed.endpoints.delete(this.#endpoint);
  if(!installed.endpoints.size){
   if(Object.getOwnPropertyDescriptor(installed.prototype,'send')?.value!==installed.replacement)throw Error('Owned WebSocket adapter identity changed');
   Object.defineProperty(installed.prototype,'send',installed.descriptor);installations.delete(installed.prototype);
  }
 }
}
