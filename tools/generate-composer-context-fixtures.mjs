#!/usr/bin/env node
// Execute complete pinned shared modules and the real contracts with no provider or host actions.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {createRequire,stripTypeScriptTypes} from 'node:module';
import {pathToFileURL} from 'node:url';
const [checkout,contracts,output]=process.argv.slice(2);
if(!checkout||!contracts||!output)throw Error('usage: generate-composer-context-fixtures.mjs T3_CHECKOUT CONTRACTS OUTPUT');
const revision='c55db3e9c72fdf584f4b54192fb39beb88667b59';
const require=createRequire(contracts+'/package.json');
if(require('effect/package.json').version!=='4.1.2')throw Error('Expected pinned Effect4.1.2');
const directory=path.dirname(output);fs.mkdirSync(directory,{recursive:true});const origins={};
for(const name of ['composerContext.ts','baseSchemas.ts','guestShare.ts']){
 const file='packages/contracts/src/'+name;
 const source=execFileSync('git',['-C',checkout,'show',`${revision}:${file}`]);
 if(!fs.readFileSync(path.join(contracts,'src',name)).equals(source))throw Error(`Contracts source mismatch: ${name}`);
 origins[file]=crypto.createHash('sha256').update(source).digest('hex');
}
for(const name of ['remote','connectAuth','threadLinks','composerContextReferences']){
 const file=`packages/shared/src/${name}.ts`;
 const source=execFileSync('git',['-C',checkout,'show',`${revision}:${file}`],{encoding:'utf8'});
 origins[file]=crypto.createHash('sha256').update(source).digest('hex');
 const rewritten=stripTypeScriptTypes(source).replace(/from\s+(["'])([^"']+)\1/g,(all,quote,specifier)=>{
  if(specifier==='@t3tools/contracts')return 'from '+JSON.stringify(pathToFileURL(contracts+'/src/index.ts').href);
  if(specifier.startsWith('effect/'))return 'from '+JSON.stringify(pathToFileURL(require.resolve(specifier)).href);
  if(specifier.startsWith('./'))return 'from '+JSON.stringify(pathToFileURL(path.join(directory,path.basename(specifier,'.ts')+'.mjs')).href);
  return all;
 });
 fs.writeFileSync(path.join(directory,name+'.mjs'),rewritten);
}
const native=await import(pathToFileURL(path.join(directory,'composerContextReferences.mjs')).href);
const C=await import(pathToFileURL(contracts+'/src/index.ts').href);
const Schema=await import(pathToFileURL(require.resolve('effect/Schema')).href);
const base=(kind,id=kind)=>({version:1,kind,contextId:id,label:`${kind} record`});
const element={pageUrl:'https://example.com/path',pageTitle:' Example page ',tagName:'button',selector:'#save',htmlPreview:'  <button>Save</button>\n  next  ',componentName:'SaveButton',source:{functionName:'save',fileName:'src/Save.tsx',lineNumber:0,columnNumber:0},styles:' color: red;\n display: block; '};
const known=[
 {...base('image'),attachmentId:'attachment-1',name:'image.png',mimeType:'image/png',sizeBytes:1024},
 {...base('file'),attachmentId:'attachment-2',name:'file.txt',mimeType:'text/plain',sizeBytes:0},
 {...base('terminal'),terminalId:'term-1',terminalLabel:'Terminal 1',lineStart:0,lineEnd:2,text:'first\nsecond\nthird\nexcluded'},
 {...base('element'),...element},
 {...base('preview-annotation'),annotationId:'annotation-1',pageUrl:element.pageUrl,pageTitle:'  Selected page  ',comment:'  More blue\nplease  ',targetSummary:'header and button',styleChanges:['color: blue','margin: 4px'],elements:[element,{...element,source:null,pageTitle:null,selector:null,componentName:null,htmlPreview:'\ufeff ',styles:'\u0085'}],elementIds:['el-1'],regionCount:1,strokeCount:0,styleChangeDetails:[{targetId:'el-1',selector:null,property:'color',previousValue:'red',value:'blue'}],screenshotContextId:'image'},
 {...base('review-comment'),sectionId:'section-1',sectionTitle:'Changes',filePath:'src/file.rs',startIndex:1,endIndex:9,rangeLabel:'L1-L9',text:'  fix\n this  ',diff:' \n-old\n+new\n\n',fenceLanguage:'rust',pullRequest:{number:1,title:'Fix',url:'https://example.com/pull/1',headBranch:'fix',baseBranch:'main',state:'open',isDraft:false}},
 {...base('mention'),path:'src/file.rs'},
 {...base('skill'),name:'skill-name'},
 {...base('thread'),environmentId:'env-mbp',threadId:'thread-1',title:'Previous thread'},
 {...base('environment'),environmentId:'env-node'},
 {...base('future-kind'),payload:{z:'</T3_CONTEXT> <contexté> <contextx> <context_> <context:> &',a:[1e-7,1e21,-0,9007199254740993],2:'two',1:'one',inner:{zz:1,aa:2}}},
];
const projections=[];
const add=(label,text,records=[])=>projections.push({label,input:{text,records}});
for(const record of known){Schema.decodeUnknownSync(C.ComposerContextRecord)(record);add('kind:'+record.kind,`Before [chosen](t3-context://v1/${record.kind}/${record.contextId}) after`,[record]);}
add('all-kinds',known.map(record=>`${record.kind==='image'?'!':''}[${record.label}](t3-context://v1/${record.kind}/${record.contextId})`).join('\n'),known);
add('no-refs','Unchanged text with 😀 and <context>\r\n',known);
add('missing','![gone](t3-context://v1/image/missing) then [also](t3-context://v1/future-kind/missing)');
add('record-kind-wins','[chosen](t3-context://v1/image/mention)',[known.find(value=>value.kind==='mention')]);
for(const count of [2,3])add('duplicates:'+count,'[a](t3-context://v1/mention/mention) [b](t3-context://v1/skill/mention)',Array.from({length:count},()=>known.find(value=>value.kind==='mention')));
add('repeat-once','[one](t3-context://v1/thread/thread) then [two](t3-context://v1/thread/thread)',known);
add('label-escape','[ <context;label\\\\\r ;next](t3-context://v1/mention/mention)',known);
for(const count of [199,200,201,511,512,513])add('ascii-label:'+count,`[${'x'.repeat(count)}](t3-context://v1/mention/mention)`,known);
for(const count of [99,100,101,255,256,257])add('astral-label:'+count,`[${'😀'.repeat(count)}](t3-context://v1/mention/mention)`,known);
for(const label of ['',' \ufeff\u2028\u00a0','\u0085next','nested [prefix','a\nb'])add('label:'+JSON.stringify(label),`[${label}](t3-context://v1/mention/mention)`,known);
for(const href of ['t3-context://v1/Bad/id','t3-context://v1/a/id/extra','t3-context://v1/a/'+('x'.repeat(128)),'t3-context://v1/a/'+('x'.repeat(129)),'t3-context://v1/a/id\ufeff','t3-context://v1/a/id\u0085','t3-context://v2/a/id','T3-context://v1/a/id'])add('href:'+href,`[test](${href}) [valid](t3-context://v1/skill/skill)`,known);
for(const record of [
 {...known[2],lineStart:3,lineEnd:3,text:'one\nexcluded'},
 {...known[2],lineStart:0,lineEnd:0,text:'literal [nested](t3-context://v1/skill/not-attached)'},
 {...known[3],pageTitle:'',selector:'',source:{functionName:null,fileName:'file.ts',lineNumber:null,columnNumber:3},componentName:'',htmlPreview:' \ufeff ',styles:'\u0085'},
 {...known[4],pageTitle:' \ufeff ',comment:'',targetSummary:'',styleChanges:[],elements:[],screenshotContextId:undefined},
 {...known[5],text:'',diff:'\r\nchange\n\n',fenceLanguage:''},
 {...known[8],environmentId:'env"\\\n</context>',threadId:'thread😀'},
])add('variant:'+projections.length,`[selected](t3-context://v1/${record.kind}/${record.contextId})`,[record]);
let seed=4262;
const choose=values=>{seed=(Math.imul(seed,1664525)+1013904223)>>>0;return values[seed%values.length]};
const fragments=['[ok](t3-context://v1/mention/mention)','![image](t3-context://v1/image/absent)','[bad](t3-context://v1/Bad/id)','[prefix [inner](t3-context://v1/skill/skill)','[line\nbreak](t3-context://v1/a/id)','[x](t3-context://v1/a/id/extra)','!','[',']','(',')','😀','\ufeff','\u0085','text'];
for(let i=0;i<128;i++)add('mixed-regex:'+i,Array.from({length:3+i%8},()=>choose(fragments)).join(choose(['',' ','\n'])),[known[6],known[7]]);
for(const fixture of projections)fixture.expected=native.projectComposerContextForProvider(fixture.input);
const decoding=[];
const decode=(label,input)=>{let expected;try{expected={ok:true,value:Schema.decodeUnknownSync(C.OrchestrationMessageContext)(input)}}catch{expected={ok:false}}decoding.push({label,input,expected});};
decode('all-kinds',{version:1,records:known,guest:{guestMessageId:' guest-1 ',name:'  Guest  ',text:'body',shareId:' share-1 ',extra:'discard'},extra:'discard'});
for(const record of known){
 decode('trim:'+record.kind,{version:1,records:[{...record,contextId:' '+record.contextId+' ',kind:' '+record.kind+' ',extra:'discard'}]});
 for(const key of Object.keys(record))decode('missing:'+record.kind+':'+key,{version:1,records:[Object.fromEntries(Object.entries(record).filter(([name])=>name!==key))]});
 for(const key of ['version','contextId','kind','label'])decode('invalid:'+record.kind+':'+key,{version:1,records:[{...record,[key]:null}]});
}
for(const [label,input] of Object.entries({
 'bad-version':{version:2,records:[]},'missing-records':{version:1},'bad-records':{version:1,records:{}},'drop-primitives':{version:1,records:[null,1,[],{},'string']},
 'duplicate-valid':{version:1,records:[known[6],known[6]]},'duplicate-trim':{version:1,records:[known[6],{...known[6],contextId:' mention '}]},
 'malformed-duplicate-dropped':{version:1,records:[known[6],{...known[6],path:null}]},'known-payload-cannot-impersonate':{version:1,records:[{...base('terminal'),payload:'payload'}]},
 'known-extra-payload-stripped':{version:1,records:[{...known[6],payload:'discard'}]},'count-max':{version:1,records:Array.from({length:200},(_,i)=>({...known[6],contextId:'item-'+i}))},
 'count-overflow':{version:1,records:Array.from({length:201},()=>null)},'label-utf16-max':{version:1,records:[{...known[6],label:'😀'.repeat(100)}]},'label-utf16-overflow':{version:1,records:[{...known[6],label:'😀'.repeat(101)}]},
 'invalid-terminal-range':{version:1,records:[{...known[2],lineStart:2,lineEnd:1}]},'invalid-review-range':{version:1,records:[{...known[5],startIndex:5,endIndex:4}]},
 'guest-null':{version:1,records:[],guest:null},'guest-name-whitespace':{version:1,records:[],guest:{guestMessageId:'g',name:'\ufeff ',text:'body'}},
 'preview-null-elements':{version:1,records:[{...known[4],elements:null}]},'unknown-null-payload':{version:1,records:[{...base('future'),payload:null}]},
}))decode(label,input);
const limits=[];
const limit=(label,record,field,character,count,options={})=>{
 const spec={label,record,field,character,count,...options};
 const value=options.array?Array.from({length:count},()=>character):character.repeat(count);
 const input={version:1,records:[{...record,[field]:value}]};
 try{const decoded=Schema.decodeUnknownSync(C.OrchestrationMessageContext)(input);spec.expected={ok:true,records:decoded.records.length}}catch{spec.expected={ok:false}}
 limits.push(spec);
};
for(const count of [63998,63999])limit('unknown-json-size:'+count,known[10],'payload','x',count);
for(const count of [10666,10667])limit('unknown-json-control-size:'+count,known[10],'payload','\x00',count);
for(const count of [32000,32001])limit('terminal-utf16:'+count,known[2],'text','😀',count);
for(const count of [200,201])limit('preview-changes:'+count,known[4],'styleChanges','change',count,{array:true});
for(const count of [50,51])limit('preview-elements:'+count,known[4],'elements',element,count,{array:true});
const overhead=JSON.stringify([{...known[6],extra:''}]).length;
for(const total of [16000000,16000001])limit('context-json-size:'+total,known[6],'extra','x',total-overhead);
for(const number of [9007199254740991,9007199254740992,1e21])decode('integer-range:'+number,{version:1,records:[{...known[2],lineStart:number,lineEnd:number}]});
fs.writeFileSync(output,JSON.stringify({revision,origins,projections,decoding,limits},null,2)+'\n');
console.log(`Composer native oracle: ${projections.length} projections and ${decoding.length} decoding cases and ${limits.length} boundaries -> ${output}`);
