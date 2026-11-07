#!/usr/bin/env node
// Execute the complete pinned native Tailscale module without spawning commands.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-managed-tailscale-fixtures.mjs CHECKOUT CONTRACTS OUTPUT');
const revision='c55db3e9c72fdf584f4b54192fb39beb88667b59';
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const directory=path.dirname(output);fs.mkdirSync(directory,{recursive:true});
const sources={},loaded=new Map();
function module(file){
 if(loaded.has(file))return loaded.get(file);
 const target=path.join(directory,crypto.createHash('sha256').update(file).digest('hex')+'.mjs');
 loaded.set(file,target);
 const source=execFileSync('git',['-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'});
 sources[file]=crypto.createHash('sha256').update(source).digest('hex');
 const code=stripTypeScriptTypes(source).replace(/from\s+(["'])([^"']+)\1/g,(all,quote,specifier)=>{
  let destination;
  if(specifier==='@t3tools/contracts')destination=contracts+'/src/index.ts';
  else if(specifier.startsWith('effect/'))destination=require.resolve(specifier);
  else if(specifier.startsWith('./')||specifier.startsWith('../'))destination=module(path.posix.normalize(path.posix.join(path.posix.dirname(file),specifier)));
  else if(specifier.startsWith('@t3tools/')){
   const [,pkg,entry]=specifier.match(/^@t3tools\/([^/]+)\/(.+)$/)??[];
   if(!pkg)throw Error('Unhandled package '+specifier);
   destination=module(`packages/${pkg}/src/${entry}.ts`);
  }else return all;
  return 'from '+JSON.stringify(pathToFileURL(destination).href);
 });
 fs.writeFileSync(target,code);return target;
}
const native=await import(pathToFileURL(module('packages/tailscale/src/tailscale.ts')).href);
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const targets=['127.0.0.1','127.0.0.2','::1'].flatMap(host=>[80,443,3773].map(port=>({host,port})));
const endpoints=['3773','03773','0','127.0.0.1:3773','localhost:3773','box.1:3773','127.1:3773','127.000.0.1:3773','0.0.0.0:3773','[::]:3773','[::1]:3773','[0:0:0:0:0:0:0:1]:3773','[::ffff:127.0.0.1]:3773','[::ffff:7f00:1]:3773','[0:0:0:0:0:ffff:7f00:1]:3773','127.0.0.2:3773','192.168.1.2:3773','unix:/tmp/synthetic.sock','127.0.0.1:03773','127.0.0.1:443','127.0.0.1:80','127.0.0.1','127.0.0.1:','user@127.0.0.1:3773','127.0.0.1:3773/path?query','HOST.:3773','[::1].:3773','[broken]:3773','',':3773'];
const proxies=endpoints.flatMap(endpoint=>[endpoint,`http://${endpoint}`,`https://${endpoint}`,`https+insecure://${endpoint}`,`tcp://${endpoint}`]);
const cases=[];
for(const target of targets)for(const proxy of proxies){
 for(const kind of ['tcp','web','service','foreground']){
  const config=kind==='tcp'?{TCP:{443:{TCPForward:proxy}}}:{Web:{'node.tail.example:443':{Handlers:{'/kept':{Proxy:'127.0.0.1:9999'},'/target':{Proxy:proxy}}}}};
  const status=kind==='service'?{Services:{'svc:sample':config}}:kind==='foreground'?{Foreground:{worker:config}}:config;
  const raw=JSON.stringify(status);cases.push({target,raw,expected:native.parseTailscaleServeRoutesTo(raw,target)??null});
 }
}
const malformed=['null','{}','[]','4','false','"value"','{','{"TCP":null}','{"Web":[]}','{"Services":{"sample":null}}','{"Foreground":[]}','{"TCP":{"443":null}}','{"TCP":{"443":{"TCPForward":3}}}','{"Web":{"node:443":{"Handlers":null}}}','{"Web":{"node:443":{"Handlers":{"/":{"Proxy":false}}}}}','{"unrecognized":{"private":"ignored"}}'];
for(const raw of malformed)cases.push({target:targets[0],raw,expected:native.parseTailscaleServeRoutesTo(raw,targets[0])??null});
const routePorts=['443','80','0','-1',' 443 ','0x1bb','4.43e2','1.5','NaN','Infinity','','4294967295','9007199254740992'];
for(const port of routePorts)for(const target of targets){const raw=JSON.stringify({TCP:{[port]:{TCPForward:`${target.host==='::1'?'[::1]':target.host}:${target.port}`}},Web:{[`node:${port}`]:{Handlers:{'/':{Proxy:String(target.port)}}}}});cases.push({target,raw,expected:native.parseTailscaleServeRoutesTo(raw,target)??null});}
const statuses=[];
for(const value of [null,{},[],false,{Self:null},{Self:[]},{Self:{}},...[undefined,null,' node.tail.example. ','UPPER.EXAMPLE..','\ufeffnode.\ufeff','',42].flatMap(DNSName=>[undefined,{},[],{https:[]},{other:true},null].map(CapMap=>({Self:{DNSName,CapMap,TailscaleIPs:['100.64.1.2','100.127.255.255','100.128.0.1','100.64x.1.2','100.64.1.2tail','100.64.1.2.3','::1',5]}})))]){const raw=JSON.stringify(value);let expected;try{expected=Effect.runSync(native.parseTailscaleStatus(raw));}catch{expected=null;}statuses.push({raw,expected});}
const off=[];
for(const owner of [undefined,'foreground','svc:sample'])for(const mount of [undefined,'/','/kept'])for(const servePort of [443,0,-1,1e21]){const route={servePort,...(owner===undefined?{}:{owner}),...(mount===undefined?{}:{mount})};off.push({route,expected:native.tailscaleServeOffArgs(route)??null});}
const fixture={revision,effect:'4.1.2',sources,cases,statuses,off};
const serialized=Object.entries(fixture).map(([key,value])=>'  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))).join(',\n');
fs.writeFileSync(output,'{\n'+serialized+'\n}\n');
console.log(`${cases.length} Serve cases, ${statuses.length} status cases, ${off.length} selective off argument cases`);
