#!/usr/bin/env node
// Run pinned TypeScript policy, equality and adapter classifiers with no runtime services.
import {readFileSync,writeFileSync} from 'node:fs';
import {stripTypeScriptTypes} from 'node:module';
const [directory,output]=process.argv.slice(2);
if(!directory||!output)throw Error('Usage: generate-selection-fixtures.mjs source-directory output.json');
const read=name=>readFileSync(directory+'/'+name,'utf8');
const model=read('model.ts');const policy=read('policy.ts');const classifiers=read('selectionTransition.ts');
const source='const copySorted=(a,compare)=>[...a].sort(compare);'+model.slice(model.indexOf('function canonicalModelSelectionOptions('),model.indexOf('/**\n * Whether a composer'))+policy.slice(policy.indexOf('export type ProviderSessionTransition ='))+classifiers.slice(classifiers.indexOf('export interface ProviderSelectionTransitionInput'));
const api=await import('data:text/javascript;base64,'+Buffer.from(stripTypeScriptTypes(source)).toString('base64'));
const selection={instanceId:'codex',model:'one'};
const base={driver:'codex',continuationIdentity:{driverKind:'codex',continuationKey:'codex:home:/shared'},modelSelection:selection,runtimeMode:'full-access',interactionMode:'default',workspace:'/repo'};
const scenarios=[
 ['same',{}],['interaction',{interactionMode:'plan'}],['runtime',{runtimeMode:'approval-required'}],['workspace',{workspace:'/other'}],
 ['model',{modelSelection:{...selection,model:'two'}}],['options',{modelSelection:{...selection,options:[{id:'effort',value:'high'}]}}],
 ['false-is-explicit',{modelSelection:{...selection,options:[{id:'fastMode',value:false}]}}],['empty-options',{modelSelection:{...selection,options:[]}}],
 ['driver',{driver:'claudeAgent'}],['continuation-driver',{continuationIdentity:{...base.continuationIdentity,driverKind:'claudeAgent'}}],
 ['continuation-key',{continuationIdentity:{...base.continuationIdentity,continuationKey:'codex:home:/other'}}],
 ['instance',{modelSelection:{...selection,instanceId:'codex-alt'}}],
 ['model-workspace',{modelSelection:{...selection,model:'two'},workspace:'/other'}],
 ['model-runtime',{modelSelection:{...selection,model:'two'},runtimeMode:'approval-required'}],
 ['unavailable',{available:false}],['extra-identity-field',{continuationIdentity:{...base.continuationIdentity,extra:'ignored'}}]
];
const plans=[undefined,api.turnScopedSelectionTransition(),{type:'restart_session'},{type:'create_with_handoff'},{type:'reject',reason:'adapter refused'}];
const cases=scenarios.flatMap(([name,patch])=>plans.map(selectionTransition=>{const input={current:base,target:{...base,available:true,...patch},...(selectionTransition?{selectionTransition}:{})};return{name:name+'/'+(selectionTransition?.type??'missing'),input,expected:api.decideProviderSessionTransition(input)};}));
for(const available of [true,false]){const input={current:null,target:{...base,available}};cases.push({name:'fresh/'+available,input,expected:api.decideProviderSessionTransition(input)});}
const a=[{id:'reasoningEffort',value:'high'},{id:'fastMode',value:false}];
const equality=[
 [selection,{...selection,options:[]}], [{...selection,options:a},{...selection,options:[...a].reverse()}],
 [selection,{...selection,options:[{id:'fastMode',value:false}]}],
 [{...selection,options:[{id:'x',value:true}]},{...selection,options:[{id:'x',value:'true'}]}],
 [{...selection,options:[{id:'x',value:true},{id:'x',value:'true'}]},{...selection,options:[{id:'x',value:'true'},{id:'x',value:true}]}],
 [{...selection,options:[{id:'effort',value:'高'},{id:'effort',value:'básico'}]},{...selection,options:[{id:'effort',value:'básico'},{id:'effort',value:'高'}]}],
 [selection,{...selection,unrelated:'ignored'}],
 [{...selection,options:[{id:'x',value:false},{id:'x',value:false}]},{...selection,options:[{id:'x',value:false}]}]
].map(([left,right])=>({left,right,expected:api.modelSelectionsEqual(left,right)}));
const acp=[];
for(const supports of [false,true])for(const target of [selection,{...selection,model:'two'},{...selection,options:a}]){const input={current:selection,target,sessionCapabilities:{sessions:{supportsModelSwitchInSession:supports}}};acp.push({input,expected:api.acpSelectionTransition(input)});}
const result={reference:'9e85538535c8553901351112ab02dd1dd62db5ee',sourceFiles:['apps/server/src/orchestration-v2/ProviderSessionTransitionPolicy.ts','packages/provider-core/src/server/selectionTransition.ts','packages/shared/src/model.ts'],cases,equality,acp};
writeFileSync(output,JSON.stringify(result,null,2)+'\n');
