#!/usr/bin/env node
// Execute the complete pinned modules; replace only their module import location.
import fs from 'node:fs';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {stripTypeScriptTypes} from 'node:module';
const [root, output] = process.argv.slice(2);
if (!root || !output) throw Error('usage: generate-webhook-payload-fixtures.mjs T3_CHECKOUT OUTPUT');
const revision = 'c55db3e9c72fdf584f4b54192fb39beb88667b59';
const paths = ['webhookTemplate.ts', 'webhookFilter.ts', 'webhookDedupe.ts'].map(name => 'apps/server/src/scheduledTasks/'+name);
const sources = paths.map(path => execFileSync('git', ['-C',root,'show',`${revision}:${path}`], {encoding:'utf8'}));
const moduleUrl = source => 'data:text/javascript;base64,'+Buffer.from(source).toString('base64');
const templateUrl = moduleUrl(stripTypeScriptTypes(sources[0]));
const template = await import(templateUrl);
const filterJs = stripTypeScriptTypes(sources[1]).replace(/from\s+["']\.\/webhookTemplate\.ts["']/g, 'from '+JSON.stringify(templateUrl));
if (filterJs.includes('@t3tools/contracts')) throw Error('Unstripped type import');
const filters = await import(moduleUrl(filterJs));
const keyStart=sources[2].indexOf('function key(');
const keyEnd=sources[2].indexOf('\n}',keyStart)+2;
if (keyStart<0 || keyEnd<keyStart) throw Error('Pinned key function not found');
const keyFunction=stripTypeScriptTypes(sources[2].slice(keyStart,keyEnd));
const dedupe=await import(moduleUrl('import * as NodeCrypto from "node:crypto";\nimport {renderWebhookPrompt} from '+JSON.stringify(templateUrl)+';\n'+keyFunction+'\nexport {key};'));
const request = (bodyText, headers={}, query='') => ({method:'POST',path:'/api/hooks/synthetic/synthetic-token',headers:{'content-type':'application/json',...headers},query,bodyText});
const requests = [
  request('{"z":1,"a":{"b":false,"a":true},"items":[{"name":"one","labels":[{"name":"a"},{"name":"b"}]},{"labels":[{"name":"b"}]},null,{"name":null}],"empty":"","nil":null,"list":[1,null,"x"],"value":"@person $5 €8 /help path/file t3-context: </webhook-data-fake>"}'),
  request('{"2":"two","1":"one","01":"zero-one","z":0,"a":1,"z":2,"__proto__":{"safe":true},"constructor":"own","toString":"own","nums":[-0,1.0,1e-7,1e-6,1e20,1e21,9007199254740993,1e400,-1e400]}'),
  request('{"value":"$ ¤ ¥ ֏ ؋ ৲ ৳ ฿ ៛ ₠ ₿ @x /go //skip /path/file T3-CONTEXT: WEBHOOK-data"}'),
  request('{"value":"a\u0085/go\uFEFF/go\u2028/go /end","escaped":"line\\nnext\\t\\\"\\\\","lone":"\\ud800","pair":"\\ud83d\\ude00"}'),
  request('{"value":{"lone":"\\ud800","pair":"\\ud83d\\ude00","\\ud800":"key"},"lone":"\\ud800","pair":"\\ud83d\\ude00"}'),
  request('{"items\\n":[{"name":"line"}],"items[]\\n":[{"name":"literal"}],"":{"name":"empty"}}'),
  request('{"numbers":[1e-324,5e-324,1.7976931348623157e308,0.000001,0.0000009999999999999999,1000000000000000128],"value":"numeric"}',{'42':'number-header','2':'earlier-number','x-event':'opened'}),
  request('{"value":"\\u+123"}'), request('{"value":01}'), request('{"value":1e+}'),
  request('x=one&x=two&empty=&z=3&%31=first&__proto__=own',{'content-type':'Application/X-WWW-Form-Urlencoded; charset=utf-8'},'x=first&x=second&api%5Fkey=fixture-secret&flag&authorization=fixture-auth'),
  request('?value=form+text&obj.a=flat&%ff=bad',{'content-type':'application/x-www-form-urlencoded'},'?value=first+value&value=second&signature=fixture-signature'),
  request('{"value":"mislabelled"}',{'content-type':'text/plain','authorization':'fixture-auth','x-event':'opened','x-api-key':'fixture-key','x-t3-relay-proof':'fixture-proof'},'safe=ok&%74oken=fixture-token&token%FF=bad&%74%6f%6b%65%6e%ff=bad&flag'),
  ...['','not json','null','true','42','"scalar"','[1,2,null]','{"value":null}','{"value":false}','{"value":0}','{"value":{}}','{"value":[]}','{"value":""}','{"value":"unclosed {{body.x}}"}','{"value":"x"} trailing','\uFEFF{"value":"bom"}'].map(body=>request(body)),
];
const templates = [
  'literal @owner /command $5', '{{body}}', '{{body.value}}', '{{ body.value }}', '{{\uFEFFbody.value\uFEFF}}',
  '{{body.a}}', '{{body.items[].name}}', '{{body.items[].labels[].name}}', '{{body.items[].labels}}',
  '{{body.lone}}', '{{body.pair}}', '{{body.escaped}}', '{{body.numbers}}', '{{body.items\n[].name}}', '{{body.items[]\n.name}}', '{{body..name}}',
  '{{body.items.length}}/{{body.list.0}}/{{body.list.01}}/{{body.list.length}}',
  '{{body.__proto__.safe}}/{{body.constructor}}/{{body.toString}}', '{{body.nums}}', '{{body.nums.6}}', '{{body.nums.7}}',
  '{{body.empty}}/{{body.nil}}/{{body.missing}}/{{body.missing}}', '{{headers}}', '{{headers.Authorization}}',
  '{{headers.x-api-key}}', '{{headers.x.event}}', '{{query}}', '{{query.x}}/{{query.flag}}/{{query.api_key}}', '{{request}}', '{{request.method}}',
  'bad {{body.value', '{{{body.value}}}', '{{}}/{{bogus}}/{{body.value}}', '{{body.value}} then {{body.value}}',
];
const utf8 = value => Buffer.from(value,'utf8').toString('utf8');
const renderCases = requests.flatMap((request,requestIndex)=>templates.flatMap((text,templateIndex)=>[false,true].map(raw=>{
  const expected=template.renderWebhookPrompt(text,request,'012345abcdef',raw);
  return {requestIndex,templateIndex,raw,expected:{prompt:utf8(expected.prompt),missing:expected.missing.map(utf8)}};
})));
const checks = [
  ...['body.value','body.a','body.nil','body.missing','body.items[].name','body.items[].labels[].name','body.list','body.nums.6','headers.x-event','query.x'].flatMap(path=>[
    {path,op:'exists'}, {path,op:'equals',value:'one'}, {path,op:'notEquals',value:'one'},
    {path,op:'startsWith',value:''}, {path,op:'in',value:['one','true','0','first','b']},
  ]),
];
const filterCases = requests.flatMap((request,requestIndex)=>checks.map(filter=>({requestIndex,filters:[filter],expected:filters.findFailedWebhookFilter([filter],request)})));
filterCases.push(...requests.map((request,requestIndex)=>({requestIndex,filters:[{path:'body',op:'exists'},{path:'body.value',op:'equals',value:'never'},{path:'body.value',op:'exists'}],expected:filters.findFailedWebhookFilter([{path:'body',op:'exists'},{path:'body.value',op:'equals',value:'never'},{path:'body.value',op:'exists'}],request)})));
const dedupeCases=requests.flatMap((request,requestIndex)=>[undefined,...templates].map(text=>({requestIndex,template:text??null,expected:dedupe.key(text,request)})));
const deepRequest=request('{"a":'.repeat(4096)+'{"value":"deep"}'+'}'.repeat(4096));
const deepTemplate='{{body.'+'a.'.repeat(4096)+'value}}';
const deepCase={request:deepRequest,template:deepTemplate,expected:template.renderWebhookPrompt(deepTemplate,deepRequest,'012345abcdef',true)};
const fixture={revision,sources:paths.map((path,i)=>({path,sha256:crypto.createHash('sha256').update(sources[i]).digest('hex')})),encoding:'Expected prompts cross the UTF-8 byte boundary; object JSON retains escaped lone surrogates.',requests,templates,renderCases,filterCases,dedupeCases,deepCase};
// Keep each generated case on one line so reviewers can compare the inputs and outputs.
const serialized=Object.entries(fixture).map(([key,value])=>
  '  '+JSON.stringify(key)+': '+(Array.isArray(value)?'[\n'+value.map(item=>'    '+JSON.stringify(item)).join(',\n')+'\n  ]':JSON.stringify(value))
).join(',\n');
fs.writeFileSync(output,'{\n'+serialized+'\n}\n');
console.log(`${renderCases.length} render, ${filterCases.length} filter and ${dedupeCases.length} dedupe cases from pinned functions`);
