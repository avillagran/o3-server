// Node 26.13.0 src/node_perf.cc PerformanceNowImpl: double(elapsedNs)/1e6.
// Keep the original numerical deadline; transport/queue time never restarts it.
import {performance} from 'node:perf_hooks';
import {hrtime} from 'node:process';
export const MAX_ELAPSED_NS=(1n<<64n)-1n;
const SCALE=1e6;
function finite(value){if(typeof value!=='number'||!Number.isFinite(value))throw new RangeError('Invalid source deadline clock value');return value;}
function unsigned(value){if(typeof value!=='bigint'||value<0n||value>MAX_ELAPSED_NS)throw new RangeError('Invalid source monotonic sample');return value;}
export function nativeElapsedMilliseconds(elapsedNs){return Number(unsigned(elapsedNs))/SCALE;}
// Exactly 64 bit decisions, without evaluating the unrepresentable u64+1
// sentinel. Callers establish that zero belongs to the monotone accepted set.
function maximumAccepted(accept){
 let result=0n;
 for(let bit=63n;bit>=0n;bit--){const candidate=result|(1n<<bit);if(accept(candidate))result=candidate;}
 return result;
}
export function nativeExpiryElapsedNs(deadlineMs){
 finite(deadlineMs);
 if(deadlineMs<=0)return 0n;
 if(deadlineMs>nativeElapsedMilliseconds(MAX_ELAPSED_NS))throw new RangeError('Source deadline exceeds native monotonic range');
 // Native SessionControl expires inclusively: performance.now() >= deadline.
 return maximumAccepted(ns=>nativeElapsedMilliseconds(ns)<deadlineMs)+1n;
}
export function elapsedNanosecondBounds(nowMs){
 finite(nowMs);
 if(nowMs<0||nowMs>nativeElapsedMilliseconds(MAX_ELAPSED_NS))throw new RangeError('Invalid native performance sample');
 const lower=nativeExpiryElapsedNs(nowMs);
 if(nativeElapsedMilliseconds(lower)!==nowMs)throw new RangeError('Unattainable native performance sample');
 const upper=maximumAccepted(ns=>nativeElapsedMilliseconds(ns)<=nowMs);
 return Object.freeze({lower,upper});
}
export function sourceDeadlineBounds(originalDeadlineMs,{beforeNs,nowMs,afterNs}){
 unsigned(beforeNs);unsigned(afterNs);
 if(afterNs<beforeNs)throw new RangeError('Reversed source monotonic bracket');
 const elapsed=elapsedNanosecondBounds(nowMs),expiry=nativeExpiryElapsedNs(originalDeadlineMs);
 if(elapsed.lower>afterNs)throw new RangeError('Inconsistent source monotonic clocks');
 // Actual origin = sampledHrtime - elapsed. It therefore lies between these
 // bracket endpoints even when both integer-to-double and division round.
 const lower=beforeNs-elapsed.upper+expiry,upper=afterNs-elapsed.lower+expiry;
 // Signed canonical decimals preserve already-expired deadlines. No Number
 // conversion of absolute clock coordinates, epsilon, clamp or response grace.
 return Object.freeze({lowerNs:String(lower),upperNs:String(upper)});
}
export function captureSourceDeadline(originalDeadlineMs){
 const beforeNs=hrtime.bigint(),nowMs=performance.now(),afterNs=hrtime.bigint();
 return sourceDeadlineBounds(originalDeadlineMs,{beforeNs,nowMs,afterNs});
}
