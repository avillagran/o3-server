#!/usr/bin/env node
// Execute the exact reference mode functions, including legacy/stale client saves.
import fs from 'node:fs';
import {execFileSync} from 'node:child_process';
import {stripTypeScriptTypes} from 'node:module';
import vm from 'node:vm';
import crypto from 'node:crypto';
const root = process.argv[2];
if (!root) throw Error('usage: generate-task-mode-fixtures.mjs T3_CHECKOUT');
const revision = 'c55db3e9c72fdf584f4b54192fb39beb88667b59';
const read = path => execFileSync('git', ['-C',root,'show',`${revision}:${path}`], {encoding:'utf8'});
function extract(source, name) {
  const at = source.indexOf(`function ${name}(`);
  if (at < 0) throw Error(name);
  const body = source.indexOf('{',at);
  let end=body+1, depth=1;
  for (; depth && end<source.length; end++) {
    if (source[end]==='{') depth++;
    if (source[end]==='}') depth--;
  }
  if (depth) throw Error('unbalanced '+name);
  return source.slice(at,end);
}
const functions = extract(read('packages/contracts/src/scheduledTask.ts'),'scheduledTaskThreadMode')+'\n'+extract(read('apps/server/src/scheduledTasks/ScheduledTaskService.ts'),'resolveThreadMode');
const native = vm.runInNewContext(stripTypeScriptTypes(functions)+'\n({scheduledTaskThreadMode,resolveThreadMode})');
const modes = [undefined,'new_thread','continue','fresh_session'];
const stored = [null,...modes.flatMap(threadMode => [null,'bound'].map(threadId=>({threadId,...(threadMode===undefined?{}:{threadMode})})))];
const cases = stored.flatMap(existing=>modes.flatMap(threadMode=>[undefined,null,'asked'].map(threadId=>{
  const input = {...(threadId===undefined?{}:{threadId}),...(threadMode===undefined?{}:{threadMode})};
  return {existing,input,expected:native.resolveThreadMode(input,existing)};
})));
const legacy = stored.filter(Boolean).map(task=>({task,expected:native.scheduledTaskThreadMode(task)}));
fs.writeFileSync('tests/fixtures/task-modes.json',JSON.stringify({revision,sourceHash:crypto.createHash('sha256').update(functions).digest('hex'),cases,legacy},null,2)+'\n');
console.log(`${cases.length} save cases, ${legacy.length} stored cases from ${revision}`);
