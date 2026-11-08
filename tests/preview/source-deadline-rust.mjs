// Actual Node -> exact production Rust conversion through owned private stdio.
import fs from 'node:fs';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {performance} from 'node:perf_hooks';
import {hrtime} from 'node:process';
import {captureSourceDeadline} from '../../runtime/preview/source-deadline.mjs';
assert(fs.existsSync('/run/.containerenv'));assert.equal(process.versions.node,'26.13.0');
const child=spawn('/fixture',{stdio:['pipe','pipe','pipe'],env:{PATH:'/usr/bin:/bin',HOME:process.env.HOME}});
const ended=Promise.withResolvers();child.once('error',ended.reject);child.once('close',(code,signal)=>ended.resolve({code,signal}));
let stderr=0;child.stderr.on('data',bytes=>{stderr+=bytes.length;});
const lines=createInterface({input:child.stdout})[Symbol.asyncIterator]();
async function command(value){await new Promise((resolve,reject)=>child.stdin.write(value+'\n',error=>error?reject(error):resolve()));const reply=await lines.next();assert.equal(reply.done,false);assert(reply.value.length<=512);return reply.value.split(' ');}
const samples=[];let validationCases=0,queuedExpiry;
try{
 const selftest=await command('selftest');assert.equal(selftest[0],'SELFTEST');validationCases=Number(selftest[1]);
 for(let id=1;id<=40;id++){
  const originalDeadlineMs=performance.now()+(id%2?5000:-5000),source=captureSourceDeadline(originalDeadlineMs);
  const before=hrtime.bigint(),reply=await command(`capture ${id} ${source.lowerNs} ${source.upperNs}`),after=hrtime.bigint();
  assert.equal(reply[0],'CAPTURE');assert.equal(Number(reply[1]),id);
  const [mb,ma,lower,upper,delta]=reply.slice(2,7).map(BigInt);const expired=reply[7]==='1';
  assert(before<=mb&&mb<=ma&&ma<=after,'Rust clock is inside the actual Node stdio round trip');
  assert.equal(String(lower),source.lowerNs);assert.equal(String(upper),source.upperNs);assert.equal(delta,lower-ma,'Exact signed shift is against Rust after sample');
  assert.equal(expired,id%2===0);
  const entry=await command(`entry ${id}`);assert.equal(entry[0],'ENTRY');assert.equal(entry[2],expired?'1':'0');
  samples.push({id,expired,sourceIntervalNs:String(upper-lower),rustBracketNs:String(ma-mb),roundTripNs:String(after-before),signedShiftVerified:true});
 }
 const deadline=performance.now()+50,source=captureSourceDeadline(deadline),created=await command(`capture 41 ${source.lowerNs} ${source.upperNs}`);
 assert.equal(created[0],'CAPTURE');assert.equal(created[7],'0','A fresh captured deadline is initially live');
 assert.deepEqual(await command('wait 70'),['WAIT']);
 const entry=await command('entry 41');assert.equal(entry[2],'1','Waiting work expires without resetting its frozen deadline');assert.equal(entry[3],'20','Expired waiting work never executes the fixture effect');
 assert(performance.now()>=deadline);queuedExpiry={initiallyLive:true,expiredAtEntry:true,effectCountUnchanged:true,storedInstantUnchanged:true,actualProductionControl:false};
 await command('wait 0');child.stdin.end('quit\n');const exit=await ended.promise;assert.equal(exit.code,0);assert.equal(exit.signal,null);assert.equal(stderr,0);
}catch(error){child.kill();await ended.promise;throw error;}
const guard=process.env.O3_RECURSIVE_FS_GUARD_DIR;assert(guard&&fs.readdirSync(guard).some(name=>name.endsWith('.installed.json')));assert.equal(fs.readdirSync(guard).filter(name=>name.endsWith('.attempts.jsonl')).length,0);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,nodeVersion:process.versions.node,rustSource:'/source/src/native_page_worker/source_deadline.rs',samples,validationCases,queuedExpiry,
 qualification:'The unregistered exact production module uses real Linux CLOCK_MONOTONIC and std::Instant; standalone fixture supplies only the reviewed Linux x64 libc ABI and three Fault variants. Signed conversion and immutable waiting admission checks are physical; no Tokio from_std equivalence or actual production Control/callback-entry integration is proved.',
 sourceReferences:['https://github.com/rust-lang/rust/blob/1.99.1/library/std/src/sys/time/unix.rs#L61-L66','https://github.com/nodejs/node/blob/v26.13.0/src/node_perf.cc#L345-L347']},null,2)+'\n');
console.log('SOURCE_DEADLINE_RUST_INSTANT_PASS');
