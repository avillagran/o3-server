import fs from "node:fs";
import promises from "node:fs/promises";
import child from "node:child_process";
import {promisify} from "node:util";
import {syncBuiltinESMExports} from "node:module";
import {basename, isAbsolute, join} from "node:path";
const directory=process.env.O3_RECURSIVE_FS_GUARD_DIR;
if(!directory||!isAbsolute(directory))throw Error("A private absolute guard directory is required");
const originalOpen=fs.openSync,originalWrite=fs.writeSync,originalClose=fs.closeSync;
function write(name,value,flags){const fd=originalOpen(join(directory,name),flags|fs.constants.O_NOFOLLOW,0o600);try{originalWrite(fd,JSON.stringify(value)+"\n")}finally{originalClose(fd)}}
function refuse(operation,reason){const error=new Error("Prohibited operation in private device proof: "+operation+" "+reason);error.code="ERR_PRIVATE_PROOF_PROHIBITED";write(process.pid+".attempts.jsonl",{operation,reason,stack:error.stack},fs.constants.O_WRONLY|fs.constants.O_CREAT|fs.constants.O_APPEND);throw error}
function preserve(original,replacement,check){for(const key of Reflect.ownKeys(original)){const descriptor=Object.getOwnPropertyDescriptor(original,key);if(key===promisify.custom&&typeof descriptor.value==="function"){const custom=descriptor.value;descriptor.value=function(...args){check(...args);return Reflect.apply(custom,this,args)}}Object.defineProperty(replacement,key,descriptor)}return replacement}
function checked(operation,options){if(options&&options.recursive===true)refuse(operation,"recursive removal")}
for(const name of ["rm","rmSync","rmdir","rmdirSync"]){const original=fs[name];fs[name]=preserve(original,function(path,options,...args){checked("fs."+name,options);return Reflect.apply(original,fs,[path,options,...args])},(path,options)=>checked("fs."+name,options))}
for(const name of ["rm","rmdir"]){const original=promises[name];promises[name]=async function(path,options){checked("fs/promises."+name,options);return Reflect.apply(original,promises,[path,options])}}
function checkBuild(operation,command,args){const name=typeof command==="string"?basename(command):"";const values=Array.isArray(args)?args.filter(v=>typeof v==="string"):[];if(/^(?:gradle|gradlew|javac)(?:\.bat|\.cmd)?$/.test(name)||((name==="bash"||name==="sh")&&values.some(v=>/(?:^|\/)build\.sh$/.test(v)))||(name==="java"&&values.some(v=>/junit|robolectric/i.test(v))))refuse(operation,"helper or Android build/test executable "+name)}
for(const name of ["spawn","spawnSync","execFile","execFileSync","fork"]){const original=child[name];child[name]=preserve(original,function(command,args,...rest){checkBuild("child_process."+name,command,args);return Reflect.apply(original,child,[command,args,...rest])},(command,args)=>checkBuild("child_process."+name,command,args))}
for(const name of ["exec","execSync"]){const original=child[name];const check=command=>{if(typeof command==="string"&&/(?:^|[\s/])(?:gradlew?|javac)(?:[\s]|$)|(?:^|[\s/])build\.sh(?:[\s]|$)/.test(command))refuse("child_process."+name,"build/test shell command")};child[name]=preserve(original,function(command,...rest){check(command);return Reflect.apply(original,child,[command,...rest])},check)}
syncBuiltinESMExports();
const live=await import("node:fs"),livePromises=await import("node:fs/promises"),liveChild=await import("node:child_process"),bareFs=await import("fs"),barePromises=await import("fs/promises");
if(live.rm!==fs.rm||live.rmSync!==fs.rmSync||livePromises.rm!==promises.rm||livePromises.rmdir!==promises.rmdir||liveChild.spawn!==child.spawn||bareFs.rm!==fs.rm||barePromises.rm!==promises.rm)throw Error("Private proof guard export identity mismatch");
write(process.pid+".installed.json",{pid:process.pid,fsNamedIdentity:true,promisesNamedIdentity:true,childNamedIdentity:true},fs.constants.O_WRONLY|fs.constants.O_CREAT|fs.constants.O_EXCL);
process.stderr.write("PRIVATE_DEVICE_PROOF_GUARD_INSTALLED\n");
