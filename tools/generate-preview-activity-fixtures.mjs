#!/usr/bin/env node
// Execute the whole pinned activity service with its actual Effect layer.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,dependencies,output]=process.argv.slice(2);
if(!output)throw Error('usage: generate-preview-activity-fixtures.mjs CHECKOUT DEPENDENCIES OUTPUT');
const require=createRequire(dependencies+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Effect4.1.2 required');
const revision='32a0531748c44b25072d7495ce0b9bd7e9450a87';
const path='apps/server/src/preview/PreviewAgentActivity.ts';
const source=execFileSync('git',['-C',checkout,'show',`${revision}:${path}`],{encoding:'utf8'});
const code=stripTypeScriptTypes(source).replace(/from ["'](effect(?:\/[^"']+)?)['"]/g,(_,name)=>'from '+JSON.stringify(pathToFileURL(require.resolve(name)).href));
const native=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const Effect=await import(pathToFileURL(require.resolve('effect/Effect')).href);
const PubSub=await import(pathToFileURL(require.resolve('effect/PubSub')).href);
const traces=[];
for(let seed=0;seed<32;seed++){
 const operations=[{begin:'a',token:0},{begin:'a',token:1},{end:0},{end:0},{begin:'b',token:2},{end:1},{begin:'a',token:3},{end:0}];
 let rng=seed+1,next=4;
 for(let i=0;i<24;i++){
  rng=(Math.imul(rng,1664525)+1013904223)>>>0;
  operations.push(rng%3===0?{begin:rng%2?'a':'b',token:next++}:{end:rng%next});
 }
 for(let token=0;token<next;token++)operations.push({end:token});
 const steps=await Effect.runPromise(Effect.scoped(Effect.gen(function*(){
  const service=yield*native.PreviewAgentActivity;
  const subscription=yield*service.changes;
  const ends=new Map(),result=[];
  for(const operation of operations){
   if(operation.begin!==undefined)ends.set(operation.token,yield*service.begin(operation.begin));
   else if(ends.has(operation.end))yield*ends.get(operation.end);
   result.push({operation,events:yield*PubSub.takeUpTo(subscription,1000),active:{a:yield*service.active('a'),b:yield*service.active('b')}});
  }
  return result;
 }).pipe(Effect.provide(native.layer))));
 traces.push({seed,steps});
}
fs.writeFileSync(output,JSON.stringify({revision,effect:'4.1.2',sources:{[path]:crypto.createHash('sha256').update(source).digest('hex')},traces},null,2)+'\n');
console.log(`${traces.length} complete native activity traces`);
