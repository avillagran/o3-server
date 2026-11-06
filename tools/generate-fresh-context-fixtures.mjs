#!/usr/bin/env node
// Execute the exact pure boundary/session policy from the pinned nightly, never a rewritten oracle.
import fs from 'node:fs';
import {execFileSync} from 'node:child_process';
import {stripTypeScriptTypes} from 'node:module';
import vm from 'node:vm';
import crypto from 'node:crypto';
const source=process.argv[2];if(!source)throw Error('usage: generate-fresh-context-fixtures.mjs T3_CHECKOUT');
const revision='c55db3e9c72fdf584f4b54192fb39beb88667b59';
const read=path=>execFileSync('git',['-C',source,'show',`${revision}:${path}`],{encoding:'utf8',maxBuffer:16<<20});
const extract=(text,name)=>{const start=text.indexOf(`function ${name}(`);if(start<0)throw Error(name);let at=text.indexOf('{',start),depth=1;for(at++;depth&&at<text.length;at++){if(text[at]==='{')depth++;else if(text[at]==='}')depth--;}if(depth)throw Error('unbalanced '+name);return text.slice(start,at)};
const orch=read('apps/server/src/orchestration-v2/Orchestrator.ts'),restart=read('apps/server/src/orchestration-v2/RestartBackgroundNote.ts');
const names=['neverStarted','cancelledBeforeStart','contextBoundaryOrdinal','providerThreadPredatesBoundary','sessionsReleasedByFreshContext'];
const functions=names.map(n=>extract(n==='neverStarted'||n==='cancelledBeforeStart'?restart:orch,n)).join('\n');
const native=vm.runInNewContext(stripTypeScriptTypes(functions)+`\n({${names.join(',')}})`);
const run=(ordinal,status,freshContext,startedAt=null,extra={})=>({ordinal,status,freshContext,startedAt,...extra});
const sets=[[],[run(1,'completed',false,'now')],[run(1,'completed',true,'now'),run(2,'queued',true),run(3,'cancelled',true)],[run(1,'completed',true,'now'),run(2,'preparing',true)],[run(2,'cancelled',true,null,{restartContinuationOfRunId:'source'})],[run(2,'failed',true)],[run(2,'cancelled',true,'now')],[run(2,'rolled_back',true,'now')]];
const boundaries=sets.flatMap(runs=>[0,1,2,99].map(through=>({runs,through,expected:native.contextBoundaryOrdinal(runs.filter(r=>r.ordinal<=through))})));
const threads=[0,1,3].flatMap(boundary=>[null,1,3,4].map(firstRunOrdinal=>({thread:{firstRunOrdinal},boundary,expected:native.providerThreadPredatesBoundary({firstRunOrdinal},boundary)})));
const sessions=['ready','running','stopped','error'].flatMap(status=>[true,false].flatMap(multi=>[true,false].map(active=>{const providerSessions=[{id:'one',status,capabilities:{sessions:{supportsMultipleProviderThreadsPerSession:multi}}},{id:'unrelated',status:'ready',capabilities:{sessions:{supportsMultipleProviderThreadsPerSession:false}}}];const activeProviderThread=active?{providerSessionId:'one'}:null;return {providerSessions,activeProviderThread,expected:native.sessionsReleasedByFreshContext({providerSessions},activeProviderThread??undefined).map(s=>s.id)}})));
fs.writeFileSync('tests/fixtures/fresh-context-boundaries.json',JSON.stringify({revision,sourceHash:crypto.createHash('sha256').update(functions).digest('hex'),boundaries,threads,sessions},null,2)+'\n');
console.log(`${boundaries.length} boundaries, ${threads.length} thread rows, ${sessions.length} session release cases from ${revision}`);
