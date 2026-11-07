// Run inside the private test container. JSON.stringify and slice are native intrinsics.
import { readFileSync } from 'node:fs';
const source = '/native/apps/server/src/preview/ServerBrowser.ts';
const native = readFileSync(source, 'utf8');
for (const field of ['message', 'defaultValue']) {
  if (!native.includes(`tab.dialog.${field}().slice(0, 4000)`)) throw new Error('Native clipping changed');
}
const cases = [
  ['empty', []], ['controls', Array.from({length:32},(_,i)=>i)],
  ['quotes-backslashes', [34,92,47]], ['pair', [0xd83d,0xde00]],
  ['high', [0xd800]], ['low', [0xdc00]], ['mixed', [0xd800,65,0xdc00,0xd83d,0xde00]],
  ['unicode', [0x2028,0x2029,0xfeff,0xffff]],
  ['clipped-pair', [...Array(3999).fill(65),0xd83d,0xde00]],
  ['complete-boundary', [...Array(3998).fill(65),0xd83d,0xde00]],
  ['past-boundary', [...Array(4000).fill(65),0xd83d,0xde00]],
].map(([name, units]) => {
  const string = String.fromCharCode(...units).slice(0,4000);
  const clipped = Array.from({length:string.length},(_,i)=>string.charCodeAt(i));
  return { name, units, clipped, quoted:JSON.stringify(string), structured:JSON.stringify({dialog:{type:'prompt',message:string,defaultValue:string},tabs:[null,true]}) };
});
console.log(JSON.stringify({nativeRevision:process.env.NATIVE_REVISION,nativeSource:source,cases},null,2));
