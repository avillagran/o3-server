#!/usr/bin/env node
// Execute the pinned native worker decision with its actual Effect and ID schemas.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {createRequire, stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [source, output] = process.argv.slice(2);
const revision = '4cb6945dc60ed03f567dc5daafd083e52308e82b';
const manifest = JSON.parse(fs.readFileSync(source + '/SOURCE.json', 'utf8'));
if (manifest.revision !== revision) throw Error('Pinned native input required');
for (const [name, hash] of Object.entries(manifest.files)) {
  if (crypto.createHash('sha256').update(fs.readFileSync(source + '/' + name)).digest('hex') !== hash)
    throw Error('Changed native input: ' + name);
}
const require = createRequire(source + '/package.json');
const effectPackage = JSON.parse(fs.readFileSync(new URL('../package.json', pathToFileURL(require.resolve('effect/DateTime'))), 'utf8'));
if (effectPackage.version !== '4.1.2') throw Error('Effect 4.1.2 required');
const DateTime = await import(pathToFileURL(require.resolve('effect/DateTime')).href);
const {CommandId, MessageId} = await import(pathToFileURL(source + '/packages/contracts/src/baseSchemas.ts').href);
const name = 'apps/server/src/orchestration-v2/UsageLimitRecoveryWorker.ts';
const text = fs.readFileSync(source + '/' + name, 'utf8');
const constant = text.match(/^const MAX_OVERDUE_ARM_MS = .*;$/m)?.[0];
const start = text.indexOf('export function limitRecoveryCommand(');
const end = text.indexOf('// Events that can make a thread a candidate');
if (!constant || start < 0 || end < start) throw Error('Native extraction changed');
const code = stripTypeScriptTypes(constant + '\n' + text.slice(start, end).replace('export function', 'function'));
const decide = new Function('CommandId', 'MessageId', 'DateTime', code + '\nreturn limitRecoveryCommand;')(CommandId, MessageId, DateTime);
const reset = '2026-11-07T03:00:00.000Z';
const resetMs = Date.parse(reset);
const base = {id:'thread', status:'failed', lastErrorClass:'usage_limit', latestRunId:'run', usageLimitResetAt:reset,
  latestRunCompletedAt:'2026-11-07T02:59:00.000Z', updatedAt:'2026-11-07T02:59:01.000Z',
  archivedAt:null, settledOverride:null, pendingRuntimeRequest:null, snoozedUntil:null, limitRecovery:null};
const variants = [base,
  ...['status','lastErrorClass','latestRunId','usageLimitResetAt','archivedAt','settledOverride','pendingRuntimeRequest'].flatMap(key =>
    [null, '', key === 'settledOverride' ? 'settled' : key === 'status' ? 'running' : 'other'].map(value => ({...base,[key]:value}))),
  ...[null, reset, '2026-11-07T03:01:00.000Z'].map(latestRunCompletedAt => ({...base,latestRunCompletedAt})),
  ...[false,true].flatMap(autoResume => [undefined,'request-1'].flatMap(requestId => [false,true].map(snooze =>
    ({...base,limitRecovery:{runId:'run',resetAt:reset,autoResume,snooze,...(requestId === undefined ? {} : {requestId})}})))),
  {...base,limitRecovery:{runId:'old',resetAt:reset,autoResume:true}},
  {...base,limitRecovery:{runId:'run',resetAt:'2026-11-07T04:00:00.000Z',autoResume:true}},
  ...['2026-11-07T02:59:00.000Z',reset,'2026-11-07T03:01:00.000Z'].map(snoozedUntil =>
    ({...base,snoozedUntil,limitRecovery:{runId:'run',resetAt:reset,autoResume:true,requestId:'fresh'}}))];
const cases = [];
for (const thread of variants) for (const autoResume of [false,true]) for (const snooze of [false,true])
  for (const nowMs of [resetMs-1,resetMs,resetMs+1,resetMs+86400000,resetMs+86400001]) {
    const candidate = structuredClone(thread);
    // These fields are Effect DateTime values in ProjectionLimitRecoveryCandidate.
    for (const key of ['latestRunCompletedAt','updatedAt','snoozedUntil'])
      if (candidate[key] != null) {
        candidate[key] = DateTime.makeUnsafe(candidate[key]);
      }
    cases.push({input:{thread,autoResume,snooze,nowMs},expected:decide(candidate,autoResume,nowMs,snooze)});
  }
fs.writeFileSync(output, JSON.stringify({revision,effectVersion:'4.1.2',sources:manifest.files,cases},null,2)+'\n');
console.log('NATIVE_LIMIT_RECOVERY_PASS: '+cases.length+' actual worker decisions');
