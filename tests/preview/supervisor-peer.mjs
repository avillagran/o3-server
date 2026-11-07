// Synthetic process boundary fixture, not a production fallback.
import fs from 'node:fs';
fs.writeFileSync(process.env.HOME+'/peer-started.json',JSON.stringify({pid:process.pid,stage:'module'}));
const MAGIC=Buffer.from('O3PG0001');let pending=Buffer.alloc(0),init,mode,latePacket;let invalidations=0,invocations=0;
function send(value){const body=Buffer.from(JSON.stringify(value));const head=Buffer.alloc(12);MAGIC.copy(head);head.writeUInt32LE(body.length,8);process.stdout.write(Buffer.concat([head,body]));}
process.stdin.on('data',chunk=>{pending=Buffer.concat([pending,chunk]);while(pending.length>=12){if(!pending.subarray(0,8).equals(MAGIC))process.exit(2);const n=pending.readUInt32LE(8);if(n>16777216)process.exit(2);if(pending.length<n+12)return;const packet=JSON.parse(pending.subarray(12,12+n));pending=pending.subarray(12+n);handle(packet);}});
function handle(packet){
 if(packet.kind==='init'){
  init=packet;mode=packet.configuration.fixture;fs.writeFileSync(process.env.HOME+'/peer-started.json',JSON.stringify({pid:process.pid,stage:'init',fixture:mode}));
  if(mode==='mixed'){process.stdout.write('not protocol stdout\n');}
  if(mode==='oversize'){const b=Buffer.alloc(12);MAGIC.copy(b);b.writeUInt32LE(16777217,8);process.stdout.write(b);return;}
  if(mode==='startup-hang')return;
  send({kind:'ready',protocol:1,nonce:mode==='wrong-nonce'?'wrong':init.nonce});return;
 }
 if(packet.kind==='close'){if(mode==='late'&&latePacket){fs.writeFileSync(process.env.HOME+'/late-sent.json',JSON.stringify({afterOwnerClose:true}));send({kind:'result',nonce:init.nonce,id:latePacket.id,ok:true,value:'late'});}if(mode==='hang'||mode==='startup-hang')return;send({kind:'closed',nonce:init.nonce,artifactHealthy:true,nativeRecursiveRequests:0,executedRecursiveRemovals:0});process.stdout.write('',()=>process.exit(0));}
 if(packet.kind==='owner-control'){invalidations++;send({kind:'owner-result',nonce:init.nonce,id:mode==='owner-wrong-id'?packet.id+1:packet.id,ok:true,...(mode==='owner-extra'?{value:true}:{})});return;}
 if(packet.kind==='invoke'){
  fs.writeFileSync(process.env.HOME+'/peer-invocations.json',JSON.stringify({count:++invocations}));
  if(mode==='surrogate'){send({kind:'result',nonce:init.nonce,id:packet.id,ok:true,value:{'\ud800':'\udfff',pair:'😀'}});return;}
  if(mode==='execution-gate'){
   const timer=setInterval(()=>{try{const marker=fs.lstatSync(process.env.HOME+'/release-execution');if(!marker.isFile()||marker.isSymbolicLink())process.exit(2);clearInterval(timer);send({kind:'result',nonce:init.nonce,id:packet.id,ok:true,value:{nativeTimeoutMs:packet.timeoutMs,pid:process.pid}});}catch(error){if(error.code!=='ENOENT')process.exit(2);}},5);return;
  }
  if(mode==='grace'){setTimeout(()=>send({kind:'result',nonce:init.nonce,id:packet.id,ok:true,value:{nativeTimeoutMs:packet.timeoutMs}}),packet.timeoutMs+60);return;}
  if(mode==='crash'){process.exit(17);}
  if(mode==='hang')return;
  if(mode==='malformed'){send({kind:'result',nonce:init.nonce,id:packet.id,ok:true});return;}
  if(mode==='late'){latePacket=packet;return;}
  if(mode==='native-error'){send({kind:'result',nonce:init.nonce,id:packet.id,ok:false,error:{tag:'PreviewAutomationExecutionError',message:'synthetic native failure'}});return;}
  send({kind:'result',nonce:init.nonce,id:packet.id,ok:true,value:{operation:packet.operation,input:packet.input,invalidations,home:process.env.HOME,ambient:process.env.O3_HOST_SENTINEL??null,pid:process.pid}});
 }
}
process.stdin.on('end',()=>process.exit(0));
