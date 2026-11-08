// Fixed framing shared by the owned worker entry; no filesystem or evaluation.
export const MAGIC=Buffer.from('O3PG0001');
export const MAX_FRAME_BYTES=16*1024*1024;
export class FrameDecoder {
 #header=Buffer.alloc(12);#headerBytes=0;#body;#bodyBytes=0;#accept;
 constructor(accept){this.#accept=accept;}
 feed(chunk){
  let offset=0;
  while(offset<chunk.length){
   if(this.#headerBytes<12){const size=Math.min(12-this.#headerBytes,chunk.length-offset);chunk.copy(this.#header,this.#headerBytes,offset,offset+size);offset+=size;this.#headerBytes+=size;if(this.#headerBytes<12)continue;
    if(!this.#header.subarray(0,8).equals(MAGIC))throw Error('frame_magic');const length=this.#header.readUInt32LE(8);if(!length||length>MAX_FRAME_BYTES)throw Error('frame_length');this.#body=Buffer.alloc(length);
   }
   const size=Math.min(this.#body.length-this.#bodyBytes,chunk.length-offset);chunk.copy(this.#body,this.#bodyBytes,offset,offset+size);offset+=size;this.#bodyBytes+=size;
   if(this.#bodyBytes===this.#body.length){const value=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(this.#body));this.#headerBytes=0;this.#bodyBytes=0;this.#body=undefined;this.#accept(value);}
  }
 }
}
