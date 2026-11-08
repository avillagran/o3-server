import fs from 'node:fs';import fsp from 'node:fs/promises';import path from 'node:path';import assert from 'node:assert/strict';
import {RetainingArtifactFs} from '../../runtime/preview/retaining-artifact-fs.mjs';
assert(fs.existsSync('/run/.containerenv'));
for(const name of ['tmp','quarantine','guard'])fs.mkdirSync('/out/'+name,{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const adapter=new RetainingArtifactFs({tempRoot:'/out/tmp',quarantineRoot:'/out/quarantine'});adapter.install();
const first=await fsp.mkdtemp('/out/tmp/playwright-artifacts-');fs.mkdirSync(first+'/nested',{mode:0o700});fs.writeFileSync(first+'/nested/value','retained synthetic bytes');
await Promise.all([fsp.rm(first,{recursive:true,force:true}),fsp.rm(first,{recursive:true,force:true})]);
assert(!fs.existsSync(first));assert(adapter.healthy);const receipt=adapter.receipts.find(r=>r.kind==='quarantined');assert.equal(fs.readFileSync('/out/quarantine/'+receipt.retained+'/nested/value','utf8'),'retained synthetic bytes');assert.equal(adapter.receipts.filter(r=>r.kind==='already-retained').length,1);
// A fixture link points only inside the retained private fixture directory.
fs.symlinkSync('tmp','/out/private-link');assert.throws(()=>new RetainingArtifactFs({tempRoot:'/out/private-link',quarantineRoot:'/out/quarantine'}),/link/);
const second=await fsp.mkdtemp('/out/tmp/playwright-artifacts-');fs.renameSync(second,'/out/quarantine/original-slot');fs.mkdirSync(second,{mode:0o700});await assert.rejects(fsp.rm(second,{recursive:true,force:true}),/identity/);assert(fs.existsSync(second));assert(fs.existsSync('/out/quarantine/original-slot'));assert(!adapter.healthy);
await assert.rejects(fsp.rm('/out/unowned',{recursive:true,force:true}),e=>e.code==='ERR_PRIVATE_PROOF_PROHIBITED');
assert(adapter.receipts.every(r=>r.executedRecursiveRemoval===false));fs.writeFileSync('/out/proof.json',JSON.stringify({complete:true,cases:['nonempty-retained','concurrent-duplicate','ancestor-link-refused','replacement-refused','unowned-guard-refused'],physicalRecursiveRemovals:0,expectedUnownedGuardRefusals:1,receipts:adapter.receipts},null,2)+'\n');console.log('RETAINING_ARTIFACT_FS_PASS');
