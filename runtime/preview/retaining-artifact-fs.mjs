// Explicit retention boundary for the unchanged Playwright attach cleanup.
import fs from 'node:fs';
import promises from 'node:fs/promises';
import path from 'node:path';
import {randomUUID} from 'node:crypto';
import {syncBuiltinESMExports} from 'node:module';
const identity=s=>({dev:s.dev.toString(),ino:s.ino.toString(),uid:s.uid.toString()});
const same=(a,b)=>a.dev===b.dev&&a.ino===b.ino&&a.uid===b.uid;
function directory(value){
 if(!path.isAbsolute(value)||path.resolve(value)!==value)throw Error('Artifact root must be canonical and absolute');
 let prefix=path.parse(value).root;
 for(const part of value.slice(prefix.length).split('/').filter(Boolean)){prefix=path.join(prefix,part);if(fs.lstatSync(prefix).isSymbolicLink())throw Error('Artifact ancestor is a link');}
 const fd=fs.openSync(value,fs.constants.O_RDONLY|fs.constants.O_DIRECTORY|fs.constants.O_NOFOLLOW);
 try{const s=fs.fstatSync(fd,{bigint:true});if(!s.isDirectory()||s.uid!==BigInt(process.getuid())||(s.mode&0o777n)!==0o700n)throw Error('Artifact root must be owned0700');return identity(s);}finally{fs.closeSync(fd);}
}
export class RetainingArtifactFs {
 #root;#quarantine;#rootIdentity;#quarantineIdentity;#owned=new Map();#original;#receipts=[];#fault=false;
 constructor({tempRoot,quarantineRoot}){
  this.#root=tempRoot;this.#quarantine=quarantineRoot;this.#rootIdentity=directory(tempRoot);this.#quarantineIdentity=directory(quarantineRoot);
  if(this.#rootIdentity.dev!==this.#quarantineIdentity.dev)throw Error('Artifact retention must be atomic on one filesystem');
 }
 get receipts(){return structuredClone(this.#receipts);}
 get installed(){return !!this.#original;}
 get healthy(){return !this.#fault;}
 #roots(){if(!same(directory(this.#root),this.#rootIdentity)||!same(directory(this.#quarantine),this.#quarantineIdentity))throw Error('Artifact root identity changed');}
 install(){
  if(this.#original)throw Error('Artifact boundary already installed');
  const mkdtemp=promises.mkdtemp,rm=promises.rm;this.#original={mkdtemp,rm};const self=this;
  promises.mkdtemp=async function(prefix,...args){
   const value=await Reflect.apply(mkdtemp,this,[prefix,...args]);
   if(typeof prefix==='string'&&prefix===path.join(self.#root,'playwright-artifacts-')){
    try{self.#roots();if(typeof value!=='string'||path.dirname(value)!==self.#root)throw Error('Invalid artifact slot');self.#owned.set(value,{identity:directory(value),retained:null});}
    catch(error){self.#fault=true;throw error;}
   }
   return value;
  };
  promises.rm=async function(value,options){
   const slot=typeof value==='string'?self.#owned.get(value):undefined;
   if(!slot)return Reflect.apply(rm,this,[value,options]);
   const work=async()=>{try{
    self.#roots();if(options?.recursive!==true||options?.force!==true)throw Error('Unexpected artifact cleanup mode');
    if(slot.retained){try{fs.lstatSync(value);throw Error('Artifact slot was replaced');}catch(error){if(error.code!=='ENOENT')throw error;}self.#receipts.push({kind:'already-retained',nativeRecursiveRequest:true,executedRecursiveRemoval:false});return;}
    if(!same(directory(value),slot.identity))throw Error('Artifact slot identity changed');
    const target=path.join(self.#quarantine,randomUUID());
    // A private owned0700 destination plus unguessable new name prevents replacement.
    try{fs.lstatSync(target);throw Error('Artifact retention collision');}catch(error){if(error.code!=='ENOENT')throw error;}
    await promises.rename(value,target);
    if(!same(directory(target),slot.identity))throw Error('Retained artifact identity changed');
    slot.retained=target;self.#receipts.push({kind:'quarantined',nativeRecursiveRequest:true,executedRecursiveRemoval:false,slot:path.basename(value),retained:path.basename(target),identity:slot.identity});
   }catch(error){self.#fault=true;self.#receipts.push({kind:'refused',nativeRecursiveRequest:options?.recursive===true,executedRecursiveRemoval:false});throw error;}};
   const result=(slot.pending??Promise.resolve()).then(work);slot.pending=result;return result;
  };
  syncBuiltinESMExports();
 }
}
