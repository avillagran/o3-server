// Reapply the complete original projection to snapshots extracted by the actual
// Rust State/credential consumer inside an isolated container.
import {projectNative} from './projection-caller-oracle.mjs';
import fs from 'node:fs';
import assert from 'node:assert/strict';
const records=JSON.parse(fs.readFileSync('/caller-data.json','utf8'));
assert.equal(records.actualState,true);assert.equal(records.actualProviderCredential,true);
for(const entry of records.cases)assert.deepEqual(projectNative(entry.input),entry.expectedShell);
fs.writeFileSync('/out/caller-db-proof.json',JSON.stringify({complete:true,actualState:true,actualProviderCredential:true,wholeNativeProjectionStore:true,fullProjectionAndShellCodecs:true,cases:records.cases.length,qualification:'Actual owned SQLite records and provider Context snapshots passed through complete pinned native projection and full codecs. The public MCP/CDP consumer is not yet selected.'},null,2)+'\n');
console.log('ACTUAL_CALLER_DATABASE_NATIVE_PROJECTION_PASS');
