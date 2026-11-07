// Real production Rust tab/pipe transport in the reviewed Chromium container.
import fs from 'node:fs';import assert from 'node:assert/strict';import {spawnSync} from 'node:child_process';
assert(fs.existsSync('/run/.containerenv'));assert(fs.existsSync('/usr/bin/chromium'));
fs.mkdirSync('/out/guard',{mode:0o700});process.env.O3_RECURSIVE_FS_GUARD_DIR='/out/guard';await import('/guard.mjs');
const result=spawnSync('/supervisor',[],{encoding:'utf8',timeout:30000,maxBuffer:65536});
assert.equal(result.status,0,(result.stderr??'').slice(-8192));const proof=JSON.parse(result.stdout);assert(proof.complete&&proof.actualRustOwnerPipe&&proof.exactArguments&&proof.exactResult&&proof.otherTargetSurvived&&proof.lateResponseDrained&&proof.contextOverrideRefused&&proof.closedOwnerRefused);assert.equal(proof.profileRemovalRequests,0);
assert.equal(fs.readdirSync('/out/guard').filter(name=>name.endsWith('.attempts.jsonl')).length,0);fs.writeFileSync('/out/proof.json',JSON.stringify(proof,null,2)+'\n');console.log('ACTUAL_OWNED_RAW_PIPE_PASS');
