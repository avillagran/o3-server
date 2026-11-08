// Fixture-only bridge. Preserve the complete original native proof, including
// its two lone-surrogate cases; export a separately qualified Rust Value corpus.
import assert from 'node:assert/strict';
import fs from 'node:fs';import {createHash} from 'node:crypto';
const original=fs.readFileSync('/out/browser-profile-catalog-native.json');
const proof=JSON.parse(original);
assert.equal(proof.complete,true);
assert.equal(proof.revision,'83a3674ffd84ffb33de768210c806cb1ac53fd80');
function wellFormed(value){
 if(typeof value==='string')return value.isWellFormed();
 if(Array.isArray(value))return value.every(wellFormed);
 if(value&&typeof value==='object')return Object.entries(value).every(([key,item])=>key.isWellFormed()&&wellFormed(item));
 return true;
}
assert(proof.traces.every(wellFormed));
const included=[],excluded=[];
for(const [index,entry]of proof.schemaCases.entries()){
 if(wellFormed(entry.input))included.push(entry);
 else {assert.equal(entry.accepted,true);excluded.push({index,reason:'native-string-is-not-well-formed-utf16'});}
}
assert.equal(excluded.length,2);
const reference={complete:true,revision:proof.revision,originalProofSha256:createHash('sha256').update(original).digest('hex'),sourceSha256:proof.sourceSha256,catalogFragmentSha256:proof.catalogFragmentSha256,traces:proof.traces,schemaCases:included,excludedNativeSchemaCases:excluded,qualification:'All original state traces and scalar-string schema cases. Only the two original accepted !String.isWellFormed inputs are excluded from this additional Rust serde_json::Value reference; their complete inputs/results remain in the separately hashed original native proof. No runtime assertion is inferred from this conversion.'};
assert(wellFormed(reference));
fs.writeFileSync('/out/browser-profile-catalog-native-scalar-4314.json',JSON.stringify(reference,null,2)+'\n');
console.log('NATIVE_BROWSER_PROFILE_CATALOG_SCALAR_REFERENCE_READY');
