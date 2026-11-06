#!/usr/bin/env node
// Execute the pinned upstream's pure process isolation helpers.
import * as NodeCrypto from 'node:crypto';
import {readFileSync, writeFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const [directory, output] = process.argv.slice(2);
if (!directory || !output) throw Error('Usage: generate-agent-scope-fixtures.mjs upstream-directory output.json');
const source = readFileSync(directory + '/apps/server/src/process/agentScope.ts', 'utf8');
const helpers = source.slice(source.indexOf('export function ownedAgentSlice('), source.indexOf('function parseKeyValues('));
if (!helpers.includes('function agentSliceMemoryLimits(') || !helpers.includes('function classifyScope(')) throw Error('Pinned source layout changed');
const code = stripTypeScriptTypes(helpers + '\nexport {agentSliceMemoryLimits};');
// Source imports are supplied explicitly; no upstream module initialization runs.
const api = new Function('NodeCrypto', code.replaceAll('export ', '') + '; return {ownedAgentSlice,agentSliceMemoryLimits,classifyScope};')(NodeCrypto);
const paths = ['/tmp/profile/userdata', '/home/example/.t3/userdata', '/tmp/á🚀/userdata', '', '/tmp/a/../b'];
const totals = [0, 1, 3, 4, 1024, ...[1, 2, 4, 6, 8, 16, 32, 64].map(gib => gib * 1024 ** 3), 16 * 1024 ** 3 + 3];
const states = [];
for (const loadState of ['loaded', 'not-found']) for (const activeState of ['active', 'inactive', 'failed', 'deactivating']) for (const result of ['success', 'oom-kill']) for (const oomKills of [undefined, 0, 1]) for (const populated of [undefined, false, true]) {
    const input = {loadState, activeState, result, oomKills, populated};
    states.push({input, expected: api.classifyScope(input)});
}
writeFileSync(output, JSON.stringify({source:'c55db3e9c72fdf584f4b54192fb39beb88667b59', slices: paths.map(input => ({input, expected:api.ownedAgentSlice(input)})), limits:totals.map(input => ({input, expected:api.agentSliceMemoryLimits(input)})), states}, null, 2) + '\n');
