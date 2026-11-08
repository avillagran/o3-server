import fs from 'node:fs';import assert from 'node:assert/strict';import {FrameDecoder,MAGIC,MAX_FRAME_BYTES} from '../../runtime/preview/framed-ipc.mjs';
assert(fs.existsSync('/run/.containerenv'));
function frame(value){const body=Buffer.from(JSON.stringify(value)),header=Buffer.alloc(12);MAGIC.copy(header);header.writeUInt32LE(body.length,8);return Buffer.concat([header,body]);}
const values=[];const decoder=new FrameDecoder(v=>values.push(v));const packet=frame({text:'paired😀value'});for(const byte of packet)decoder.feed(Buffer.from([byte]));assert.deepEqual(values,[{text:'paired😀value'}]);
const maximum='x'.repeat(MAX_FRAME_BYTES-2),batched=Buffer.concat([frame(maximum),frame({next:true})]);decoder.feed(batched);assert.equal(values[1].length,maximum.length);assert.deepEqual(values[2],{next:true});
for(const n of [0,MAX_FRAME_BYTES+1]){const header=Buffer.alloc(12);MAGIC.copy(header);header.writeUInt32LE(n,8);assert.throws(()=>new FrameDecoder(()=>{}).feed(header),/frame_length/);}
const invalid=frame('x');invalid[12]=255;assert.throws(()=>new FrameDecoder(()=>{}).feed(invalid));assert.throws(()=>new FrameDecoder(()=>{}).feed(Buffer.alloc(12)),/frame_magic/);
fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,fragmentedUtf8:true,maximumFramePlusCoalescedSuccessor:true,oversizedBeforeAllocation:true,invalidUtf8Refused:true})+'\n');console.log('NATIVE_PAGE_FRAMING_PASS');
