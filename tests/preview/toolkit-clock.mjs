// Small private measurement only: compare Node's monotonic coordinate with the
// explicit Linux kernel clock. Rust Instant conversion is a separate proof.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
assert(fs.existsSync('/run/.containerenv'));
fs.mkdirSync('/out/guard',{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const program="import sys,json,time\nfor line in sys.stdin:\n row=json.loads(line)\n print(json.dumps({'id':row['id'],'ns':str(time.clock_gettime_ns(time.CLOCK_MONOTONIC))}),flush=True)\n";
const child=spawn('/usr/bin/python3',['-u','-c',program],{stdio:['pipe','pipe','pipe'],env:{PATH:'/usr/bin:/bin',HOME:'/out/home'}});
const exited=Promise.withResolvers();child.once('error',exited.reject);child.once('close',(code,signal)=>exited.resolve({code,signal}));
let stderr=0;child.stderr.on('data',bytes=>{stderr+=bytes.length;if(stderr>4096)child.kill();});
const lines=createInterface({input:child.stdout})[Symbol.asyncIterator](),samples=[];
try{
 for(let id=1;id<=200;id++){
  const deadlineMs=performance.now()+(id%2?5000:-5000);
  const before=process.hrtime.bigint(),now=performance.now(),after=process.hrtime.bigint();
  const remaining=deadlineMs-now;
  const earliest=before+BigInt(Math.floor(remaining*1e6)),latest=after+BigInt(Math.ceil(remaining*1e6));
  const sent=process.hrtime.bigint();await new Promise((resolve,reject)=>child.stdin.write(JSON.stringify({id})+'\n',error=>error?reject(error):resolve()));
  const next=await lines.next();assert.equal(next.done,false);const received=process.hrtime.bigint(),reply=JSON.parse(next.value),kernel=BigInt(reply.ns);
  assert.equal(reply.id,id);assert(sent<=kernel&&kernel<=received,'The kernel sample must lie inside the Node monotonic round trip');
  if(id%2)assert(kernel<earliest);else assert(kernel>=latest);
  samples.push({id,expired:id%2===0,intervalNs:String(latest-earliest),roundTripNs:String(received-sent)});
 }
 child.stdin.end();const exit=await exited.promise;assert.equal(exit.code,0);assert.equal(exit.signal,null);assert.equal(stderr,0);
}catch(error){child.kill();await exited.promise;throw error;}
assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,nodeVersion:process.versions.node,kernelClock:'CLOCK_MONOTONIC',samples,maxIntervalNs:String(samples.reduce((largest,row)=>BigInt(row.intervalNs)>largest?BigInt(row.intervalNs):largest,0n)),qualification:'Node hrtime/performance deadline interval versus the explicit Linux kernel clock in the same private namespace; no Rust Instant conversion, queued admission, extra grace, or production timing equivalence claim.'},null,2)+'\n');
console.log('TOOLKIT_MONOTONIC_KERNEL_MEASUREMENT_PASS');
