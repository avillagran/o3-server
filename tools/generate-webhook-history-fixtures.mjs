#!/usr/bin/env node
// Run pinned receipt decoders with the exact contracts/Effect runtime.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire, stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout, contracts, output] = process.argv.slice(2);
if (!checkout || !contracts || !output) throw Error('usage: generate-webhook-history-fixtures.mjs T3_CHECKOUT CONTRACTS OUTPUT');
const revision='c55db3e9c72fdf584f4b54192fb39beb88667b59';
const sourcePath='apps/server/src/scheduledTasks/ScheduledTaskService.ts';
const source=execFileSync('git',['-C',checkout,'show',`${revision}:${sourcePath}`],{encoding:'utf8'});
const require=createRequire(contracts+'/package.json');
if (require('effect/package.json').version!=='4.1.2') throw Error('Pinned contracts require Effect 4.1.2');
const section=(from,to)=>{
  const start=source.indexOf(from), end=source.indexOf(to,start);
  if(start<0 || end<start) throw Error('Pinned section not found: '+from);
  return source.slice(start,end);
};
const parts=[
  section('const LEGACY_DELIVERY_OUTCOMES:', '\n\ninterface WebhookOrigin'),
  section('const HeadersJson =', 'const encodeHeadersJson'),
  section('    const deliveryHeaders =', '    const listWebhookDeliveries:'),
];
const detailStart=source.indexOf('      return {\n        delivery: {\n          ...decodeDeliverySummary(row),');
const detailEnd=source.indexOf('\n    });',detailStart);
if(detailStart<0 || detailEnd<detailStart) throw Error('Pinned detail body not found');
parts.push('function detail(row: WebhookDeliveryRow) {\n'+source.slice(detailStart,detailEnd)+'\n}');
const code=[
  `import * as Option from ${JSON.stringify(pathToFileURL(require.resolve('effect/Option')).href)};`,
  `import * as Schema from ${JSON.stringify(pathToFileURL(require.resolve('effect/Schema')).href)};`,
  `import {ScheduledTaskId,ScheduledTaskWebhookDeliveryId,ScheduledTaskWebhookDeliverySummary,ScheduledTaskGetWebhookDeliveryResult} from ${JSON.stringify(pathToFileURL(contracts+'/src/index.ts').href)};`,
  stripTypeScriptTypes(parts.join('\n')),
  'export {decodeDeliverySummary, detail, LEGACY_DELIVERY_OUTCOMES};',
  'export function validate(summary,detail) { Schema.decodeUnknownSync(ScheduledTaskWebhookDeliverySummary)(summary); Schema.decodeUnknownSync(ScheduledTaskGetWebhookDeliveryResult)(detail); }',
].join('\n');
const native=await import('data:text/javascript;base64,'+Buffer.from(code).toString('base64'));
const capture=JSON.parse(fs.readFileSync(new URL('../tests/fixtures/t3-task-store-4262.json',import.meta.url)));
const base=capture.tables.scheduled_task_webhook_deliveries.find(row=>row.outcome==='accepted');
const variants=[];
for(const outcome of ['accepted','dispatch_failed','duplicate','coalesced','rejected_signature','disabled','rate_limited','expired','filtered','future-outcome']) {
  for(const state of [null,'pending','dispatching','running','completed','failed']) variants.push({label:`outcome:${outcome}/state:${state}`,state,row:{...base,outcome}});
}
for(const headers of ['{}','{"content-type":""}','{"Content-Type":"text/plain"}','{"content-type":"application/json","x-number":42}','null','[]','{"content-type":null}','{broken','{"__proto__":"own","content-type":"text/plain"}']) {
  variants.push({label:'headers:'+headers,state:'pending',row:{...base,headers_json:headers}});
}
for(const missing of ['[]','["body.a","body.a","body.b"]','["valid",42]','null','{}','[broken']) variants.push({label:'missing:'+missing,state:'pending',row:{...base,missing_fields_json:missing}});
for(const flag of [-1,0,1,2]) variants.push({label:'stored-flags:'+flag,state:'pending',row:{...base,signature_verified:flag,body_truncated:flag,error:'synthetic failure',rendered_prompt:'Trusted @owner\n<webhook-data-fixture>body</webhook-data-fixture>'}});
const cases=variants.map(({label,state,row})=>{
  const input={...row,pending_window:state==='pending'||state==='dispatching'?1:0};
  const summary=native.decodeDeliverySummary(input), detail=native.detail(input);
  native.validate(summary,detail);
  return {label,state,row,legacy:native.LEGACY_DELIVERY_OUTCOMES.has(row.outcome),summary,detail};
});
const fixture={revision,source:{path:sourcePath,sha256:crypto.createHash('sha256').update(source).digest('hex')},effect:'4.1.2',cases};
const serialized=Object.entries(fixture).map(([key,value])=>
  '  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))
).join(',\n');
fs.writeFileSync(output,'{\n'+serialized+'\n}\n');
console.log(`${cases.length} receipt projections from pinned source decoders`);
