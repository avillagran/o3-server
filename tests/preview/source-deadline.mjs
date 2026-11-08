// Source-only numerical model: Node 26.13.0's original performance comparator.
// Actual Linux-clock measurement below is separate from Rust Instant conversion.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import {performance} from 'node:perf_hooks';
import {hrtime} from 'node:process';
import {spawn} from 'node:child_process';
import {createInterface} from 'node:readline';
import {MAX_ELAPSED_NS,nativeElapsedMilliseconds as native,elapsedNanosecondBounds,nativeExpiryElapsedNs,sourceDeadlineBounds,captureSourceDeadline} from '../../runtime/preview/source-deadline.mjs';
assert(fs.existsSync('/run/.containerenv'));
assert.equal(process.versions.node,'26.13.0');
const bits=new DataView(new ArrayBuffer(8));
function adjacent(value,direction){bits.setFloat64(0,value);let raw=bits.getBigUint64(0);raw+=BigInt(direction);bits.setBigUint64(0,raw);return bits.getFloat64(0);}
function exactFloorScaled(value){
 bits.setFloat64(0,value);const raw=bits.getBigUint64(0),negative=(raw>>63n)!==0n;
 const exponent=Number((raw>>52n)&2047n),fraction=raw&((1n<<52n)-1n);
 let numerator=(exponent===0?fraction:(1n<<52n)|fraction)*1000000n;
 const shift=(exponent===0?-1074:exponent-1023-52);
 if(shift>=0)return (negative?-numerator:numerator)<<BigInt(shift);
 const denominator=1n<<BigInt(-shift);if(!negative)return numerator/denominator;
 return -((numerator+denominator-1n)/denominator);
}
assert.equal(Math.floor(0.3*1e6),300000);
assert.equal(exactFloorScaled(0.3),299999n); // Naive floor(product) can round upward.
let numericalCases=0,boundsCases=0,linearReferenceCases=0;
const elapsed=new Set([0n,1n,2n,999999n,1000000n,123456789n,MAX_ELAPSED_NS]);
for(let bit=0n;bit<=63n;bit++)for(const delta of [-2n,-1n,0n,1n,2n]){const n=(1n<<bit)+delta;if(n>=0n&&n<=MAX_ELAPSED_NS)elapsed.add(n);}
for(const n of elapsed){
 const p=native(n),range=elapsedNanosecondBounds(p);
 assert(range.lower<=n&&n<=range.upper);
 assert.equal(native(range.lower),p);assert.equal(native(range.upper),p);
 if(range.lower>0n)assert(native(range.lower-1n)<p);
 if(range.upper<MAX_ELAPSED_NS)assert(native(range.upper+1n)>p);
 const deadlines=new Set([p,p-5000,p+1,adjacent(p,1)]);
 if(p>0)deadlines.add(adjacent(p,-1));
 for(const deadline of deadlines){
  if(!Number.isFinite(deadline)||deadline>native(MAX_ELAPSED_NS))continue;
  const expiry=nativeExpiryElapsedNs(deadline);
  assert(native(expiry)>=deadline);
  if(expiry>0n)assert(native(expiry-1n)<deadline);
  numericalCases++;
  const origin=MAX_ELAPSED_NS-n>=1000000n?1000000n:0n,sample=origin+n;
  const beforeNs=sample>3n?sample-3n:0n,afterNs=sample+7n<=MAX_ELAPSED_NS?sample+7n:MAX_ELAPSED_NS;
  const frozen=sourceDeadlineBounds(deadline,{beforeNs,nowMs:p,afterNs});
  const lower=BigInt(frozen.lowerNs),upper=BigInt(frozen.upperNs),actual=origin+expiry;
  assert(lower<=actual&&actual<=upper);
  assert.match(frozen.lowerNs,/^-?(?:0|[1-9]\d*)$/);assert.match(frozen.upperNs,/^-?(?:0|[1-9]\d*)$/);
  assert.deepEqual(Object.keys(frozen),['lowerNs','upperNs']);assert(Object.isFrozen(frozen));boundsCases++;
 }
}
// Independent bounded linear reference around known native samples, including
// integer-to-double plateaus beyond 2^53. It does not copy the binary search.
for(const n of [0n,1n,1000000n,(1n<<53n)-1n,1n<<53n,(1n<<53n)+1n,1n<<60n,1n<<63n,MAX_ELAPSED_NS]){
 const deadline=native(n),start=n>16384n?n-16384n:0n,stop=n+16384n<MAX_ELAPSED_NS?n+16384n:MAX_ELAPSED_NS;
 let first=null,last=null;
 for(let cursor=start;cursor<=stop;cursor++){const value=native(cursor);if(value>=deadline&&first===null)first=cursor;if(value<=deadline)last=cursor;}
 const range=elapsedNanosecondBounds(deadline);assert.equal(range.lower,first);assert.equal(range.upper,last);linearReferenceCases++;
}
let rejectedCases=0;
for(const value of [NaN,Infinity,-Infinity,'1',null,undefined]){assert.throws(()=>nativeExpiryElapsedNs(value),RangeError);rejectedCases++;}
for(const value of [-1,0.0000005,Infinity,NaN,native(MAX_ELAPSED_NS)+1]){assert.throws(()=>elapsedNanosecondBounds(value),RangeError);rejectedCases++;}
for(const value of [-1n,MAX_ELAPSED_NS+1n,1,'1']){assert.throws(()=>native(value),RangeError);rejectedCases++;}
for(const sample of [{beforeNs:2n,nowMs:0,afterNs:1n},{beforeNs:0n,nowMs:1,afterNs:0n},{beforeNs:0n,nowMs:0,afterNs:MAX_ELAPSED_NS+1n}]){assert.throws(()=>sourceDeadlineBounds(1,sample),RangeError);rejectedCases++;}
// Explicitly retain signed expired bounds; no rebasing to a fresh timeout.
assert(BigInt(sourceDeadlineBounds(-1,{beforeNs:0n,nowMs:native(1000000n),afterNs:1000000n}).lowerNs)<0n);
const program="import sys,json,time\nfor line in sys.stdin:\n row=json.loads(line)\n print(json.dumps({'id':row['id'],'ns':str(time.clock_gettime_ns(time.CLOCK_MONOTONIC))}),flush=True)\n";
const child=spawn('/usr/bin/python3',['-u','-c',program],{stdio:['pipe','pipe','pipe'],env:{PATH:'/usr/bin:/bin',HOME:process.env.HOME}});
const ended=Promise.withResolvers();child.once('error',ended.reject);child.once('close',(code,signal)=>ended.resolve({code,signal}));
let stderr=0;child.stderr.on('data',bytes=>{stderr+=bytes.length;});
const replies=createInterface({input:child.stdout})[Symbol.asyncIterator](),samples=[];
try{
 for(let id=1;id<=64;id++){
  const deadline=performance.now()+(id%2?5000:-5000),frozen=captureSourceDeadline(deadline);
  const sent=hrtime.bigint();await new Promise((resolve,reject)=>child.stdin.write(JSON.stringify({id})+'\n',error=>error?reject(error):resolve()));
  const next=await replies.next(),received=hrtime.bigint();assert.equal(next.done,false);const reply=JSON.parse(next.value),kernel=BigInt(reply.ns);
  assert.equal(reply.id,id);assert(sent<=kernel&&kernel<=received);
  const lower=BigInt(frozen.lowerNs),upper=BigInt(frozen.upperNs);
  if(id%2)assert(kernel<lower);else assert(kernel>=upper);
  samples.push({id,expired:id%2===0,intervalNs:String(upper-lower),roundTripNs:String(received-sent)});
 }
 child.stdin.end();const exit=await ended.promise;assert.equal(exit.code,0);assert.equal(exit.signal,null);assert.equal(stderr,0);
}catch(error){child.kill();await ended.promise;throw error;}
// Real native inclusive comparator at and after its exact captured deadline.
// The upper bound may be conservative late for measurement only; production
// admission uses lowerNs exclusively and never waits for this diagnostic bound.
const nativeComparisons=[];
for(let id=1;id<=8;id++){
 const deadline=performance.now()+0.5,frozen=captureSourceDeadline(deadline),upper=BigInt(frozen.upperNs);
 const bound=hrtime.bigint()+5000000n;
 while(hrtime.bigint()<upper&&hrtime.bigint()<bound){}
 assert(hrtime.bigint()>=upper,'Bounded private comparator measurement');
 assert(performance.now()>=deadline);nativeComparisons.push({id,inclusiveExpired:true});
}
const guard=process.env.O3_RECURSIVE_FS_GUARD_DIR;
assert(guard&&fs.readdirSync(guard).some(name=>name.endsWith('.installed.json')));
assert.equal(fs.readdirSync(guard).filter(name=>name.endsWith('.attempts.jsonl')).length,0);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,nodeVersion:process.versions.node,numericalCases,boundsCases,linearReferenceCases,rejectedCases,samples,nativeComparisons,
 sourceReferences:['https://github.com/nodejs/node/blob/v26.13.0/src/node_perf.cc#L345-L347','https://github.com/nodejs/node/blob/v26.13.0/src/node_process_methods.cc#L667-L674','https://github.com/nodejs/node/blob/v26.13.0/deps/uv/src/unix/core.c#L141-L143','https://github.com/nodejs/node/blob/v26.13.0/deps/uv/src/unix/linux.c#L1508-L1539'],
 qualification:'Exact source-backed Node double(elapsed u64 ns)/1e6 threshold inversion plus actual Node/Linux CLOCK_MONOTONIC measurement in one private namespace. No Rust Instant conversion, control queue admission, callback cancellation or complete production timing equivalence is proved.'},null,2)+'\n');
console.log('SOURCE_DEADLINE_NODE_KERNEL_PASS');
