#!/usr/bin/env node
// Execute the pinned upstream failure helpers with their real Effect dependency.
import {readFileSync, writeFileSync} from 'node:fs';
import {createRequire, stripTypeScriptTypes} from 'node:module';
const [directory, dependencyRoot, output] = process.argv.slice(2);
if (!output) throw Error('Usage: generate-provider-failure-fixtures.mjs upstream-directory effect-package-root output.json');
const require = createRequire(dependencyRoot + '/package.json');
const [Schema, Cause, DateTime] = await Promise.all(['effect/Schema', 'effect/Cause', 'effect/DateTime'].map(name => import(require.resolve(name))));
const failure = readFileSync(directory + '/packages/provider-core/src/server/failure.ts', 'utf8');
const allocator = readFileSync(directory + '/packages/provider-core/src/server/IdAllocator.ts', 'utf8');
const encode = allocator.slice(allocator.indexOf('const encodePart ='), allocator.indexOf('export function deriveThreadFromProviderThread'));
const derive = allocator.slice(allocator.indexOf('  providerTurn: (input) =>'), allocator.indexOf('  nodeFromProviderItem: (input) =>')) + allocator.slice(allocator.indexOf('  turnItemFromProviderItem: (input) =>'), allocator.indexOf('  approvalNode: (input) =>'));
if (!encode.includes('encodeURIComponent') || !derive.includes('TurnItemId.make')) throw Error('Pinned allocator source layout changed');
const brand = {make: value => value};
const ids = new Function('TurnItemId', 'ProviderTurnId', stripTypeScriptTypes(encode + 'const derive = {' + derive + '};') + '\nreturn {derive};')(brand, brand);
const body = failure.slice(failure.indexOf('export class ContextHandoffBudgetError'));
if (!body.includes('function makeProviderFailureTurnItem')) throw Error('Pinned failure source layout changed');
const api = new Function('Schema', 'Cause', 'DateTime', 'IdAllocator', stripTypeScriptTypes(body).replace(/^export /gm, '') + '\nreturn {makeProviderFailure,makeProviderFailureTurnItem,makeProviderRetryTurnItem};')(Schema, Cause, DateTime, ids);
const messages = [
  '', '   ', 'A useful failure.', '\ufeff padded \ufeff',
  'See https://alice:private@example.com/path?q=private#private).',
  'éhttps://example.com?token=private and xhttps://example.com?token=kept',
  'Bearer private; Basic private, bearer private', 'xBearer kept éBearer private',
  'api_key=private password:"private value" secret=private',
  '{"access_token":"private","Authorization":"Bearer private","other":"keep"}',
  "{'credential':'private','api-key':'private'}",
  'foo-api-key=private prefix_token = private secret="private value"',
  'sk-' + 'Q'.repeat(24), 'sk-' + 'Q'.repeat(20) + '---',
  'xsk-' + 'Q'.repeat(24), 'é sk-' + 'Q'.repeat(24),
  'before\u0000middle\u0007after\u007f\tline\nnext',
  'x'.repeat(4095) + '🧪' + 'suffix', '🧪'.repeat(2100),
  '\u0085https://example.com/one\u0085two?secret=private',
];
const inputs = [{}, ...messages.map(message => ({message}))];
for (const prefix of ['', 'x', 'é', 'K', 'ſ', '-', '_']) {
  for (const suffix of ['', '-', '---', 'K', 'é', '\u0085', '\ufeff']) {
    for (const secret of ['https://alice:private@example.com/path?secret=private', 'Bearer private', 'api_key=private', 'sk-' + 'Q'.repeat(20)]) {
      inputs.push({message:prefix + secret + suffix});
    }
  }
}
for (const kind of ['unknown','provider_error','transport_error','validation_error','usage_limit']) {
  inputs.push({message:'Native failure',class:kind,retryable:false,code:'native-code',resetAt:'2026-11-06T18:00:00+02:00'});
}
inputs.push({class:'usage_limit',resetAt:'invalid'}, {class:'usage_limit',resetAt:'2026-11-06'}, {message:'  ',code:'  '}, {message:'Failure',code:'x'.repeat(127)+'🧪suffix'});
for (const tag of ['ProviderAdapterTurnStartError','ProviderAdapterEventStreamError','ProviderAdapterOpenSessionError','ProviderAdapterResumeThreadError']) {
  inputs.push({cause:{_tag:tag,message:'private raw defect',cause:{_tag:'ProviderAdapterProtocolError',reason:'previous_turn_active'}}});
  inputs.push({cause:{_tag:tag,cause:{_tag:'CodexGoalObjectiveTooLongError',message:'Shorten the objective.'}}});
}
for (const cause of [
  {_tag:'ContextHandoffBudgetError'}, {_tag:'ContextHandoffBackgroundWorkError'},
  {_tag:'CodexAppServerRequestError',method:'thread/goal/set',message:'goal objective must not be empty'},
  {_tag:'CodexAppServerRequestError',method:'thread/goal/set',message:'private defect'},
  {_tag:'AcpProcessExitedError',signal:'SIGKILL'}, {_tag:'AcpProcessExitedError',code:17},
  {_tag:'AcpProcessExitedError',signal:'private output',code:1.5},
  {_tag:'ContextHandoffDeliveryUncertainError'},
  {_tag:'UnknownDefect',message:'private raw defect',code:'safe-code'},
]) inputs.push({cause});
const cases = inputs.map(input => ({input, expected: api.makeProviderFailure(input)}));
const occurredAt = DateTime.makeUnsafe('2026-11-06T18:00:00.123Z');
const cards = ['codex','claudeAgent'].flatMap(driver => ['provider_error','usage_limit'].flatMap(kind => [false,true].map(startup => {
  const input = {driver,threadId:'thread:ñ',runId:'run:1',nodeId:'node:1',providerThreadId:'provider:1',providerTurnId:startup ? ids.derive.providerTurn({driver,nativeTurnId:'failed:attempt:/🧪'}) : 'turn:/🧪',itemOrdinal:7,failure:api.makeProviderFailure({message:'Native failure',class:kind}),occurredAt,...(startup ? {retry:{attempt:2,maxAttempts:3,retryDelayMs:1000},retryStartedAt:DateTime.makeUnsafe('2026-11-06T17:59:00.123Z')} : {})};
  return {input:JSON.parse(JSON.stringify(input)),expected:api.makeProviderFailureTurnItem(input)};
})));
writeFileSync(output,JSON.stringify({source:'c55db3e9c72fdf584f4b54192fb39beb88667b59',cases,cards},null,2)+'\n');
